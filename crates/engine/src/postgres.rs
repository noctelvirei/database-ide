//! PostgreSQL simple-query sessions, ordered notices, cancellation and execution plans.

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
use crate::session::{Event, PlanMode, Run, Session};

mod cancel;
mod execution;
mod plans;
mod values;

/// Connection, parser and server failures, without connection strings or passwords.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The saved details name another engine or omit a database login.
    #[error("PostgreSQL requires a PostgreSQL connection and a user name")]
    InvalidConnection,
    /// Domain sign-in is reserved for DAT-8.
    #[error("PostgreSQL Windows sign-in is not supported yet")]
    WindowsSignIn,
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
        Self::Server(
            error
                .as_db_error()
                .map_or_else(|| error.to_string(), |e| e.message().into()),
        )
    }
}

/// One password-authenticated session. Runs are serialized through protocol cleanup,
/// including when a consumer drops a Run. Debug never includes credentials.
pub struct Postgres {
    database: String,
    requests: mpsc::Sender<Request>,
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
pub async fn connect(details: &Connection, password: Option<&str>) -> Result<Postgres, Error> {
    if details.sign_in == SignInMethod::Windows {
        return Err(Error::WindowsSignIn);
    }
    let user = details
        .user
        .as_deref()
        .filter(|name| !name.is_empty())
        .ok_or(Error::InvalidConnection)?;
    if details.engine != Engine::PostgreSql {
        return Err(Error::InvalidConnection);
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
    if let Some(password) = password {
        config.password(password);
    }
    let mut builder = native_tls::TlsConnector::builder();
    builder
        .danger_accept_invalid_certs(details.trust_server_certificate)
        .danger_accept_invalid_hostnames(details.trust_server_certificate);
    let tls = builder.build()?;
    let establish = async {
        let socket =
            tokio::net::TcpStream::connect((details.host.as_str(), details.port.unwrap_or(5432)))
                .await?;
        let address = socket.peer_addr()?;
        let connector = postgres_native_tls::TlsConnector::new(tls.clone(), &details.host);
        let (client, connection) = config.connect_raw(socket, connector).await?;
        Ok::<_, Error>((client, connection, address))
    };
    let (client, mut connection, address) = if details.connect_timeout == 0 {
        establish.await?
    } else {
        tokio::time::timeout(
            Duration::from_secs(details.connect_timeout.into()),
            establish,
        )
        .await
        .map_err(|_| Error::ConnectTimeout)??
    };
    let cancellation = cancel::Cancellation::new(&client, address, details.host.clone(), tls);
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
    idle_query(&client, &mut notices, &setup).await?;
    let database = details.database.as_deref().unwrap_or(user).to_owned();
    let (requests, receiver) = mpsc::channel(1);
    tokio::spawn(worker(client, notices, receiver, cancellation, background));
    Ok(Postgres { database, requests })
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
            if requests.send(request).await.is_err() {
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
}

impl Session for Postgres {
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
    mut requests: mpsc::Receiver<Request>,
    cancellation: cancel::Cancellation,
    _background: ConnectionTask,
) {
    loop {
        tokio::select! {
            biased;
            notice = notices.recv(), if !notices.is_closed() => {
                if let Some(notice) = notice { let _ = notice.acknowledged.send(()); }
            }
            request = requests.recv() => {
                let Some(request) = request else { break; };
                if request.cancel.is_cancelled() {
                    request.handled.store(true, Ordering::Release);
                    continue;
                }
                let failed = if let Some(mode) = request.mode {
                    plans::run(&client, &mut notices, &cancellation, &request, mode).await.is_err()
                } else {
                    execution::run(&client, &mut notices, &cancellation, &request, &request.batch, false).await.is_err()
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
