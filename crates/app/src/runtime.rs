//! The tokio runtime the engine's drivers run on, beside gpui's own executor. The app makes
//! one at startup and never connects until the user asks.
//!
//! A session moves onto the runtime for each piece of work (a run, a database switch, a
//! database list) and comes back when the work ends, so the UI thread never blocks on the
//! network and never holds a session another task is using.
//!
//! A run's rows never cross to the UI thread: the runtime writes each result set to a spill
//! file as it arrives (docs/spec.md, "Results: spill to disk") and the grid reads pages back
//! from it. So a slow UI never holds up the server, and memory stays flat however many rows
//! come back.

use std::future::Future;
use std::sync::Arc;

use dbide_engine::connections::Connection;
use dbide_engine::danger::Engine;
use dbide_engine::objects::{self, Index, Kind, Object, Script};
use dbide_engine::session::{self, CancelHandle, ConnectError, ConnectErrorKind, Event, Session};
use dbide_engine::spill::{Reader, Store};
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::{SinkExt as _, StreamExt as _};

/// A cheap handle to the app's one runtime.
#[derive(Clone)]
pub struct Runtime(Arc<tokio::runtime::Runtime>);

/// What a run reports to the UI, in order: `Started` once, the run's result sets and other
/// events as they come, then `Finished` with the session back.
pub enum RunUpdate {
    /// The run has begun; the handle cancels it from any thread.
    Started(CancelHandle),
    /// A result set has started. Its rows appear in the reader as they arrive, and it is
    /// complete once the next result set starts or the run finishes.
    ResultSet(Reader),
    /// One engine event other than a result set or its rows.
    Event(Event),
    /// The run is over: the session is free for the next one, and `results` owns the
    /// run's spill files, in result order. Dropping a store deletes its file and ends its
    /// readers, so the UI keeps them as long as it shows the results.
    Finished {
        session: Box<dyn Session>,
        results: Vec<Store>,
    },
}

/// One catalog call the object explorer makes on a session (docs/spec.md, "Object explorer
/// and Script as"). Every call but `Databases` reads the session's current database.
#[derive(Clone, Debug)]
pub enum Explore {
    /// The server's user databases.
    Databases,
    /// The tables, views, procedures and functions in every user schema.
    Objects,
    /// A table's indexes.
    Indexes(Object),
    /// An object's script in `form`, for a new query tab.
    Script(Object, Script),
}

/// What an `Explore` call returns, in the same order as its variants.
pub enum Explored {
    Databases(Vec<String>),
    Objects(Vec<Object>),
    Indexes(Vec<Index>),
    Script(String),
}

/// The object kinds the explorer lists under a database.
const LISTED_KINDS: [Kind; 5] = [
    Kind::Table,
    Kind::View,
    Kind::MaterializedView,
    Kind::Procedure,
    Kind::Function,
];

/// Calls the catalog function `name` of `engine`'s objects module with `args`, turning its
/// error into the text the explorer shows.
macro_rules! catalog {
    ($engine:expr, $name:ident($($arg:expr),*)) => {
        match $engine {
            Engine::SqlServer => objects::mssql::$name($($arg),*).await.map_err(|e| e.to_string()),
            Engine::PostgreSql => objects::postgres::$name($($arg),*).await.map_err(|e| e.to_string()),
            Engine::MySql => objects::mysql::$name($($arg),*).await.map_err(|e| e.to_string()),
        }
    };
}

/// Runs one explorer call on `session`.
async fn explore(
    session: &mut dyn Session,
    engine: Engine,
    request: Explore,
) -> Result<Explored, String> {
    Ok(match request {
        Explore::Databases => Explored::Databases(
            catalog!(engine, databases(session))?
                .into_iter()
                .map(|database| database.name)
                .collect(),
        ),
        Explore::Objects => {
            let mut listed = Vec::new();
            for schema in catalog!(engine, schemas(session))? {
                if schema.system {
                    continue;
                }
                let objects = catalog!(engine, objects(session, &schema.name))?;
                listed.extend(
                    objects
                        .into_iter()
                        .filter(|o| !o.system && LISTED_KINDS.contains(&o.kind)),
                );
            }
            Explored::Objects(listed)
        }
        Explore::Indexes(table) => Explored::Indexes(catalog!(engine, indexes(session, &table))?),
        Explore::Script(object, form) => {
            Explored::Script(catalog!(engine, script(session, &object, form))?)
        }
    })
}

/// How many updates may wait for the UI before the run pauses for it to catch up.
const RUN_UPDATES_BUFFERED: usize = 64;

impl Runtime {
    /// Two worker threads: connecting and streaming results are I/O-bound.
    pub fn new() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("engine")
            .enable_all()
            .build()?;
        Ok(Self(Arc::new(runtime)))
    }

    /// Connects on the runtime. The returned future can be awaited on gpui's executor.
    pub fn connect(
        &self,
        details: Connection,
        password: Option<String>,
    ) -> impl Future<Output = Result<Box<dyn Session>, ConnectError>> + use<> {
        let (sender, receiver) = oneshot::channel();
        self.0.spawn(async move {
            let outcome = session::connect(&details, password.as_deref()).await;
            // The receiver is gone if the window closed meanwhile; the session drops here.
            let _ = sender.send(outcome);
        });
        async move {
            receiver.await.unwrap_or_else(|_| {
                Err(ConnectError {
                    kind: ConnectErrorKind::Other,
                    message: "The app stopped before the connection finished.".into(),
                })
            })
        }
    }

    /// Closes a session on the runtime, where its driver's tasks live.
    pub fn close(&self, session: Box<dyn Session>) {
        self.0.spawn(async move { drop(session) });
    }

    /// Runs `batch` on `session`. The receiver yields the run's updates and ends after
    /// `Finished`; if the UI drops it, the run is cancelled, the session closed and the
    /// spill files deleted.
    pub fn run(&self, mut session: Box<dyn Session>, batch: String) -> mpsc::Receiver<RunUpdate> {
        let (mut updates, receiver) = mpsc::channel(RUN_UPDATES_BUFFERED);
        self.0.spawn(async move {
            let mut results = Results::default();
            {
                let mut run = session.run(&batch);
                if updates
                    .send(RunUpdate::Started(run.cancel_handle()))
                    .await
                    .is_err()
                {
                    return;
                }
                while let Some(event) = run.next().await {
                    let update = match event {
                        Event::ResultSetStarted { columns } => match results.start(columns) {
                            Ok(reader) => RunUpdate::ResultSet(reader),
                            Err(error) => RunUpdate::Event(error),
                        },
                        Event::Rows(rows) => match results.append(rows) {
                            Some(error) => RunUpdate::Event(error),
                            None => continue,
                        },
                        other => RunUpdate::Event(other),
                    };
                    // A UI that went away drops the run, which cancels it.
                    if updates.send(update).await.is_err() {
                        return;
                    }
                }
            }
            let results = results.finish();
            let _ = updates.send(RunUpdate::Finished { session, results }).await;
        });
        receiver
    }

    /// Switches `session` to `database`, returning the session (switched or not) and the
    /// outcome. PostgreSQL reconnects, so it needs the password again.
    pub fn switch_database(
        &self,
        session: Box<dyn Session>,
        database: String,
        password: Option<String>,
    ) -> impl Future<Output = (Box<dyn Session>, Result<(), String>)> + use<> {
        self.with_session(session, move |session| {
            Box::pin(async move {
                session
                    .switch_database(&database, password.as_deref())
                    .await
                    .map_err(|error| error.to_string())
            })
        })
    }

    /// The server's user databases, by name, with the session back.
    pub fn databases(
        &self,
        session: Box<dyn Session>,
        engine: Engine,
    ) -> impl Future<Output = (Box<dyn Session>, Result<Vec<String>, String>)> + use<> {
        let listing = self.explore(session, engine, Explore::Databases);
        async move {
            let (session, listed) = listing.await;
            let names = listed.map(|explored| match explored {
                Explored::Databases(names) => names,
                _ => unreachable!("a database list returns databases"),
            });
            (session, names)
        }
    }

    /// Makes one explorer catalog call on `session`, with the session back.
    pub fn explore(
        &self,
        session: Box<dyn Session>,
        engine: Engine,
        request: Explore,
    ) -> impl Future<Output = (Box<dyn Session>, Result<Explored, String>)> + use<> {
        self.with_session(session, move |session| {
            Box::pin(explore(session, engine, request))
        })
    }

    /// Lends `session` to `work` on the runtime and hands both back when it finishes. If the
    /// runtime stops first, the future never resolves; the app is closing then.
    fn with_session<T, W>(
        &self,
        mut session: Box<dyn Session>,
        work: W,
    ) -> impl Future<Output = (Box<dyn Session>, T)> + use<T, W>
    where
        T: Send + 'static,
        W: for<'a> FnOnce(&'a mut dyn Session) -> BoxFuture<'a, T> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        self.0.spawn(async move {
            let outcome = work(session.as_mut()).await;
            let _ = sender.send((session, outcome));
        });
        async move {
            match receiver.await {
                Ok(done) => done,
                Err(_) => std::future::pending().await,
            }
        }
    }
}

/// A run's spill files, written on the runtime as rows arrive.
#[derive(Default)]
struct Results {
    stores: Vec<Store>,
    /// Set once writing the current result set failed: its later rows are dropped, and
    /// the user has been told once.
    failed: bool,
}

impl Results {
    /// Completes the current result set and opens a file for the next. On failure, returns
    /// the error to show; that result set's rows are then dropped.
    fn start(&mut self, columns: Vec<dbide_engine::session::Column>) -> Result<Reader, Event> {
        self.complete_current();
        self.failed = false;
        let store = blocking(|| Store::new(columns, None)).map_err(|error| {
            self.failed = true;
            spill_error(&error)
        })?;
        let reader = store.reader();
        self.stores.push(store);
        Ok(reader)
    }

    /// Writes a batch of rows to the current result set. Returns an error to show the first
    /// time a write fails.
    fn append(&mut self, rows: Vec<Vec<dbide_engine::session::Value>>) -> Option<Event> {
        if self.failed {
            return None;
        }
        let store = self.stores.last_mut()?;
        let error = blocking(|| store.append(rows)).err()?;
        self.failed = true;
        Some(spill_error(&error))
    }

    /// Completes the last result set and hands every store over.
    fn finish(mut self) -> Vec<Store> {
        self.complete_current();
        self.stores
    }

    fn complete_current(&mut self) {
        if let Some(store) = self.stores.last_mut() {
            // A store that failed to write still shows the rows it kept.
            let _ = blocking(|| store.complete());
        }
    }
}

/// Spill reads and writes block on the disk; this lets the runtime move its other tasks
/// to another worker meanwhile.
fn blocking<T>(work: impl FnOnce() -> T) -> T {
    tokio::task::block_in_place(work)
}

/// A spill failure as an error in the messages strip. The run itself carries on.
fn spill_error(error: &dbide_engine::spill::Error) -> Event {
    Event::Error {
        text: format!("Couldn't keep this result's rows on disk: {error}"),
        line: None,
        code: None,
    }
}
