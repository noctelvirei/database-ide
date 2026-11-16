//! Waits for the cancel connection to close before allowing another query on the session.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::future;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

/// Retains only the endpoint, TLS policy and opaque cancellation capability.
pub(super) struct Cancellation {
    token: tokio_postgres::CancelToken,
    address: SocketAddr,
    hostname: String,
    tls: native_tls::TlsConnector,
}

impl std::fmt::Debug for Cancellation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cancellation").finish_non_exhaustive()
    }
}

impl Cancellation {
    /// Uses the session's resolved endpoint and TLS policy, with no DNS race on cancellation.
    pub(super) fn new(
        client: &tokio_postgres::Client,
        address: SocketAddr,
        hostname: String,
        tls: native_tls::TlsConnector,
    ) -> Self {
        Self {
            token: client.cancel_token(),
            address,
            hostname,
            tls,
        }
    }

    /// Bounds delivery and server closure to one second; an error requires socket disposal.
    pub(super) async fn send(&self) -> Result<(), ()> {
        tokio::time::timeout(Duration::from_secs(1), self.send_inner())
            .await
            .map_err(|_| ())?
    }

    async fn send_inner(&self) -> Result<(), ()> {
        let socket = TcpStream::connect(self.address).await.map_err(|_| ())?;
        let (reader, writer) = socket.into_split();
        let reader = Arc::new(Mutex::new(reader));
        let socket = CancelSocket {
            reader: reader.clone(),
            writer,
        };
        let tls = postgres_native_tls::TlsConnector::new(self.tls.clone(), &self.hostname);
        self.token
            .cancel_query_raw(socket, tls)
            .await
            .map_err(|_| ())?;
        // The fork shuts down its write half without reading EOF. Retain the read half
        // across that call so cancellation cannot arrive during the next statement.
        let mut bytes = [0; 256];
        loop {
            let mut buffer = ReadBuf::new(&mut bytes);
            future::poll_fn(|cx| {
                Pin::new(&mut *reader.lock().expect("cancel read lock is not poisoned"))
                    .poll_read(cx, &mut buffer)
            })
            .await
            .map_err(|_| ())?;
            if buffer.filled().is_empty() {
                return Ok(());
            }
        }
    }
}

struct CancelSocket {
    reader: Arc<Mutex<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
}

impl AsyncRead for CancelSocket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(
            &mut *self
                .reader
                .lock()
                .expect("cancel read lock is not poisoned"),
        )
        .poll_read(cx, buffer)
    }
}

impl AsyncWrite for CancelSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.writer).poll_write(cx, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.writer).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}
