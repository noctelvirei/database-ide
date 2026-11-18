//! The tokio runtime the engine's drivers run on, beside gpui's own executor. The app makes
//! one at startup and never connects until the user asks.
//!
//! A session moves onto the runtime for each piece of work (a run, a database switch, a
//! database list) and comes back when the work ends, so the UI thread never blocks on the
//! network and never holds a session another task is using.

use std::future::Future;
use std::sync::Arc;

use dbide_engine::connections::Connection;
use dbide_engine::danger::Engine;
use dbide_engine::objects;
use dbide_engine::session::{self, CancelHandle, ConnectError, ConnectErrorKind, Event, Session};
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::{SinkExt as _, StreamExt as _};

/// A cheap handle to the app's one runtime.
#[derive(Clone)]
pub struct Runtime(Arc<tokio::runtime::Runtime>);

/// What a run reports to the UI, in order: `Started` once, the run's events, then
/// `Finished` with the session back.
pub enum RunUpdate {
    /// The run has begun; the handle cancels it from any thread.
    Started(CancelHandle),
    /// One engine event. Row batches stay on the runtime and never arrive here: U4 writes
    /// them to the spill store there.
    Event(Event),
    /// The run is over and the session is free for the next one.
    Finished(Box<dyn Session>),
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
    /// `Finished`; if the UI drops it, the run is cancelled and the session closed.
    pub fn run(&self, mut session: Box<dyn Session>, batch: String) -> mpsc::Receiver<RunUpdate> {
        let (mut updates, receiver) = mpsc::channel(RUN_UPDATES_BUFFERED);
        self.0.spawn(async move {
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
                    if matches!(event, Event::Rows(_)) {
                        continue;
                    }
                    // A UI that went away drops the run, which cancels it.
                    if updates.send(RunUpdate::Event(event)).await.is_err() {
                        return;
                    }
                }
            }
            let _ = updates.send(RunUpdate::Finished(session)).await;
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
        self.with_session(session, move |session| {
            Box::pin(async move {
                let listed = match engine {
                    Engine::SqlServer => objects::mssql::databases(session)
                        .await
                        .map_err(|e| e.to_string()),
                    Engine::PostgreSql => objects::postgres::databases(session)
                        .await
                        .map_err(|e| e.to_string()),
                    Engine::MySql => objects::mysql::databases(session)
                        .await
                        .map_err(|e| e.to_string()),
                };
                listed.map(|objects| objects.into_iter().map(|o| o.name).collect())
            })
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
