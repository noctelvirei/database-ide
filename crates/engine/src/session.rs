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
        // Serialize delivery with completion so a paused caller cannot cancel after Done.
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
