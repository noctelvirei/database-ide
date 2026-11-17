//! MySQL text-protocol sessions with bounded results and server-side cancellation.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::{StreamExt, future, stream};
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts, prelude::Queryable};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::connections::{Connection, SignInMethod, TlsMode};
use crate::danger::Engine;
use crate::session::{Event, PlanMode, Run, Session};

mod diagnostics;
mod execution;
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
    requests: mpsc::Sender<Request>,
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
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
    stopped: CancellationToken,
    database_changes: Arc<DatabaseChanges>,
    timed_out: Arc<AtomicBool>,
}

struct DatabaseChanges {
    statements: Vec<diagnostics::Statement>,
    // Immutable slots let current_database return &str while the worker records USE.
    names: Vec<OnceLock<String>>,
    handled: AtomicBool,
}

/// Connect with saved details and a separately supplied password. TLS uses the native
/// certificate store; Optional falls back only when the server has no TLS capability.
pub async fn connect(details: &Connection, password: Option<&str>) -> Result<MySql, Error> {
    if details.sign_in == SignInMethod::Windows {
        return Err(Error::WindowsSignIn);
    }
    let user = details
        .user
        .as_deref()
        .filter(|user| !user.is_empty())
        .ok_or(Error::InvalidConnection)?;
    if details.engine != Engine::MySql || details.instance.is_some() {
        return Err(Error::InvalidConnection);
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
        .map_err(|_| Error::ConnectTimeout)?
    }
    .map_err(|error| Error::Server(diagnostics::redact(&error.to_string(), password)))?;
    let (requests, receiver) = mpsc::channel(1);
    tokio::spawn(worker(conn, options, receiver, details.query_timeout));
    let database = details.database.clone().unwrap_or_default();
    Ok(MySql {
        database,
        database_changes: Vec::new(),
        requests,
    })
}

impl Session for MySql {
    fn current_database(&self) -> &str {
        self.database_changes
            .iter()
            .rev()
            .flat_map(|changes| changes.names.iter().rev())
            .find_map(OnceLock::get)
            .map_or(&self.database, String::as_str)
    }

    fn run<'a>(&'a mut self, batch: &'a str) -> Run<'a> {
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
            events,
            cancel: cancel.clone(),
            stopped: CancellationToken::new(),
            database_changes: changes.clone(),
            timed_out: timed_out.clone(),
        };
        let requests = self.requests.clone();
        let source = stream::once(async move {
            if requests.send(request).await.is_err() {
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

    fn plan<'a>(&'a mut self, _batch: &'a str, _mode: PlanMode) -> Run<'a> {
        Run::new(
            stream::once(async {
                Event::Error {
                    text: "Execution plans are not supported for MySQL".into(),
                    line: None,
                    code: None,
                }
            }),
            CancellationToken::new(),
        )
    }
}

async fn worker(
    mut conn: Conn,
    options: Opts,
    mut requests: mpsc::Receiver<Request>,
    timeout: u32,
) {
    while let Some(request) = requests.recv().await {
        if request.cancel.is_cancelled() {
            request
                .database_changes
                .handled
                .store(true, Ordering::Release);
            continue;
        }
        let id = conn.id();
        let operation = execution::run(&mut conn, &request);
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
