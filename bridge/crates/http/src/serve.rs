// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The accept loop. `axum::serve` sets no timer, so hyper's header read
//! timeout never runs and a client can hold a connection open forever by
//! sending its headers one byte at a time (slowloris). Here:
//!
//! - the first bytes (to tell HTTP/1 from HTTP/2) must arrive within the
//!   header timeout;
//! - HTTP/1: hyper's `header_read_timeout` for every request head, also
//!   the wait for the next request on a kept-alive connection;
//! - HTTP/2: keep-alive pings, so dead peers are dropped.
//!
//! Bodies have their own limits (sizes, and the per-route timeouts in
//! [`crate::Server::router`]). A response the client stops reading can't
//! hold its connection either: a socket that takes no bytes for
//! [`Timeouts::write_stall`] is closed.
//!
//! On shutdown the listener closes at once, open connections get
//! [`Timeouts::grace`] to finish, and whatever is left is dropped.

use std::io;
use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use bytes::{Buf as _, Bytes};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower::ServiceExt as _;

/// The accept loop's timers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeouts {
    /// How long a client has to send a request's headers.
    pub(crate) header: Duration,
    /// How long open connections may go on after shutdown starts.
    pub(crate) grace: Duration,
    /// How long a socket may refuse to take more bytes.
    pub(crate) write_stall: Duration,
}

impl Timeouts {
    /// The production values. The grace fits in the 30 s that systemd and
    /// Kubernetes give before killing (Docker gives 10 s unless
    /// `stop_grace_period` says otherwise).
    pub(crate) const DEFAULT: Self = Self {
        header: Duration::from_secs(15),
        grace: Duration::from_secs(20),
        write_stall: Duration::from_secs(60),
    };
}

/// Most requests at once on one HTTP/2 connection (hyper's default is 200).
const H2_MAX_STREAMS: u32 = 64;

/// HTTP/2 keep-alive pings.
const H2_KEEP_ALIVE: Duration = Duration::from_secs(20);

/// The HTTP/2 client connection preface (RFC 9113 §3.4).
const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Serves `router` until `shutdown`, then gives open connections
/// `timeouts.grace` to finish their requests and drops the rest.
pub(crate) async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: CancellationToken,
    timeouts: Timeouts,
) -> io::Result<()> {
    let connections = TaskTracker::new();
    // Cancelled when the grace is over: the connections still open stop.
    let force = CancellationToken::new();
    loop {
        let accepted = tokio::select! {
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            Err(error) if is_connection_error(&error) => continue,
            Err(error) => {
                // Out of file descriptors and the like: wait a little,
                // like axum::serve does.
                tracing::error!(error = %error, "accept failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let router = router.clone();
        let stop = Stop {
            shutdown: shutdown.clone(),
            force: force.clone(),
        };
        connections.spawn(async move {
            if let Err(error) = connection(stream, peer, router, stop, timeouts).await {
                tracing::debug!(error = %error, "connection ended with an error");
            }
        });
    }
    // No new connections from here: the port is free and the backlog gone.
    drop(listener);
    connections.close();
    if tokio::time::timeout(timeouts.grace, connections.wait())
        .await
        .is_err()
    {
        tracing::warn!(
            open = connections.len(),
            "connections still open after the shutdown grace; closing them"
        );
        force.cancel();
        connections.wait().await;
    }
    Ok(())
}

/// What ends a connection: `shutdown` asks it to finish, `force` drops it.
struct Stop {
    shutdown: CancellationToken,
    force: CancellationToken,
}

fn is_connection_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    H1,
    H2,
}

/// Reads until it is clear whether the client speaks HTTP/2 (its preface)
/// or HTTP/1. `None` if it closed first.
async fn sniff(stream: &mut TcpStream) -> io::Result<Option<(Version, Bytes)>> {
    let mut buf = vec![0_u8; H2_PREFACE.len()];
    let mut filled = 0_usize;
    loop {
        let free = buf.get_mut(filled..).unwrap_or_default();
        let read = stream.read(free).await?;
        if read == 0 {
            if filled == 0 {
                return Ok(None);
            }
            // Short and closed: let HTTP/1 answer (or not) what it got.
            buf.truncate(filled);
            return Ok(Some((Version::H1, Bytes::from(buf))));
        }
        filled = filled.saturating_add(read);
        let got = buf.get(..filled).unwrap_or_default();
        if !H2_PREFACE.starts_with(got) {
            buf.truncate(filled);
            return Ok(Some((Version::H1, Bytes::from(buf))));
        }
        if filled == H2_PREFACE.len() {
            return Ok(Some((Version::H2, Bytes::from(buf))));
        }
    }
}

async fn connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    router: Router,
    stop: Stop,
    timeouts: Timeouts,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let header_timeout = timeouts.header;
    // Small responses (JSON, push events) shouldn't wait for Nagle.
    let _ = stream.set_nodelay(true);
    let Ok(sniffed) = tokio::time::timeout(header_timeout, sniff(&mut stream)).await else {
        return Ok(()); // Nothing (or not enough) in time: just close.
    };
    let Some((version, read)) = sniffed? else {
        return Ok(());
    };
    let io = TokioIo::new(Prefixed::new(read, stream, timeouts.write_stall));
    // `ConnectInfo` for the handlers that look at the client's address.
    let service =
        TowerToHyperService::new(router.map_request(move |request: http::Request<Incoming>| {
            let mut request = request.map(Body::new);
            request.extensions_mut().insert(ConnectInfo(peer));
            request
        }));
    match version {
        Version::H1 => {
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(header_timeout);
            let connection = pin!(builder.serve_connection(io, service));
            drive(
                connection,
                &stop,
                hyper::server::conn::http1::Connection::graceful_shutdown,
            )
            .await?;
        }
        Version::H2 => {
            let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
            builder
                .timer(TokioTimer::new())
                .keep_alive_interval(Some(H2_KEEP_ALIVE))
                .keep_alive_timeout(H2_KEEP_ALIVE)
                .max_concurrent_streams(H2_MAX_STREAMS);
            let connection = pin!(builder.serve_connection(io, service));
            drive(
                connection,
                &stop,
                hyper::server::conn::http2::Connection::graceful_shutdown,
            )
            .await?;
        }
    }
    Ok(())
}

/// Runs a connection; on shutdown, asks it to finish gracefully (no new
/// requests, open ones complete) and keeps running it until it does or
/// `force` drops it (the socket closes with it).
async fn drive<C, E>(
    mut connection: Pin<&mut C>,
    stop: &Stop,
    graceful: impl FnOnce(Pin<&mut C>),
) -> Result<(), E>
where
    C: Future<Output = Result<(), E>>,
{
    tokio::select! {
        result = connection.as_mut() => return result,
        () = stop.shutdown.cancelled() => graceful(connection.as_mut()),
    }
    tokio::select! {
        result = connection => result,
        () = stop.force.cancelled() => Ok(()),
    }
}

/// A stream with bytes already read from it put back in front, whose
/// writes fail once the socket takes nothing for `write_stall`.
struct Prefixed {
    prefix: Bytes,
    inner: TcpStream,
    write_stall: Duration,
    /// Armed while a write waits on a full socket.
    stalled: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Prefixed {
    fn new(prefix: Bytes, inner: TcpStream, write_stall: Duration) -> Self {
        Self {
            prefix,
            inner,
            write_stall,
            stalled: None,
        }
    }

    /// Passes a write's result on; a pending write arms the stall timer
    /// and fails with `TimedOut` when it rings, progress disarms it.
    fn watch<T>(&mut self, cx: &mut Context<'_>, poll: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if poll.is_ready() {
            self.stalled = None;
            return poll;
        }
        let limit = self.write_stall;
        let timer = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
        match timer.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the client stopped reading",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncRead for Prefixed {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.prefix.has_remaining() {
            let take = this.prefix.len().min(buf.remaining());
            let piece = this.prefix.split_to(take);
            buf.put_slice(&piece);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Prefixed {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        this.watch(cx, poll)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
        this.watch(cx, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_flush(cx);
        this.watch(cx, poll)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use axum::routing::get;
    use tokio::io::AsyncWriteExt as _;

    use super::*;

    const SHORT: Duration = Duration::from_millis(300);

    async fn start() -> (
        SocketAddr,
        CancellationToken,
        tokio::task::JoinHandle<io::Result<()>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route(
            "/",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.ip().to_string() }),
        );
        let shutdown = CancellationToken::new();
        let timeouts = Timeouts {
            header: SHORT,
            ..Timeouts::DEFAULT
        };
        let task = tokio::spawn(serve(listener, router, shutdown.clone(), timeouts));
        (addr, shutdown, task)
    }

    /// A server whose `/` sends a body that never ends.
    async fn endless(
        timeouts: Timeouts,
    ) -> (
        SocketAddr,
        CancellationToken,
        tokio::task::JoinHandle<io::Result<()>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route(
            "/",
            get(|| async {
                let chunk = Bytes::from(vec![b'x'; 64 * 1024]);
                Body::from_stream(futures::stream::repeat_with(move || {
                    Ok::<_, io::Error>(chunk.clone())
                }))
            }),
        );
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(serve(listener, router, shutdown.clone(), timeouts));
        (addr, shutdown, task)
    }

    #[tokio::test]
    async fn shutdown_has_a_deadline_and_closes_the_listener() {
        let (addr, shutdown, task) = endless(Timeouts {
            header: SHORT,
            grace: SHORT,
            write_stall: Duration::from_secs(60),
        })
        .await;
        // A client that asks and never reads: its request never finishes.
        let mut reader = TcpStream::connect(addr).await.unwrap();
        reader
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = Instant::now();
        shutdown.cancel();
        // Closed at once, before the grace ends.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let refused = TcpStream::connect(addr).await.unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(start.elapsed() < Duration::from_secs(3));
        // And the stuck connection went with it.
        assert!(time_to_close(&mut reader).await < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn stalled_reader_is_dropped() {
        let (addr, shutdown, _task) = endless(Timeouts {
            header: SHORT,
            grace: SHORT,
            write_stall: SHORT,
        })
        .await;
        let mut reader = TcpStream::connect(addr).await.unwrap();
        reader
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        // Not reading fills the socket; the server gives up on it.
        tokio::time::sleep(Duration::from_secs(2)).await;
        // What was buffered drains, then the connection ends (it would
        // go on forever otherwise).
        assert!(time_to_close(&mut reader).await < Duration::from_secs(5));
        shutdown.cancel();
    }

    /// Reads until the server closes; how long that took.
    async fn time_to_close(stream: &mut TcpStream) -> Duration {
        let start = Instant::now();
        let mut sink = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut sink)).await;
        start.elapsed()
    }

    #[tokio::test]
    async fn answers_http1_with_the_peer_address() {
        let (addr, shutdown, task) = start().await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await.unwrap();
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(reply.ends_with("127.0.0.1"), "{reply}");
        shutdown.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn silent_and_slow_clients_are_dropped() {
        let (addr, shutdown, _task) = start().await;
        // Says nothing.
        let mut silent = TcpStream::connect(addr).await.unwrap();
        assert!(time_to_close(&mut silent).await < Duration::from_secs(5));
        // Headers that never end (slowloris).
        let mut slow = TcpStream::connect(addr).await.unwrap();
        slow.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n")
            .await
            .unwrap();
        assert!(time_to_close(&mut slow).await < Duration::from_secs(5));
        // Half an HTTP/2 preface.
        let mut half = TcpStream::connect(addr).await.unwrap();
        half.write_all(&H2_PREFACE[..10]).await.unwrap();
        assert!(time_to_close(&mut half).await < Duration::from_secs(5));
        // Idle after a first request on a kept-alive connection.
        let mut idle = TcpStream::connect(addr).await.unwrap();
        idle.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        assert!(time_to_close(&mut idle).await < Duration::from_secs(5));
        shutdown.cancel();
    }

    #[tokio::test]
    async fn prefixed_reads_the_prefix_first() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        client.write_all(b" world").await.unwrap();
        drop(client);
        let mut prefixed = Prefixed::new(Bytes::from_static(b"hello"), server, SHORT);
        let mut all = String::new();
        prefixed.read_to_string(&mut all).await.unwrap();
        assert_eq!(all, "hello world");
    }
}
