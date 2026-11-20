//! The common interface for one open connection and its ordered runs.
//!
//! `Session` is an object-safe trait: engines can be added without changing callers or a
//! central enum. A run borrows the connection exclusively until dropped. Sending SQL may
//! begin on the first poll; drivers must stream bounded row batches without buffering a run.
//! Engine-specific operations stay on the engine's own type.

use std::fmt;
use std::num::NonZeroU64;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Text representations validated without introducing a date/time dependency.
pub mod temporal;

/// The action a caller can offer after connection establishment fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectErrorKind {
    /// Ask for a different database user name or password.
    BadCredentials,
    /// The current Windows account could not sign in.
    WindowsSignInFailed,
    /// Check the address, server availability or network route.
    Unreachable,
    /// The saved connect timeout expired.
    Timeout,
    /// Certificate validation failed; verify the server before changing trust.
    Tls,
    /// Choose an existing database this login can open.
    /// Named databases: "Signed in, but database <name> doesn't exist or this login can't open it."
    /// SQL Server/MySQL without a database: "Signed in, but this login's default database
    /// doesn't exist or it can't open it."
    DatabaseUnavailable,
    /// Correct invalid or unsupported saved connection details.
    InvalidDetails,
    /// No more specific action is known; show the redacted diagnostic.
    Other,
}

/// A failed connection with an actionable category and credential-free display text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ConnectError {
    /// The category to branch on, independent of the server's wording.
    pub kind: ConnectErrorKind,
    /// Display unchanged; supplied credentials have been removed.
    pub message: String,
}

impl ConnectError {
    pub(crate) fn new(
        kind: ConnectErrorKind,
        details: &crate::connections::Connection,
        password: Option<&str>,
        diagnostic: &str,
    ) -> Self {
        use ConnectErrorKind::*;
        let mut message = match kind {
            BadCredentials => "Sign-in failed: the user name or password is wrong.".into(),
            WindowsSignInFailed => "Windows sign-in failed: the server didn't accept this Windows account.".into(),
            Unreachable => format!("Can't reach {}: {diagnostic}.", connect_address(details)),
            Timeout => format!("No answer from {} within {} s.", connect_address(details), details.connect_timeout),
            Tls => "The server's certificate isn't trusted. Turn on Trust server certificate only if you know this server.".into(),
            DatabaseUnavailable => {
                let database = details.database.as_deref().or(match details.engine {
                    crate::danger::Engine::PostgreSql => details.user.as_deref(),
                    _ => None,
                });
                match database {
                    Some(database) => format!("Signed in, but database {database} doesn't exist or this login can't open it."),
                    None => "Signed in, but this login's default database doesn't exist or it can't open it.".into(),
                }
            }
            InvalidDetails | Other => diagnostic.into(),
        };
        if let Some(password) = password.filter(|value| !value.is_empty()) {
            message = message.replace(password, "[redacted]");
        }
        Self { kind, message }
    }

    pub(crate) fn io(
        error: &std::io::Error,
        details: &crate::connections::Connection,
        password: Option<&str>,
    ) -> Self {
        use std::io::ErrorKind;
        let reason = match error.kind() {
            ErrorKind::ConnectionRefused => "the connection was refused",
            ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => {
                "there's no network route"
            }
            ErrorKind::NotFound | ErrorKind::AddrNotAvailable => "the host name wasn't found",
            ErrorKind::TimedOut => {
                return Self::new(ConnectErrorKind::Timeout, details, password, "");
            }
            _ if matches!(error.raw_os_error(), Some(11001 | 11004)) => {
                "the host name wasn't found"
            }
            _ => {
                return Self::new(
                    ConnectErrorKind::Other,
                    details,
                    password,
                    &error.to_string(),
                );
            }
        };
        Self::new(ConnectErrorKind::Unreachable, details, password, reason)
    }
}

fn connect_address(details: &crate::connections::Connection) -> String {
    if let Some(instance) = &details.instance {
        return format!("{}\\{instance}", details.host);
    }
    let default = match details.engine {
        crate::danger::Engine::SqlServer => 1433,
        crate::danger::Engine::PostgreSql => 5432,
        crate::danger::Engine::MySql => 3306,
    };
    format!("{}:{}", details.host, details.port.unwrap_or(default))
}

/// native-tls hides Schannel's status; mssql-tds also erases it on its direct TLS path.
/// Match only certificate diagnostics when no structured certificate error is exposed.
pub(crate) fn certificate_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(io) = error.downcast_ref::<std::io::Error>()
        && io.raw_os_error().is_some_and(|code| {
            matches!(
                code as u32,
                0x800B0101
                    | 0x800B0109
                    | 0x800B010A
                    | 0x800B010F
                    | 0x80090322
                    | 0x80090325
                    | 0x80090327
                    | 0x80090328
            )
        })
    {
        return true;
    }
    let text = error.to_string().to_ascii_lowercase();
    [
        "certificate verify failed",
        "certificate validation failed",
        "certificate chain",
        "cert_e_",
        "sec_e_untrusted_root",
        "sec_e_wrong_principal",
        "sec_e_cert_expired",
        "sec_e_cert_unknown",
        "80090325",
        "80090322",
        "80090328",
        "800b0109",
        "800b010f",
        "(os error -2146762495)",
        "(os error -2146762487)",
        "(os error -2146762486)",
        "(os error -2146762481)",
    ]
    .iter()
    .any(|marker| text.contains(marker))
        || error.source().is_some_and(certificate_failure)
}

/// Opens the saved connection with its engine and separately supplied password.
/// Returns a credential-free, actionable error if validation, transport or sign-in fails.
pub async fn connect(
    details: &crate::connections::Connection,
    password: Option<&str>,
) -> Result<Box<dyn Session>, ConnectError> {
    match details.engine {
        crate::danger::Engine::SqlServer => {
            Ok(Box::new(crate::mssql::connect(details, password).await?))
        }
        crate::danger::Engine::PostgreSql => {
            Ok(Box::new(crate::postgres::connect(details, password).await?))
        }
        crate::danger::Engine::MySql => {
            Ok(Box::new(crate::mysql::connect(details, password).await?))
        }
    }
}

/// A coarse formatting and alignment hint, independent of the server's type spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeCategory {
    /// Boolean values.
    Bool,
    /// Signed or unsigned whole numbers.
    Integer,
    /// Exact base-ten numbers.
    Decimal,
    /// Approximate floating-point numbers.
    Float,
    /// Character strings.
    Text,
    /// Binary strings.
    Bytes,
    /// Calendar dates.
    Date,
    /// Clock times, optionally carrying an offset.
    Time,
    /// Dates and times, optionally carrying an offset.
    Timestamp,
    /// UUIDs.
    Uuid,
    /// JSON documents.
    Json,
    /// A type whose server text should be displayed unchanged.
    Other,
}

/// Metadata for one result column, in the order its cells appear in every row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Result label, including any SQL alias; it need not be unique.
    pub name: String,
    /// The server's engine-specific type name, for display rather than parsing.
    pub type_name: String,
    /// The UI's engine-neutral formatting hint.
    pub category: TypeCategory,
    /// Whether NULL is possible; None means the server did not report it.
    pub nullable: Option<bool>,
    /// Server-reported base table, qualified when available; None for unknown provenance.
    pub base_table: Option<String>,
    /// Server-reported base column before aliasing; None for expressions or unknown provenance.
    pub base_column: Option<String>,
}

/// One cell, converted at the driver boundary without losing integer or decimal precision.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL NULL, independent of the column's declared type.
    Null,
    /// A logical value.
    Bool(bool),
    /// A signed integer, including the full BIGINT range.
    Integer(i64),
    /// An unsigned integer, including MySQL's full unsigned BIGINT range.
    UnsignedInteger(u64),
    /// Exact server decimal text; never converted through floating point.
    Decimal(String),
    /// An approximate number, including NaN and infinities when the engine supports them.
    Float(f64),
    /// Unicode text, unmodified by the UI.
    Text(String),
    /// Binary contents, without display encoding.
    Bytes(Vec<u8>),
    /// A validated ISO 8601 calendar date.
    Date(temporal::Date),
    /// A validated ISO 8601 clock time without an offset.
    Time(temporal::Time),
    /// A validated ISO 8601 clock time with a UTC offset (PostgreSQL timetz).
    TimeWithZone(temporal::TimeWithZone),
    /// A validated ISO 8601 date and time without an offset.
    Timestamp(temporal::Timestamp),
    /// A validated ISO 8601 date and time with a UTC offset; no zone rules are inferred.
    TimestampWithZone(temporal::TimestampWithZone),
    /// The UUID's 16 bytes in network order, independent of driver storage order.
    Uuid([u8; 16]),
    /// JSON text, preserved without parsing or normalizing it.
    Json(String),
    /// Server text for unsupported types or values outside the common representation.
    Other(String),
}

/// Message severity for display; native numeric severity is retained when reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Informational output such as PRINT or a notice.
    Info,
    /// A warning that does not itself end a statement.
    Warning,
    /// An engine-reported numeric severity, such as SQL Server RAISERROR's level.
    Server(u8),
}

/// The raw plan encoding; parsing is separate from running SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFormat {
    /// SQL Server showplan XML.
    ShowplanXml,
    /// PostgreSQL EXPLAIN with FORMAT JSON.
    ExplainJson,
    /// MySQL iterator EXPLAIN JSON (explain_json_format_version=2), with optional ANALYZE.
    MySqlJson,
}

/// Events in server order. Errors do not terminate the stream; exactly one Done does.
/// Rows belong to the most recent ResultSetStarted, with cells in column order.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Starts a result set, including one that subsequently returns no rows.
    ResultSetStarted {
        /// Column metadata in result order.
        columns: Vec<Column>,
    },
    /// The next bounded batch of complete rows; each row has one value per column.
    Rows(Vec<Vec<Value>>),
    /// Server output in its position among results and statement counts.
    Message {
        /// Text ready for display.
        text: String,
        /// Display severity or the server's numeric severity.
        severity: Severity,
        /// One-based line in the submitted batch, when reported.
        line: Option<NonZeroU64>,
    },
    /// Rows affected or returned by one statement, as reported by the server.
    RowCount(u64),
    /// A raw plan for a statement, without a parsed operator tree.
    Plan {
        /// Unmodified plan payload.
        payload: String,
        /// Encoding used by the payload.
        format: PlanFormat,
    },
    /// A run error; later statements may still return events.
    Error {
        /// Text ready for display; cancellation uses "Run was cancelled".
        text: String,
        /// One-based line in the submitted batch, when reported.
        line: Option<NonZeroU64>,
        /// Native server code or SQLSTATE; None for local errors and cancellation.
        code: Option<String>,
    },
    /// The final event, after which every poll returns None.
    Done {
        /// Monotonic wall time since Run construction, including time awaiting consumption.
        elapsed: Duration,
    },
}

/// Whether a plan asks the optimizer only or executes for runtime counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanMode {
    /// Ask the optimizer without executing the statement.
    Estimated,
    /// Execute for runtime counters; engines roll back statements that can change data.
    Actual,
}

/// A database switch failed; the diagnostic is ready to display in the UI.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The requested database could not be selected or connected to.
    #[error("Cannot switch to database {database:?}: {message}")]
    SwitchDatabase {
        /// The unquoted name supplied by the caller.
        database: String,
        /// The validation, connection or server diagnostic.
        message: String,
    },
}

/// One open connection with a current database; implementations own protocol recovery.
pub trait Session: fmt::Debug + Send {
    /// The connection's current database name, updated after database-changing batches.
    fn current_database(&self) -> &str;

    /// Switches to an unquoted database name, updating this session only on success.
    /// PostgreSQL reconnects with the original details and loses connection-local state;
    /// supply its password again when required. Other engines ignore the password.
    /// A refused switch returns a displayable error and leaves the old session usable.
    fn switch_database<'a>(
        &'a mut self,
        database: &'a str,
        password: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), Error>>;

    /// Starts one batch, borrowing the connection and SQL until the Run is dropped.
    /// All failures, including send failures, are Error events. Implementations must observe
    /// the run's cancellation token, abort server work, and drain or discard the connection
    /// before reuse. Dropping a Run requests cancellation but cannot await server cleanup.
    ///
    /// ```compile_fail
    /// use dbide_engine::session::Session;
    /// fn overlapping(session: &mut dyn Session) {
    ///     let first = session.run("SELECT 1");
    ///     let second = session.run("SELECT 2");
    ///     drop((first, second));
    /// }
    /// ```
    fn run<'a>(&'a mut self, batch: &'a str) -> Run<'a>;

    /// Returns one raw plan per statement in batch order. Parse and server failures are
    /// Error events. Actual plans execute SQL and can have nontransactional side effects.
    fn plan<'a>(&'a mut self, batch: &'a str, mode: PlanMode) -> Run<'a>;
}

pub(crate) async fn use_database(
    session: &mut dyn Session,
    database: &str,
    engine: crate::danger::Engine,
) -> Result<(), Error> {
    let failure = |message: String| Error::SwitchDatabase {
        database: database.into(),
        message,
    };
    let name =
        crate::sql::identifier(database, engine).map_err(|error| failure(error.to_string()))?;
    let batch = format!("USE {name}");
    let mut run = session.run(&batch);
    let mut error = None;
    while let Some(event) = run.next().await {
        if let Event::Error { text, .. } = event {
            error.get_or_insert_with(|| failure(text));
        }
    }
    error.map_or(Ok(()), Err)
}

/// An idempotent cancellation request, safe to clone and call from another task.
/// Cancellation racing completion wins only if observed before Done; after Done it is inert.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    token: CancellationToken,
    completed: Arc<Mutex<bool>>,
}

impl CancelHandle {
    /// Requests server cancellation and wakes a pending stream. Does not wait for server
    /// acknowledgement. Further events are a cancellation Error followed by Done.
    pub fn cancel(&self) {
        let completed = self
            .completed
            .lock()
            .expect("completion lock is not poisoned");
        if !*completed {
            self.token.cancel();
        }
    }
}

/// A Send event stream with one terminal Done and an independently usable cancel handle.
/// No task or unbounded queue is created; the consumer must drain it into the spill store.
/// Dropping before Done requests cancellation and releases the borrowed event source.
pub struct Run<'a> {
    source: Option<BoxStream<'a, Event>>,
    cancel: CancelHandle,
    cancelled: BoxFuture<'static, ()>,
    started: Instant,
}

impl fmt::Debug for Run<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Run")
            .field("cancel", &self.cancel)
            .finish_non_exhaustive()
    }
}

impl<'a> Run<'a> {
    /// Wraps ordered driver events and a fresh per-run token shared with server cancellation.
    /// Source exhaustion appends Done. An explicit source Done also ends the run; its elapsed
    /// field is replaced by the measured duration. Events after it are discarded.
    /// On cancellation the source is dropped; protocol cleanup must live in the session or
    /// a driver task, rather than depending on further polls of this source.
    pub fn new(source: impl Stream<Item = Event> + Send + 'a, token: CancellationToken) -> Self {
        Self {
            source: Some(source.boxed()),
            cancel: CancelHandle {
                token: token.clone(),
                completed: Arc::new(Mutex::new(false)),
            },
            cancelled: Box::pin(token.cancelled_owned()),
            started: Instant::now(),
        }
    }

    /// Returns a handle for this run only; later runs must use fresh tokens.
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    fn done(&mut self) -> Poll<Option<Event>> {
        *self
            .cancel
            .completed
            .lock()
            .expect("completion lock is not poisoned") = true;
        self.source = None;
        Poll::Ready(Some(Event::Done {
            elapsed: self.started.elapsed(),
        }))
    }
}

impl Stream for Run<'_> {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        let run = self.get_mut();
        if *run
            .cancel
            .completed
            .lock()
            .expect("completion lock is not poisoned")
        {
            return Poll::Ready(None);
        }
        if run.source.is_none() {
            return run.done();
        }
        if run.cancelled.as_mut().poll(cx).is_ready() {
            run.source = None;
            return Poll::Ready(Some(Event::Error {
                text: "Run was cancelled".into(),
                line: None,
                code: None,
            }));
        }
        match run
            .source
            .as_mut()
            .expect("source checked above")
            .poll_next_unpin(cx)
        {
            Poll::Ready(None | Some(Event::Done { .. })) => run.done(),
            event => event,
        }
    }
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod connect_tests {
    use super::certificate_failure;

    #[test]
    fn unavailable_default_database_uses_the_user_name_only_for_postgres() {
        use super::{ConnectError, ConnectErrorKind};
        use crate::connections::Connection;
        use crate::danger::Engine;

        for (engine, expected) in [
            (
                Engine::SqlServer,
                "Signed in, but this login's default database doesn't exist or it can't open it.",
            ),
            (
                Engine::MySql,
                "Signed in, but this login's default database doesn't exist or it can't open it.",
            ),
            (
                Engine::PostgreSql,
                "Signed in, but database dbide_test doesn't exist or this login can't open it.",
            ),
        ] {
            let mut details = Connection::new(engine, "127.0.0.1");
            details.user = Some("dbide_test".into());
            let error =
                ConnectError::new(ConnectErrorKind::DatabaseUnavailable, &details, None, "");
            assert_eq!(error.message, expected, "{engine:?}");
        }
    }

    #[test]
    fn expired_certificate_is_recognised_after_native_tls_erases_its_status() {
        let error = std::io::Error::other(
            std::io::Error::from_raw_os_error(0x800B0101_u32 as i32).to_string(),
        );
        assert!(certificate_failure(&error));
    }

    #[test]
    fn certificate_name_mismatch_is_recognised_after_native_tls_erases_its_status() {
        let error = std::io::Error::other(
            std::io::Error::from_raw_os_error(0x800B010F_u32 as i32).to_string(),
        );
        assert!(certificate_failure(&error));
    }

    #[test]
    fn tls_protocol_failure_is_not_a_certificate_failure() {
        let error = std::io::Error::other("TLS handshake failed: unsupported protocol");
        assert!(!certificate_failure(&error));
    }
}
