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
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::connections::{Connection, SignInMethod, TlsMode};
use crate::danger::Engine;
use crate::session::{ConnectError, ConnectErrorKind, Event, PlanMode, Run, Session};

pub(crate) mod batches;
mod execution;
mod grid_edit;
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
}

/// One serialized SQL Server session. Dropped runs finish attention cleanup before reuse.
pub struct Mssql {
    database: String,
    current: watch::Receiver<String>,
    requests: mpsc::Sender<Work>,
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

enum Work {
    Run(Request),
    Save {
        script: crate::grid_edit::Script,
        reply: oneshot::Sender<Result<crate::grid_edit::SaveReport, crate::grid_edit::Error>>,
    },
}

/// Opens SQL or current-account Windows sign-in. Named instances use SQL Browser (SSRP).
/// Mandatory/Optional TLS encrypt with TDS 7.4; Off permits the server to require encryption.
pub async fn connect(details: &Connection, password: Option<&str>) -> Result<Mssql, ConnectError> {
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
        return Err(ConnectError::new(
            ConnectErrorKind::InvalidDetails,
            details,
            password,
            &Error::InvalidConnection.to_string(),
        ));
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
        .map_err(|error| connect_error(&error, details, password))?;
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

fn connect_error(
    error: &mssql_tds::error::Error,
    details: &Connection,
    password: Option<&str>,
) -> ConnectError {
    use mssql_tds::error::Error as DriverError;
    let kind = match error {
        DriverError::SqlServerError { diagnostics } => {
            if diagnostics.errors.iter().any(|error| {
                matches!(error.number, 4060 | 4064 | 916)
                    || (error.number == 18456 && matches!(error.state, 38 | 46))
            }) {
                ConnectErrorKind::DatabaseUnavailable
            } else if diagnostics
                .errors
                .iter()
                .any(|error| matches!(error.number, 18456 | 18452))
            {
                if details.sign_in == SignInMethod::Windows {
                    ConnectErrorKind::WindowsSignInFailed
                } else {
                    ConnectErrorKind::BadCredentials
                }
            } else {
                ConnectErrorKind::Other
            }
        }
        DriverError::Security(_) if details.sign_in == SignInMethod::Windows => {
            ConnectErrorKind::WindowsSignInFailed
        }
        DriverError::Io(error) => return ConnectError::io(error, details, password),
        DriverError::ConnectionError(text) => {
            if let Some(code) = text
                .rsplit_once("(os error ")
                .and_then(|(_, suffix)| suffix.split_once(')'))
                .and_then(|(code, _)| code.parse::<i32>().ok())
            {
                let mapped =
                    ConnectError::io(&std::io::Error::from_raw_os_error(code), details, password);
                if mapped.kind != ConnectErrorKind::Other {
                    return mapped;
                }
            }
            if text.contains("SQL Server Browser did not respond within ")
                || text.contains("for the SQL Server Browser lookup did not complete within ")
            {
                return ConnectError::new(
                    ConnectErrorKind::Unreachable,
                    details,
                    password,
                    "SQL Server Browser didn't answer, so the instance's port is unknown",
                );
            }
            ConnectErrorKind::Other
        }
        DriverError::TimeoutError(_) => ConnectErrorKind::Timeout,
        DriverError::CertificateExpired | DriverError::CertificateMismatch => ConnectErrorKind::Tls,
        DriverError::TlsError(cause) if crate::session::certificate_failure(cause) => {
            ConnectErrorKind::Tls
        }
        DriverError::TlsHandshakeError { source, .. }
            if crate::session::certificate_failure(source.as_ref()) =>
        {
            ConnectErrorKind::Tls
        }
        _ => ConnectErrorKind::Other,
    };
    ConnectError::new(kind, details, password, &error.to_string())
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
            if requests.send(Work::Run(request)).await.is_err() {
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

    /// Saves a confirmed SQL Server grid script on this session, rolling back all Save
    /// changes on conflict. An existing transaction uses a savepoint and stays open unless
    /// a server error makes it uncommittable. Dropping the future requests rollback;
    /// worker cleanup finishes before reuse (a commit already sent may finish).
    pub async fn save(
        &mut self,
        script: crate::grid_edit::Script,
    ) -> Result<crate::grid_edit::SaveReport, crate::grid_edit::Error> {
        if script.engine != Engine::SqlServer {
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

impl Session for Mssql {
    fn switch_database<'a>(
        &'a mut self,
        database: &'a str,
        _password: Option<&'a str>,
    ) -> futures::future::BoxFuture<'a, Result<(), crate::session::Error>> {
        Box::pin(crate::session::use_database(
            self,
            database,
            Engine::SqlServer,
        ))
    }

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
    mut requests: mpsc::Receiver<Work>,
    current: watch::Sender<String>,
    timeout: u32,
) {
    while let Some(request) = requests.recv().await {
        let request = match request {
            Work::Save { script, mut reply } => {
                if reply.is_closed() {
                    continue;
                }
                let result = grid_edit::save(&mut client, &script, &mut reply, timeout).await;
                let failed = client.is_connection_dead()
                    || matches!(&result, Err(crate::grid_edit::Error::Recovery { .. }));
                let _ = reply.send(result);
                if failed {
                    break;
                }
                continue;
            }
            Work::Run(request) => request,
        };
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

#[cfg(test)]
mod connect_tests {
    use super::*;

    #[test]
    fn unanswered_browser_lookups_return_unreachable_with_the_instance_address() {
        let mut details = Connection::new(Engine::SqlServer, "127.0.0.1");
        details.instance = Some("dbide_missing".into());
        for diagnostic in [
            "Error Locating Server/Instance Specified [127.0.0.1\\dbide_missing]. Ensure the instance name is correct and SQL Server Browser service is running on the host. (Connection Error: SQL Server Browser did not respond within 1000ms. Verify that the SQL Server Browser service is running on '127.0.0.1:1434'.)",
            "Error Locating Server/Instance Specified [127.0.0.1\\dbide_missing]. Ensure the instance name is correct and SQL Server Browser service is running on the host. (Connection Error: Resolving '127.0.0.1' for the SQL Server Browser lookup did not complete within 1000ms.)",
        ] {
            let driver = mssql_tds::error::Error::ConnectionError(diagnostic.into());
            let error = connect_error(&driver, &details, None);
            assert_eq!(error.kind, ConnectErrorKind::Unreachable, "{error:?}");
            assert_eq!(
                error.message,
                "Can't reach 127.0.0.1\\dbide_missing: SQL Server Browser didn't answer, so the instance's port is unknown."
            );
        }
    }

    #[test]
    fn unclassified_parallel_connect_error_keeps_the_driver_diagnostic_without_the_password() {
        let details = Connection::new(Engine::SqlServer, "127.0.0.1");
        let driver = mssql_tds::error::Error::ConnectionError(
            "All parallel connection attempts failed: access denied (os error 10013); invented-secret".into(),
        );
        let error = connect_error(&driver, &details, Some("invented-secret"));
        assert_eq!(error.kind, ConnectErrorKind::Other);
        assert_eq!(
            error.message,
            "Connection Error: All parallel connection attempts failed: access denied (os error 10013); [redacted]"
        );
        assert!(!format!("{error:?}").contains("invented-secret"));
    }
}
