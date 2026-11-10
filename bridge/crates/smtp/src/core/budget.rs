// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! A cap on the bytes received from the server on one connection.
//!
//! lettre reads replies with `read_line`, which keeps reading until it finds
//! `\n` and only then checks the line length: a server that never sends `\n`
//! fills memory until the timeout. [`ReadBudget`] sits under lettre and
//! under TLS (so it counts the encrypted bytes) and fails the read once the
//! connection has received [`MAX_RECEIVED_BYTES`], whatever lettre does.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use lettre::transport::smtp::client::AsyncTokioStream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::core::activity::Activity;

/// Everything one connection may receive: greeting, three `EHLO`s, TLS,
/// `AUTH`, up to 100 `RCPT` replies, `DATA` and `QUIT`. A real session
/// stays well under 100 KiB; the message we send does not count.
pub(crate) const MAX_RECEIVED_BYTES: usize = 1024 * 1024;

/// The server sent more than [`MAX_RECEIVED_BYTES`]: the error inside the
/// [`io::Error`] that [`ReadBudget`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the server sent more than {limit} bytes")]
pub(crate) struct ReceivedTooMuch {
    limit: usize,
}

/// A stream that fails reads after `limit` bytes. Writes pass through.
/// It also tells [`Activity`] what really goes through the socket (the
/// data timeouts of RFC 5321 are measured here, under TLS).
pub(crate) struct ReadBudget<S> {
    inner: S,
    limit: usize,
    left: usize,
    activity: Activity,
}

impl<S> ReadBudget<S> {
    /// With the connection cap, [`MAX_RECEIVED_BYTES`].
    pub(crate) fn new(inner: S) -> Self {
        Self::with_limit(inner, MAX_RECEIVED_BYTES)
    }

    pub(crate) fn with_limit(inner: S, limit: usize) -> Self {
        Self {
            inner,
            limit,
            left: limit,
            activity: Activity::default(),
        }
    }

    /// Reports reads and writes to `activity`.
    pub(crate) fn with_activity(mut self, activity: Activity) -> Self {
        self.activity = activity;
        self
    }

    pub(crate) fn get_ref(&self) -> &S {
        &self.inner
    }
}

impl<S> fmt::Debug for ReadBudget<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Nothing of the stream itself: just the counters.
        f.debug_struct("ReadBudget")
            .field("limit", &self.limit)
            .field("left", &self.left)
            .finish_non_exhaustive()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for ReadBudget<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.activity.reading();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.left == 0 {
            return Poll::Ready(Err(io::Error::other(ReceivedTooMuch { limit: this.limit })));
        }
        let read = if buf.remaining() <= this.left {
            // The whole buffer fits in the budget: read into it directly.
            let before = buf.filled().len();
            let poll = Pin::new(&mut this.inner).poll_read(cx, buf);
            if let Poll::Ready(Ok(())) = poll {
                Ok(buf.filled().len().saturating_sub(before))
            } else {
                Err(poll)
            }
        } else {
            // Only `left` bytes may come: read into a smaller buffer over
            // the (initialized) front of the unfilled part, then mark them
            // filled. No `unsafe`, unlike `ReadBuf::take`.
            let destination = buf.initialize_unfilled_to(this.left);
            let mut smaller = ReadBuf::new(destination);
            let poll = Pin::new(&mut this.inner).poll_read(cx, &mut smaller);
            let count = smaller.filled().len();
            if let Poll::Ready(Ok(())) = poll {
                buf.advance(count);
                Ok(count)
            } else {
                Err(poll)
            }
        };
        match read {
            Ok(count) => {
                this.left = this.left.saturating_sub(count);
                Poll::Ready(Ok(()))
            }
            Err(poll) => poll,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ReadBudget<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(count)) = poll {
            this.activity.wrote(count);
        }
        poll
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

impl AsyncTokioStream for ReadBudget<TcpStream> {
    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.inner.peer_addr()
    }
}

/// Whether `error` (from lettre) comes from [`ReadBudget`]: somewhere in its
/// `source()` chain there is an [`io::Error`] wrapping [`ReceivedTooMuch`].
/// `io::Error::source()` skips its own inner error, so each `io::Error` is
/// looked into with `get_ref()`.
pub(crate) fn received_too_much(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.downcast_ref::<ReceivedTooMuch>().is_some() {
            return true;
        }
        let wrapped = error
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
            .is_some_and(|inner| inner.downcast_ref::<ReceivedTooMuch>().is_some());
        if wrapped {
            return true;
        }
        current = error.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn fails_after_the_limit() {
        let (mut server, client) = tokio::io::duplex(64);
        let mut budget = ReadBudget::with_limit(client, 10);
        server.write_all(b"0123456789X").await.unwrap();
        let mut received = Vec::new();
        let mut chunk = [0_u8; 4];
        let error = loop {
            match budget.read(&mut chunk).await {
                Ok(count) => received.extend_from_slice(&chunk[..count]),
                Err(error) => break error,
            }
        };
        assert_eq!(received, b"0123456789");
        assert!(received_too_much(&error), "{error:?}");
        // It stays spent.
        assert!(budget.read(&mut chunk).await.is_err());
    }

    #[tokio::test]
    async fn a_large_buffer_gets_only_the_budget() {
        let (mut server, client) = tokio::io::duplex(64);
        let mut budget = ReadBudget::with_limit(client, 5);
        server.write_all(b"0123456789").await.unwrap();
        let mut buffer = [0_u8; 32];
        assert_eq!(budget.read(&mut buffer).await.unwrap(), 5);
        assert_eq!(&buffer[..5], b"01234");
        assert!(budget.read(&mut buffer).await.is_err());
    }

    #[tokio::test]
    async fn exactly_the_limit_then_end_of_stream_is_fine() {
        let (mut server, client) = tokio::io::duplex(64);
        let mut budget = ReadBudget::with_limit(client, 4);
        server.write_all(b"abcd").await.unwrap();
        drop(server);
        let mut received = Vec::new();
        let mut chunk = [0_u8; 2];
        loop {
            let count = budget.read(&mut chunk).await.unwrap_or(0);
            if count == 0 {
                break;
            }
            received.extend_from_slice(&chunk[..count]);
        }
        assert_eq!(received, b"abcd");
    }

    #[test]
    fn recognized_in_a_lettre_like_chain() {
        #[derive(Debug, thiserror::Error)]
        #[error("network error")]
        struct Outer(#[source] io::Error);

        let error = Outer(io::Error::other(ReceivedTooMuch { limit: 1 }));
        assert!(received_too_much(&error));
        let other = Outer(io::Error::other("connection reset"));
        assert!(!received_too_much(&other));
        let plain = io::Error::new(io::ErrorKind::BrokenPipe, "x");
        assert!(!received_too_much(&plain));
    }
}
