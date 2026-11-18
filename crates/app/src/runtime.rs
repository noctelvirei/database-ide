//! The tokio runtime the engine's drivers run on, beside gpui's own executor. The app makes
//! one at startup and never connects until the user asks.

use std::future::Future;
use std::sync::Arc;

use dbide_engine::connections::Connection;
use dbide_engine::session::{self, ConnectError, ConnectErrorKind, Session};

/// A cheap handle to the app's one runtime.
#[derive(Clone)]
pub struct Runtime(Arc<tokio::runtime::Runtime>);

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
        let (sender, receiver) = futures::channel::oneshot::channel();
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
}
