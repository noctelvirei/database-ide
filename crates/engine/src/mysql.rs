//! MySQL text-protocol sessions with bounded results and server-side cancellation.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::{StreamExt, future, stream};
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts, prelude::Queryable};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::connections::{Connection, SignInMethod, TlsMode};
use crate::danger::Engine;
use crate::session::{ConnectError, ConnectErrorKind, Event, PlanMode, Run, Session};

mod diagnostics;
mod execution;
mod grid_edit;
mod plans;
mod values;

/// Invalid saved details, unsupported sign-in or connection failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A MySQL connection must include a database login name.
    #[error("MySQL requires a MySQL connection and a user name")]
    InvalidConnection,
    /// MySQL sessions support password sign-in only.
    #[error("Windows sign-in is not supported for MySQL")]
    WindowsSignIn,
    /// Connection establishment exceeded the saved timeout in seconds.
    #[error("MySQL connection timed out")]
    ConnectTimeout,
    /// Driver or server diagnostic, without connection options or credentials.
    #[error("MySQL: {0}")]
    Server(String),
}

/// One password-authenticated connection. Runs serialize through cancellation cleanup;
/// credentials and driver connection settings are omitted from Debug.
pub struct MySql {
    database: String,
    database_changes: Vec<Arc<DatabaseChanges>>,
    requests: mpsc::Sender<Work>,
}

impl fmt::Debug for MySql {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MySql")
            .field("database", &self.current_database())
            .finish_non_exhaustive()
    }
}

struct Request {
    batch: String,
    mode: Option<PlanMode>,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
    stopped: CancellationToken,
    database_changes: Arc<DatabaseChanges>,
    timed_out: Arc<AtomicBool>,
}

enum Work {
    Run(Request),
    Save {
        script: crate::grid_edit::Script,
        reply: oneshot::Sender<Result<crate::grid_edit::SaveReport, crate::grid_edit::Error>>,
    },
}

struct DatabaseChanges {
    statements: Vec<diagnostics::Statement>,
    // Immutable slots let current_database return &str while the worker records USE.
    names: Vec<OnceLock<String>>,
    handled: AtomicBool,
}

/// Connect with saved details and a separately supplied password. TLS uses the native
/// certificate store; Optional falls back only when the server has no TLS capability.
/// UPDATE row counts include matched rows whose values did not change.
pub async fn connect(details: &Connection, password: Option<&str>) -> Result<MySql, ConnectError> {
    let invalid = |error: Error| {
        ConnectError::new(
            ConnectErrorKind::InvalidDetails,
            details,
            password,
            &error.to_string(),
        )
    };
    if details.sign_in == SignInMethod::Windows {
        return Err(invalid(Error::WindowsSignIn));
    }
    let user = details
        .user
        .as_deref()
        .filter(|user| !user.is_empty())
        .ok_or_else(|| invalid(Error::InvalidConnection))?;
    if details.engine != Engine::MySql || details.instance.is_some() {
        return Err(invalid(Error::InvalidConnection));
    }
    let ssl = (details.tls != TlsMode::Off).then(|| {
        SslOpts::default()
            .with_danger_accept_invalid_certs(details.trust_server_certificate)
            .with_danger_skip_domain_validation(details.trust_server_certificate)
    });
    let options: Opts = OptsBuilder::default()
        .ip_or_hostname(&details.host)
        .tcp_port(details.port.unwrap_or(3306))
        .prefer_socket(false)
        .client_found_rows(true)
        .user(Some(user))
        .pass(password)
        .db_name(details.database.as_deref())
        .connect_attribute("program_name", &details.application_name)
        .ssl_opts(ssl)
        .into();
    let establish = async {
        let (conn, options) = match Conn::new(options.clone()).await {
            Err(mysql_async::Error::Driver(
                mysql_async::DriverError::NoClientSslFlagFromServer,
            )) if details.tls == TlsMode::Optional => {
                let options: Opts = OptsBuilder::from_opts(options).ssl_opts(None).into();
                (Conn::new(options.clone()).await?, options)
            }
            result => (result?, options),
        };
        Ok::<_, mysql_async::Error>((conn, options))
    };
    let (conn, options) = if details.connect_timeout == 0 {
        establish.await
    } else {
        tokio::time::timeout(
            Duration::from_secs(details.connect_timeout.into()),
            establish,
        )
        .await
        .map_err(|_| ConnectError::new(ConnectErrorKind::Timeout, details, password, ""))?
    }
    .map_err(|error| connect_error(&error, details, password))?;
    let (requests, receiver) = mpsc::channel(1);
    tokio::spawn(worker(conn, options, receiver, details.query_timeout));
    let database = details.database.clone().unwrap_or_default();
    Ok(MySql {
        database,
        database_changes: Vec::new(),
        requests,
    })
}

fn connect_error(
    error: &mysql_async::Error,
    details: &Connection,
    password: Option<&str>,
) -> ConnectError {
    use mysql_async::{Error as DriverError, IoError};
    let kind = match error {
        DriverError::Server(error) if error.code == 1045 => ConnectErrorKind::BadCredentials,
        DriverError::Server(error) if matches!(error.code, 1049 | 1044) => {
            ConnectErrorKind::DatabaseUnavailable
        }
        DriverError::Io(IoError::Io(error)) => return ConnectError::io(error, details, password),
        DriverError::Io(IoError::Tls(_)) if crate::session::certificate_failure(error) => {
            ConnectErrorKind::Tls
        }
        _ => ConnectErrorKind::Other,
    };
    ConnectError::new(kind, details, password, &error.to_string())
}

impl MySql {
    /// Saves a confirmed MySQL grid script on this tab's session. Conflicts roll back
    /// the whole Save; an existing transaction uses a savepoint and stays open.
    /// Dropping the future cancels, drains and rolls back before reuse; a commit
    /// already sent may finish. UPDATE counts report matched rows, including no-ops.
    pub async fn save(
        &mut self,
        script: crate::grid_edit::Script,
    ) -> Result<crate::grid_edit::SaveReport, crate::grid_edit::Error> {
        if script.engine != Engine::MySql {
            return Err(crate::grid_edit::Error::Unsupported(script.engine));
        }
        let (reply, receiver) = oneshot::channel();
        self.requests
            .send(Work::Save { script, reply })
            .await
            .map_err(|_| grid_edit::closed())?;
        receiver.await.map_err(|_| grid_edit::closed())?
    }
}

impl Session for MySql {
    fn switch_database<'a>(
        &'a mut self,
        database: &'a str,
        _password: Option<&'a str>,
    ) -> futures::future::BoxFuture<'a, Result<(), crate::session::Error>> {
        Box::pin(crate::session::use_database(self, database, Engine::MySql))
    }

    fn current_database(&self) -> &str {
        self.database_changes
            .iter()
            .rev()
            .flat_map(|changes| changes.names.iter().rev())
            .find_map(OnceLock::get)
            .map_or(&self.database, String::as_str)
    }

    fn run<'a>(&'a mut self, batch: &'a str) -> Run<'a> {
        self.request(batch, None)
    }

    fn plan<'a>(&'a mut self, batch: &'a str, mode: PlanMode) -> Run<'a> {
        self.request(batch, Some(mode))
    }
}

impl MySql {
    fn request<'a>(&'a mut self, batch: &str, mode: Option<PlanMode>) -> Run<'a> {
        // Retire completed or unsent batches; a worker still recovering keeps its Arc.
        let completed = self
            .database_changes
            .iter()
            .take_while(|changes| {
                changes.handled.load(Ordering::Acquire) || Arc::strong_count(changes) == 1
            })
            .count();
        for changes in self.database_changes.drain(..completed) {
            if let Some(name) = changes.names.iter().rev().find_map(OnceLock::get) {
                self.database.clone_from(name);
            }
        }
        let cancel = CancellationToken::new();
        let (events, receiver) = mpsc::channel(16);
        let statements = diagnostics::statements(batch);
        let changes = Arc::new(DatabaseChanges {
            names: statements.iter().map(|_| OnceLock::new()).collect(),
            statements,
            handled: AtomicBool::new(false),
        });
        self.database_changes.push(changes.clone());
        let timed_out = Arc::new(AtomicBool::new(false));
        let request = Request {
            batch: batch.into(),
            mode,
            events,
            cancel: cancel.clone(),
            stopped: CancellationToken::new(),
            database_changes: changes.clone(),
            timed_out: timed_out.clone(),
        };
        let requests = self.requests.clone();
        let source = stream::once(async move {
            if requests.send(Work::Run(request)).await.is_err() {
                return stream::once(async { closed() }).boxed();
            }
            let events = stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|event| (event, receiver))
            });
            let stopped = stream::once(async move {
                if timed_out.load(Ordering::Acquire) {
                    Some(Event::Error {
                        text: "MySQL query timed out".into(),
                        line: None,
                        code: None,
                    })
                } else {
                    (!changes.handled.load(Ordering::Acquire)).then(closed)
                }
            })
            .filter_map(future::ready);
            events.chain(stopped).boxed()
        })
        .flatten();
        Run::new(source, cancel)
    }
}

async fn worker(mut conn: Conn, options: Opts, mut requests: mpsc::Receiver<Work>, timeout: u32) {
    while let Some(work) = requests.recv().await {
        let request = match work {
            Work::Run(request) => request,
            Work::Save { script, mut reply } => {
                if reply.is_closed() {
                    continue;
                }
                let result =
                    grid_edit::save(&mut conn, &options, &script, &mut reply, timeout).await;
                let failed = matches!(&result, Err(crate::grid_edit::Error::Recovery { .. }));
                let _ = reply.send(result);
                if failed {
                    break;
                }
                continue;
            }
        };
        if request.cancel.is_cancelled() {
            request
                .database_changes
                .handled
                .store(true, Ordering::Release);
            continue;
        }
        let id = conn.id();
        let operation = async {
            match request.mode {
                Some(mode) => plans::run(&mut conn, &request, mode).await,
                None => execution::run(&mut conn, &request).await,
            }
        };
        tokio::pin!(operation);
        let deadline = async {
            if timeout == 0 {
                future::pending::<()>().await;
            }
            tokio::time::sleep(Duration::from_secs(timeout.into())).await;
        };
        let completed = tokio::select! {
            biased;
            result = &mut operation => Some(result),
            _ = request.cancel.cancelled() => None,
            _ = deadline => {
                // Report on stream exhaustion so a full channel cannot delay KILL QUERY.
                request.timed_out.store(true, Ordering::Release);
                None
            }
        };
        let result = if let Some(result) = completed {
            result
        } else {
            request.stopped.cancel();
            // Keep polling the original future: dropping an in-flight protocol read is unsafe.
            let cleanup = async {
                loop {
                    kill(&options, id).await?;
                    tokio::select! {
                        biased;
                        result = &mut operation => return result,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(3), cleanup)
                .await
                .unwrap_or(Err(()))
        };
        if result.is_err() {
            break;
        }
        request
            .database_changes
            .handled
            .store(true, Ordering::Release);
    }
    let _ = conn.disconnect().await;
}

async fn kill(options: &Opts, id: u32) -> Result<(), ()> {
    let deliver = async {
        let mut conn = Conn::new(options.clone()).await?;
        conn.query_drop(format!("KILL QUERY {id}")).await?;
        conn.disconnect().await
    };
    tokio::time::timeout(Duration::from_secs(1), deliver)
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

fn closed() -> Event {
    Event::Error {
        text: "MySQL connection is closed".into(),
        line: None,
        code: None,
    }
}
