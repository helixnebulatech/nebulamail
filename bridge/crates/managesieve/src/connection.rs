// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Reading lines and running commands on one stream.

use std::pin::Pin;
use std::task::{Context, Poll};

use nebula_managesieve_proto::{Command, Completion, Item, ResponseDecoder, ServerLine, Status};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::client::TlsStream;

use crate::{Error, Limits};

/// Size of one read from the network.
const READ_CHUNK: usize = 16 * 1024;

/// A stream that is always encrypted. This is the only transport a
/// [`crate::Client`] can hold, so credentials never go out in plaintext.
pub(crate) enum SecureStream {
    /// TLS over TCP.
    Tls(Box<TlsStream<TcpStream>>),
    /// In-memory stream for unit tests of the command logic.
    #[cfg(test)]
    Test(tokio::io::DuplexStream),
}

impl AsyncRead for SecureStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
            #[cfg(test)]
            Self::Test(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for SecureStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
            #[cfg(test)]
            Self::Test(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
            #[cfg(test)]
            Self::Test(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
            #[cfg(test)]
            Self::Test(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

/// `now + duration`, or `now` if that overflows (absurd durations).
pub(crate) fn after(duration: std::time::Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration).unwrap_or(now)
}

/// The lines of one response: its data lines and how it ended.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) data: Vec<Vec<Item>>,
    pub(crate) done: Completion,
}

/// One ManageSieve stream with its decoder.
pub(crate) struct Connection<S> {
    stream: S,
    decoder: ResponseDecoder,
    pub(crate) limits: Limits,
    read_buffer: Vec<u8>,
    /// Bytes the current response may still take
    /// ([`Limits::max_reply_bytes`]).
    reply_left: usize,
    /// Set by the first fatal error: later commands fail at once.
    broken: bool,
}

impl<S> std::fmt::Debug for Connection<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("broken", &self.broken)
            .finish_non_exhaustive()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub(crate) fn new(stream: S, limits: Limits) -> Self {
        Self {
            stream,
            decoder: ResponseDecoder::new(limits.frame),
            limits,
            read_buffer: vec![0; READ_CHUNK],
            reply_left: limits.max_reply_bytes(),
            broken: false,
        }
    }

    /// Gives back the stream (for the TLS upgrade).
    pub(crate) fn into_stream(self) -> S {
        self.stream
    }

    /// Bytes received and not decoded yet.
    pub(crate) fn buffered(&self) -> usize {
        self.decoder.buffered()
    }

    pub(crate) fn is_broken(&self) -> bool {
        self.broken
    }

    /// After a fatal error found above this layer (a reply that makes no
    /// sense): later commands fail at once.
    pub(crate) fn mark_broken(&mut self) {
        self.broken = true;
    }

    /// A new response starts: its byte budget starts again.
    pub(crate) fn begin_reply(&mut self) {
        self.reply_left = self.limits.max_reply_bytes();
    }

    pub(crate) fn command_deadline(&self) -> Instant {
        after(self.limits.command_timeout)
    }

    fn track<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.broken = true;
        }
        result
    }

    /// The next line, by `deadline` (and never more than `read_timeout` of
    /// silence).
    pub(crate) async fn read_line(
        &mut self,
        deadline: Instant,
        during: &'static str,
    ) -> Result<ServerLine, Error> {
        let result = self.read_line_inner(deadline, during).await;
        self.track(result)
    }

    async fn read_line_inner(
        &mut self,
        deadline: Instant,
        during: &'static str,
    ) -> Result<ServerLine, Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        loop {
            if let Some(line) = self.decoder.decode()? {
                return Ok(line);
            }
            let until = deadline.min(after(self.limits.read_timeout));
            let read = tokio::time::timeout_at(until, self.stream.read(&mut self.read_buffer))
                .await
                .map_err(|_| Error::Timeout(during))?;
            let count = read?;
            if count == 0 {
                return Err(Error::ConnectionClosed);
            }
            // Counted as received, so a line that never ends is cut too.
            self.reply_left = self
                .reply_left
                .checked_sub(count)
                .ok_or(Error::Protocol("response too large"))?;
            let bytes = self.read_buffer.get(..count).unwrap_or_default();
            self.decoder.feed(bytes);
        }
    }

    /// Writes bytes and flushes, by `deadline`.
    pub(crate) async fn write(
        &mut self,
        bytes: &[u8],
        deadline: Instant,
        during: &'static str,
    ) -> Result<(), Error> {
        let result = self.write_inner(bytes, deadline, during).await;
        self.track(result)
    }

    async fn write_inner(
        &mut self,
        bytes: &[u8],
        deadline: Instant,
        during: &'static str,
    ) -> Result<(), Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        let write = async {
            self.stream.write_all(bytes).await?;
            self.stream.flush().await
        };
        tokio::time::timeout_at(deadline, write)
            .await
            .map_err(|_| Error::Timeout(during))??;
        Ok(())
    }

    /// Reads data lines until the completion. A `BYE` is an error (and the
    /// connection is then broken).
    pub(crate) async fn read_reply(
        &mut self,
        deadline: Instant,
        during: &'static str,
    ) -> Result<Reply, Error> {
        self.begin_reply();
        let mut data = Vec::new();
        loop {
            match self.read_line(deadline, during).await? {
                ServerLine::Data(items) => {
                    if data.len() >= self.limits.max_data_lines {
                        self.broken = true;
                        return Err(Error::Protocol("too many lines in one response"));
                    }
                    data.push(items);
                }
                ServerLine::Done(done) if done.status == Status::Bye => {
                    self.broken = true;
                    return Err(Error::Bye(done.text_one_line()));
                }
                ServerLine::Done(done) => return Ok(Reply { data, done }),
            }
        }
    }

    /// Sends a command and reads its whole response.
    pub(crate) async fn run(&mut self, command: &Command) -> Result<Reply, Error> {
        let name = command.name();
        tracing::debug!(command = name, "ManageSieve command");
        let deadline = self.command_deadline();
        let bytes = command.encode();
        self.write(&bytes, deadline, name).await?;
        drop(bytes);
        let reply = self.read_reply(deadline, name).await?;
        tracing::debug!(command = name, status = ?reply.done.status, "ManageSieve completion");
        Ok(reply)
    }
}
