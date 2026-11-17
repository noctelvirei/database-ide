//! SQL Server plain-SQL sessions, streaming results, attention and showplan XML.

use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use futures::{StreamExt, future, stream};
use mssql_tds::connection::client_context::{ClientContext, TdsAuthenticationMethod};
use mssql_tds::connection::tds_client::TdsClient;
use mssql_tds::connection_provider::tds_connection_provider::TdsConnectionProvider;
use mssql_tds::core::EncryptionSetting;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::connections::{Connection, SignInMethod, TlsMode};
use crate::danger::Engine;
use crate::session::{Event, PlanMode, Run, Session};

mod batches;
mod execution;
mod plans;
mod values;

/// Connection and plan failures; credentials are never retained in diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Connection must name SQL Server and a valid address and sign-in method.
    #[error(
        "SQL Server requires a SQL Server connection, a host, and a user for SQL sign-in; port and instance are mutually exclusive"
    )]
    InvalidConnection,
    /// Transport or server diagnostic with credentials removed.
    #[error("SQL Server: {0}")]
    Server(String),
    /// A flagged statement could not be isolated for estimation.
    #[error("Cannot parse SQL Server batch for danger estimates: {0}")]
    Parse(String),
}

/// One serialized SQL Server session. Dropped runs finish attention cleanup before reuse.
pub struct Mssql {
    database: String,
    current: watch::Receiver<String>,
    requests: mpsc::Sender<Request>,
}

impl fmt::Debug for Mssql {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mssql")
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

struct Request {
    batch: String,
    mode: Option<PlanMode>,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
    handled: Arc<AtomicBool>,
}

/// Opens SQL or current-account Windows sign-in. Named instances use SQL Browser (SSRP).
/// Mandatory/Optional TLS encrypt with TDS 7.4; Off permits the server to require encryption.
pub async fn connect(details: &Connection, password: Option<&str>) -> Result<Mssql, Error> {
    if details.engine != Engine::SqlServer
        || details.host.trim().is_empty()
        || details.port == Some(0)
        || (details.port.is_some() && details.instance.is_some())
        || details
            .instance
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        || (details.sign_in != SignInMethod::Windows
            && details.user.as_ref().is_none_or(|name| name.is_empty()))
    {
        return Err(Error::InvalidConnection);
    }
    let address = if let Some(instance) = &details.instance {
        format!("tcp:{}\\{instance}", details.host)
    } else {
        format!("tcp:{},{}", details.host, details.port.unwrap_or(1433))
    };
    let mut context = ClientContext::with_data_source(&address);
    context.tds_authentication_method = if details.sign_in == SignInMethod::Windows {
        TdsAuthenticationMethod::SSPI
    } else {
        TdsAuthenticationMethod::Password
    };
    if details.sign_in != SignInMethod::Windows {
        context.user_name = details.user.clone().unwrap_or_default();
        context.password = password.unwrap_or_default().into();
    }
    context.database = details.database.clone().unwrap_or_default();
    context.application_name = details.application_name.clone();
    context.connect_timeout = details.connect_timeout;
    context.login_timeout = Some(details.connect_timeout);
    context.connect_retry_count = 0;
    context.encryption_options.mode = match details.tls {
        TlsMode::Off => EncryptionSetting::PreferOff,
        TlsMode::Mandatory | TlsMode::Optional => EncryptionSetting::On,
    };
    context.encryption_options.trust_server_certificate = details.trust_server_certificate;
    let client = TdsConnectionProvider {}
        .create_client(context, &address, None)
        .await
        .map_err(|error| Error::Server(redact(&error.to_string(), password)))?;
    let database = client.database().to_owned();
    let (current_tx, current) = watch::channel(database.clone());
    let (requests, receiver) = mpsc::channel(1);
    tokio::spawn(worker(client, receiver, current_tx, details.query_timeout));
    Ok(Mssql {
        database,
        current,
        requests,
    })
}

fn redact(text: &str, password: Option<&str>) -> String {
    match password.filter(|password| !password.is_empty()) {
        Some(password) => text.replace(password, "[redacted]"),
        None => text.into(),
    }
}

impl Mssql {
    fn request<'a>(&'a mut self, batch: &str, mode: Option<PlanMode>) -> Run<'a> {
        self.database = self.current.borrow().clone();
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
        let current = self.current.clone();
        let database = &mut self.database;
        let source = stream::once(async move {
            if requests.send(request).await.is_err() {
                return stream::once(async { closed() }).boxed();
            }
            let events = stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|event| (event, receiver))
            });
            let stopped = stream::once(async move {
                *database = current.borrow().clone();
                (!handled.load(Ordering::Acquire)).then(closed)
            })
            .filter_map(future::ready);
            events.chain(stopped).boxed()
        })
        .flatten();
        Run::new(source, token)
    }
}

impl Session for Mssql {
    fn current_database(&self) -> &str {
        &self.database
    }

    fn run<'a>(&'a mut self, batch: &'a str) -> Run<'a> {
        self.request(batch, None)
    }

    /// Actual plans execute the batch, including data changes, as in SSMS.
    fn plan<'a>(&'a mut self, batch: &'a str, mode: PlanMode) -> Run<'a> {
        self.request(batch, Some(mode))
    }
}

async fn worker(
    mut client: TdsClient,
    mut requests: mpsc::Receiver<Request>,
    current: watch::Sender<String>,
    timeout: u32,
) {
    while let Some(request) = requests.recv().await {
        if !request.cancel.is_cancelled() {
            let result = execution::run(&mut client, &request, timeout).await;
            current.send_replace(client.database().to_owned());
            if result.is_err() || client.is_connection_dead() {
                break;
            }
        }
        request.handled.store(true, Ordering::Release);
    }
    let _ = client.close_connection().await;
}

fn closed() -> Event {
    Event::Error {
        text: "SQL Server connection is closed".into(),
        line: None,
        code: None,
    }
}

/// Estimates each flagged UPDATE/DELETE without running it; other findings return None.
/// Ambiguous same-line statements are matched in source order.
pub async fn estimate_rows(
    session: &mut Mssql,
    batch: &str,
    flagged: &[crate::danger::DangerousStatement],
) -> Result<Vec<Option<f64>>, Error> {
    plans::estimate_rows(session, batch, flagged).await
}
