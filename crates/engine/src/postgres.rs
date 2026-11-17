//! PostgreSQL simple-query sessions, ordered notices, cancellation and execution plans.

use std::error::Error as _;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::{StreamExt, future, stream};
use tokio::sync::{mpsc, oneshot};
use tokio_postgres::{AsyncMessage, Client, Config};
use tokio_util::sync::CancellationToken;

use crate::connections::{Connection, SignInMethod, TlsMode};
use crate::danger::Engine;
use crate::session::{ConnectError, ConnectErrorKind, Event, PlanMode, Run, Session};

mod cancel;
mod execution;
mod grid_edit;
mod metadata;
mod plans;
mod values;

/// Connection, parser and server failures, without connection strings or passwords.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The saved details name another engine or omit a database login.
    #[error("PostgreSQL requires a PostgreSQL connection and a user name")]
    InvalidConnection,
    /// Native certificate-store setup failed.
    #[error("PostgreSQL TLS: {0}")]
    Tls(#[from] native_tls::Error),
    /// The server's diagnostic, or the driver's transport diagnostic.
    #[error("PostgreSQL: {0}")]
    Server(String),
    /// The PostgreSQL dialect could not parse a plan request.
    #[error("Cannot parse PostgreSQL batch for plans: {0}")]
    Parse(String),
    /// A plan cannot be isolated without touching a caller-owned transaction.
    #[error("Actual plans require a session outside a transaction")]
    ActiveTransaction,
    /// A flagged statement did not identify an UPDATE or DELETE in this batch.
    #[error("Danger estimate requires an UPDATE or DELETE at the flagged line")]
    EstimateStatement,
    /// Cancellation delivery failed; the transport is discarded before another run.
    #[error("Could not deliver cancellation; connection discarded")]
    CancelFailed,
    /// Socket creation or endpoint inspection failed.
    #[error("PostgreSQL connection: {0}")]
    Io(#[from] std::io::Error),
    /// The saved connect timeout expired before sign-in completed.
    #[error("PostgreSQL connection timed out")]
    ConnectTimeout,
}

impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Server(error.as_db_error().map_or_else(
            || {
                error
                    .source()
                    .map_or_else(|| error.to_string(), ToString::to_string)
            },
            |e| e.message().into(),
        ))
    }
}

/// One authenticated session. Runs are serialized through protocol cleanup,
/// including when a consumer drops a Run. Debug never includes credentials.
pub struct Postgres {
    database: String,
    details: Connection,
    requests: mpsc::Sender<Work>,
}

enum Work {
    Run(Request),
    Save {
        script: crate::grid_edit::Script,
        reply: oneshot::Sender<Result<crate::grid_edit::SaveReport, crate::grid_edit::Error>>,
    },
}

impl fmt::Debug for Postgres {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Postgres")
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

struct Request {
    batch: String,
    mode: Option<PlanMode>,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
    /// Set by the worker once it has handled the request; a run whose events end
    /// without it saw the worker stop (closed connection or panic).
    handled: Arc<AtomicBool>,
}

struct Notice {
    error: tokio_postgres::error::DbError,
    acknowledged: oneshot::Sender<()>,
}

struct ConnectionTask(tokio::task::JoinHandle<()>);

impl Drop for ConnectionTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Connect using saved, password-free details and a separately supplied password.
/// Uses the native certificate store; Optional TLS permits plaintext fallback.
/// Windows sign-in uses the current logon session and ignores the supplied password.
pub async fn connect(
    details: &Connection,
    password: Option<&str>,
) -> Result<Postgres, ConnectError> {
    let invalid = || {
        ConnectError::new(
            ConnectErrorKind::InvalidDetails,
            details,
            password,
            &Error::InvalidConnection.to_string(),
        )
    };
    let windows_sign_in = details.sign_in == SignInMethod::Windows;
    // shortcut: USERNAME assumes an unchanged Windows environment; use the account API if needed.
    let windows_user = windows_sign_in
        .then(|| std::env::var("USERNAME").ok())
        .flatten();
    let user = details
        .user
        .as_deref()
        .or(windows_user.as_deref())
        .filter(|name| !name.is_empty())
        .ok_or_else(invalid)?;
    if details.engine != Engine::PostgreSql {
        return Err(invalid());
    }
    let mut config = Config::new();
    config
        .host(&details.host)
        .port(details.port.unwrap_or(5432))
        .user(user)
        .dbname(details.database.as_deref().unwrap_or(user))
        .application_name(&details.application_name)
        .ssl_mode(match details.tls {
            TlsMode::Mandatory => tokio_postgres::config::SslMode::Require,
            TlsMode::Optional => tokio_postgres::config::SslMode::Prefer,
            TlsMode::Off => tokio_postgres::config::SslMode::Disable,
        });
    if details.connect_timeout != 0 {
        config.connect_timeout(Duration::from_secs(details.connect_timeout.into()));
    }
    if let Some(password) = password.filter(|_| !windows_sign_in) {
        config.password(password);
    }
    let mut builder = native_tls::TlsConnector::builder();
    builder
        .danger_accept_invalid_certs(details.trust_server_certificate)
        .danger_accept_invalid_hostnames(details.trust_server_certificate);
    let tls = builder.build().map_err(|error| {
        ConnectError::new(
            ConnectErrorKind::Other,
            details,
            password,
            &error.to_string(),
        )
    })?;
    let establish = async {
        let addresses =
            tokio::net::lookup_host((details.host.as_str(), details.port.unwrap_or(5432)))
                .await
                .map_err(|error| ConnectError::io(&error, details, password))?;
        let mut error = ConnectError::io(
            &std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "host resolved to no addresses",
            ),
            details,
            password,
        );
        for address in addresses {
            // connect_raw discards the host; hostaddr pins cancellation without changing the SPN.
            let mut attempt = config.clone();
            attempt.hostaddr(address.ip());
            match attempt
                .connect(postgres_native_tls::MakeTlsConnector::new(tls.clone()))
                .await
            {
                Ok((client, connection)) => return Ok((client, connection, address)),
                Err(cause)
                    if cause
                        .source()
                        .is_some_and(|source| source.is::<std::io::Error>()) =>
                {
                    error = connect_error(&cause, details, password);
                }
                Err(cause) => {
                    return Err(connect_error(&cause, details, password));
                }
            }
        }
        Err(error)
    };
    let (client, mut connection, address) = if details.connect_timeout == 0 {
        establish.await
    } else {
        tokio::time::timeout(
            Duration::from_secs(details.connect_timeout.into()),
            establish,
        )
        .await
        .map_err(|_| ConnectError::new(ConnectErrorKind::Timeout, details, password, ""))?
    }?;
    let cancellation =
        cancel::Cancellation::new(&client, address, details.host.clone(), tls.clone());
    let (notices_tx, mut notices) = mpsc::channel(1);
    let background = ConnectionTask(tokio::spawn(async move {
        while let Some(message) = future::poll_fn(|cx| connection.poll_message(cx)).await {
            match message {
                Ok(AsyncMessage::Notice(error)) => {
                    // Stop reading at each notice until preceding query messages are consumed.
                    let (acknowledged, ack) = oneshot::channel();
                    if notices_tx
                        .send(Notice {
                            error,
                            acknowledged,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                    if ack.await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
                _ => {}
            }
        }
    }));
    let setup = format!(
        "SET DateStyle = 'ISO, YMD'; SET bytea_output = 'hex'; SET statement_timeout = {}",
        u64::from(details.query_timeout) * 1000
    );
    idle_query(&client, &mut notices, &setup)
        .await
        .map_err(|error| {
            ConnectError::new(
                ConnectErrorKind::Other,
                details,
                password,
                &error.to_string(),
            )
        })?;
    let database = details.database.as_deref().unwrap_or(user).to_owned();
    let (requests, receiver) = mpsc::channel(1);
    tokio::spawn(worker(
        client,
        notices,
        receiver,
        cancellation,
        background,
        metadata::Metadata::new(config, tls),
    ));
    Ok(Postgres {
        database,
        details: details.clone(),
        requests,
    })
}

fn connect_error(
    error: &tokio_postgres::Error,
    details: &Connection,
    password: Option<&str>,
) -> ConnectError {
    let windows = details.sign_in == SignInMethod::Windows;
    let kind = match error.code().map(|code| code.code()) {
        Some("28P01" | "28000") if windows => ConnectErrorKind::WindowsSignInFailed,
        Some("28P01" | "28000") => ConnectErrorKind::BadCredentials,
        Some("3D000" | "42501") => ConnectErrorKind::DatabaseUnavailable,
        _ => {
            if let Some(io) = error
                .source()
                .and_then(|source| source.downcast_ref::<std::io::Error>())
            {
                return ConnectError::io(io, details, password);
            }
            // These driver categories have no public structured accessor.
            if error.to_string() == "timeout waiting for server" {
                ConnectErrorKind::Timeout
            } else if error.source().is_some_and(|source| {
                source.is::<native_tls::Error>() && crate::session::certificate_failure(source)
            }) {
                ConnectErrorKind::Tls
            // tokio-postgres keeps its authentication category private, exposing only Display.
            } else if windows
                && (error.to_string() == "authentication error"
                    || error
                        .source()
                        .is_some_and(|source| source.to_string() == "password missing"))
            {
                ConnectErrorKind::WindowsSignInFailed
            } else {
                ConnectErrorKind::Other
            }
        }
    };
    let diagnostic = error.as_db_error().map_or_else(
        || {
            error
                .source()
                .map_or_else(|| error.to_string(), |source| format!("{error}: {source}"))
        },
        |db| db.message().into(),
    );
    ConnectError::new(kind, details, password, &diagnostic)
}

impl Postgres {
    fn request<'a>(&'a mut self, batch: &str, mode: Option<PlanMode>) -> Run<'a> {
        let token = CancellationToken::new();
        let (events, receiver) = mpsc::channel(16);
        let handled = Arc::new(AtomicBool::new(false));
        let request = Request {
            batch: batch.into(),
            mode,
            events,
            cancel: token.clone(),
            handled: handled.clone(),
        };
        let requests = self.requests.clone();
        let source = stream::once(async move {
            if requests.send(Work::Run(request)).await.is_err() {
                return stream::once(async { closed() }).boxed();
            }
            let events = stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|event| (event, receiver))
            });
            // Checked after the channel closes: the worker sets it before dropping the sender.
            let stopped =
                stream::once(async move { (!handled.load(Ordering::Acquire)).then(closed) })
                    .filter_map(future::ready);
            events.chain(stopped).boxed()
        })
        .flatten();
        Run::new(source, token)
    }

    /// Runs a confirmed PostgreSQL grid script on this tab's session. Each statement must
    /// affect one row; a conflict rolls back the whole Save. An existing user transaction
    /// is protected by a savepoint and remains open. Dropping this future requests rollback;
    /// cleanup finishes before the next session operation (a commit already sent may finish).
    pub async fn save(
        &mut self,
        script: crate::grid_edit::Script,
    ) -> Result<crate::grid_edit::SaveReport, crate::grid_edit::Error> {
        if script.engine != Engine::PostgreSql {
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

impl Session for Postgres {
    fn switch_database<'a>(
        &'a mut self,
        database: &'a str,
        password: Option<&'a str>,
    ) -> futures::future::BoxFuture<'a, Result<(), crate::session::Error>> {
        Box::pin(async move {
            let failure = |message: String| crate::session::Error::SwitchDatabase {
                database: database.into(),
                message,
            };
            crate::sql::identifier(database, Engine::PostgreSql)
                .map_err(|error| failure(error.to_string()))?;
            let mut details = self.details.clone();
            // dbname is a protocol value, not SQL: pass the original name without quoting.
            details.database = Some(database.into());
            let replacement = connect(&details, password)
                .await
                .map_err(|error| failure(error.to_string()))?;
            *self = replacement;
            Ok(())
        })
    }

    fn current_database(&self) -> &str {
        &self.database
    }

    fn run<'a>(&'a mut self, batch: &'a str) -> Run<'a> {
        self.request(batch, None)
    }

    fn plan<'a>(&'a mut self, batch: &'a str, mode: PlanMode) -> Run<'a> {
        self.request(batch, Some(mode))
    }
}

async fn worker(
    client: Client,
    mut notices: mpsc::Receiver<Notice>,
    mut requests: mpsc::Receiver<Work>,
    cancellation: cancel::Cancellation,
    _background: ConnectionTask,
    mut metadata: metadata::Metadata,
) {
    loop {
        tokio::select! {
            biased;
            notice = notices.recv(), if !notices.is_closed() => {
                if let Some(notice) = notice { let _ = notice.acknowledged.send(()); }
            }
            request = requests.recv() => {
                let Some(request) = request else { break; };
                let request = match request {
                    Work::Save { script, reply } => {
                        if reply.is_closed() { continue; }
                        let result = grid_edit::save(&client, &mut notices, &script, &reply).await;
                        let failed = client.is_closed()
                            || matches!(&result, Err(crate::grid_edit::Error::Recovery { .. }));
                        let _ = reply.send(result);
                        if failed { break; }
                        continue;
                    }
                    Work::Run(request) => request,
                };
                if request.cancel.is_cancelled() {
                    request.handled.store(true, Ordering::Release);
                    continue;
                }
                let failed = if let Some(mode) = request.mode {
                    plans::run(&client, &mut notices, &cancellation, &request, mode).await.is_err()
                } else {
                    execution::run(&client, &mut notices, &cancellation, &request, &request.batch, Some(&mut metadata)).await.is_err()
                };
                if failed { break; }
                request.handled.store(true, Ordering::Release);
            }
        }
    }
    // Requests still queued are dropped unhandled here, so their runs end with `closed()`.
}

/// The error a run reports when the session's connection is gone.
fn closed() -> Event {
    Event::Error {
        text: "PostgreSQL connection is closed".into(),
        line: None,
        code: None,
    }
}

// Internal housekeeping never blocks the connection's notice acknowledgement barrier.
async fn idle_query(
    client: &Client,
    notices: &mut mpsc::Receiver<Notice>,
    sql: &str,
) -> Result<(), tokio_postgres::Error> {
    let query = client.batch_execute(sql);
    tokio::pin!(query);
    loop {
        tokio::select! {
            biased;
            result = &mut query => return result,
            Some(notice) = notices.recv() => { let _ = notice.acknowledged.send(()); }
        }
    }
}

/// Estimates flagged UPDATE/DELETE rows in finding order without executing those statements.
/// Other danger kinds, and an UPDATE/DELETE inside a CTE, return None; rows come from
/// the top estimated-plan node after adding RETURNING 1 where absent (PostgreSQL
/// otherwise reports zero for a ModifyTable root).
pub async fn estimate_rows(
    session: &mut Postgres,
    batch: &str,
    flagged: &[crate::danger::DangerousStatement],
) -> Result<Vec<Option<f64>>, Error> {
    plans::estimate_rows(session, batch, flagged).await
}
