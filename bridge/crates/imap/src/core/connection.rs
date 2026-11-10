// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Reading responses and running commands on one stream.

use std::pin::Pin;
use std::task::{Context, Poll};

use nebula_imap_proto::command::{self, ChunkEnd, Command, EncodeOptions, RawLine};
use nebula_imap_proto::response::{
    Completion, Condition, Response, ResponseDecoder, Tagged, Untagged,
};
use nebula_imap_proto::types::TagGenerator;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_rustls::client::TlsStream;

use crate::{Error, Limits, Refusal};

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

/// When a read must finish.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Deadline {
    /// Normal command: `read_timeout` of silence or the command deadline.
    Command(Instant),
    /// Waiting in IDLE: only the given instant counts.
    At(Instant),
}

/// What to do with a `+` received while waiting for the tagged response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StrayContinuation {
    /// A protocol error.
    Reject,
    /// Answer `*` (cancel, RFC 3501 §6.2.2): XOAUTH2 sends an error
    /// challenge before its NO.
    CancelSasl,
    /// OAUTHBEARER: answer its error challenge with `%x01` (RFC 7628
    /// §3.2.3), so the server ends with NO; `*` for any further one.
    OAuthBearer,
}

/// One IMAP stream with its decoder and tag generator.
pub(crate) struct Connection<S> {
    stream: S,
    decoder: ResponseDecoder,
    tags: TagGenerator,
    pub(crate) limits: Limits,
    read_buffer: Vec<u8>,
    /// Set by the first fatal error: later commands fail at once.
    broken: bool,
    /// Text of a BYE, reported if the stream then closes.
    bye: Option<String>,
    /// A command was sent and its tagged response has not arrived yet. If
    /// the future running it is dropped (the HTTP client went away, a
    /// timeout layer fired), this stays set and the connection counts as
    /// broken: the next command would read the answers of the old one.
    in_flight: bool,
    /// What the current command has received so far.
    budget: CommandBudget,
}

/// Untagged data received by the current command, checked against
/// [`Limits::max_command_bytes`] and [`Limits::max_command_responses`].
#[derive(Debug, Clone, Copy, Default)]
struct CommandBudget {
    bytes: usize,
    responses: usize,
}

impl<S> std::fmt::Debug for Connection<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("broken", &self.broken)
            .field("in_flight", &self.in_flight)
            .finish_non_exhaustive()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub(crate) fn new(stream: S, limits: Limits) -> Self {
        Self {
            stream,
            decoder: ResponseDecoder::new(limits.frame),
            tags: TagGenerator::new(),
            limits,
            read_buffer: vec![0; READ_CHUNK],
            broken: false,
            bye: None,
            in_flight: false,
            budget: CommandBudget::default(),
        }
    }

    /// Gives back the stream (for the TLS upgrade).
    pub(crate) fn into_stream(self) -> S {
        self.stream
    }

    pub(crate) fn decoder(&self) -> &ResponseDecoder {
        &self.decoder
    }

    pub(crate) fn decoder_mut(&mut self) -> &mut ResponseDecoder {
        &mut self.decoder
    }

    /// Broken by a fatal error, or left with a command half done.
    pub(crate) fn is_broken(&self) -> bool {
        self.broken || self.in_flight
    }

    /// Starts counting the untagged data of a new command.
    pub(crate) fn begin_command(&mut self) {
        self.budget = CommandBudget::default();
    }

    /// The open command (IDLE) got its tagged response.
    pub(crate) fn finish_command(&mut self) {
        self.in_flight = false;
    }

    /// Counts one response of `len` bytes against the command budget.
    fn charge(&mut self, response: &Response, len: usize) -> Result<(), Error> {
        if !matches!(response, Response::Untagged(_)) {
            return Ok(());
        }
        self.budget.bytes = self.budget.bytes.saturating_add(len);
        self.budget.responses = self.budget.responses.saturating_add(1);
        if self.budget.bytes > self.limits.max_command_bytes
            || self.budget.responses > self.limits.max_command_responses
        {
            return Err(Error::Protocol("too much data for one command"));
        }
        Ok(())
    }

    pub(crate) fn mark_broken(&mut self) {
        self.broken = true;
    }

    pub(crate) fn command_deadline(&self) -> Deadline {
        Deadline::Command(after(self.limits.command_timeout))
    }

    /// Marks the connection broken if `result` is a fatal error.
    fn track<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if let Err(error) = &result
            && error.is_fatal()
        {
            self.broken = true;
        }
        result
    }

    /// Reads the next response. Cancel-safe: bytes already read stay in the
    /// decoder.
    pub(crate) async fn read_response(
        &mut self,
        deadline: Deadline,
        during: &'static str,
    ) -> Result<Response, Error> {
        let result = self.read_response_inner(deadline, during).await;
        self.track(result)
    }

    async fn read_response_inner(
        &mut self,
        deadline: Deadline,
        during: &'static str,
    ) -> Result<Response, Error> {
        loop {
            let before = self.decoder.buffered();
            if let Some(response) = self.decode_buffered()? {
                let len = before.saturating_sub(self.decoder.buffered());
                self.charge(&response, len)?;
                return Ok(response);
            }
            // `read_timeout` is per read: a big literal may take long, as
            // long as bytes keep coming.
            let until = match deadline {
                Deadline::Command(end) => end.min(after(self.limits.read_timeout)),
                Deadline::At(end) => end,
            };
            if !self.read_once(until).await? {
                return Err(Error::Timeout(during));
            }
        }
    }

    /// One read from the stream into the decoder. `false` on timeout.
    async fn read_once(&mut self, until: Instant) -> Result<bool, Error> {
        let Ok(read) =
            tokio::time::timeout_at(until, self.stream.read(&mut self.read_buffer)).await
        else {
            return Ok(false);
        };
        let count = read?;
        if count == 0 {
            return Err(self.bye.take().map_or(Error::ConnectionClosed, Error::Bye));
        }
        let bytes = self.read_buffer.get(..count).unwrap_or_default();
        self.decoder.feed(bytes);
        Ok(true)
    }

    /// Reads the next response, or `None` once `until` passes (which is not
    /// an error: IDLE uses it to wake up). Cancel-safe.
    pub(crate) async fn read_until(&mut self, until: Instant) -> Result<Option<Response>, Error> {
        let result = self.read_until_inner(until).await;
        self.track(result)
    }

    /// A response that is already complete in the buffer, without reading.
    pub(crate) fn try_decode(&mut self) -> Result<Option<Response>, Error> {
        let result = self.decode_buffered();
        self.track(result)
    }

    fn decode_buffered(&mut self) -> Result<Option<Response>, Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        let response = self.decoder.decode()?;
        if let Some(Response::Untagged(Untagged::Status(status))) = &response
            && status.condition == Condition::Bye
        {
            self.bye = Some(status.text.clone());
        }
        Ok(response)
    }

    async fn read_until_inner(&mut self, until: Instant) -> Result<Option<Response>, Error> {
        loop {
            if let Some(response) = self.decode_buffered()? {
                return Ok(Some(response));
            }
            if !self.read_once(until).await? {
                return Ok(None);
            }
        }
    }

    /// Writes bytes (a chunk or a raw line) and flushes.
    pub(crate) async fn write(
        &mut self,
        bytes: &[u8],
        deadline: Deadline,
        during: &'static str,
    ) -> Result<(), Error> {
        let result = self.write_inner(bytes, deadline, during).await;
        self.track(result)
    }

    async fn write_inner(
        &mut self,
        bytes: &[u8],
        deadline: Deadline,
        during: &'static str,
    ) -> Result<(), Error> {
        if self.broken {
            return Err(Error::Broken);
        }
        let until = match deadline {
            Deadline::Command(end) | Deadline::At(end) => end,
        };
        let write = async {
            self.stream.write_all(bytes).await?;
            self.stream.flush().await
        };
        tokio::time::timeout_at(until, write)
            .await
            .map_err(|_| Error::Timeout(during))??;
        Ok(())
    }

    pub(crate) async fn write_line(
        &mut self,
        line: &RawLine,
        deadline: Deadline,
        during: &'static str,
    ) -> Result<(), Error> {
        self.write(line.bytes(), deadline, during).await
    }

    /// Sends a command and reads until its tagged response. Untagged
    /// responses go to `on_untagged`. Returns the tagged response, OK or not.
    pub(crate) async fn run(
        &mut self,
        command: &impl Command,
        options: EncodeOptions,
        stray: StrayContinuation,
        on_untagged: &mut (dyn FnMut(Untagged) + Send),
    ) -> Result<Tagged, Error> {
        if self.is_broken() {
            return Err(Error::Broken);
        }
        let encoded = command::encode(command, self.tags.next_tag(), options)?;
        let name = encoded.name();
        let tag = encoded.tag().clone();
        tracing::debug!(command = name, tag = %tag, "IMAP command");
        let deadline = self.command_deadline();
        self.begin_command();
        // From here until our tagged response, the stream is mid-command.
        // Every error below is fatal anyway; should one ever not be, the
        // flag stays set and the connection is dropped (fail-closed).
        self.in_flight = true;
        for chunk in encoded.chunks() {
            self.write(chunk.bytes(), deadline, name).await?;
            if chunk.end() == ChunkEnd::AwaitContinuation
                && let Some(tagged) = self
                    .await_continuation(&tag, deadline, name, on_untagged)
                    .await?
            {
                // The server refused the literal or the SASL exchange.
                self.in_flight = false;
                return Ok(tagged);
            }
        }
        let mut acknowledged_error = false;
        loop {
            match self.read_response(deadline, name).await? {
                Response::Untagged(data) => on_untagged(data),
                Response::Tagged(tagged) if tagged.tag == tag => {
                    tracing::debug!(command = name, tag = %tag, status = ?tagged.status, "IMAP completion");
                    self.in_flight = false;
                    return Ok(tagged);
                }
                Response::Tagged(_) => {
                    return self.track(Err(Error::Protocol("tagged response for another command")));
                }
                Response::Continuation(_) => match stray {
                    StrayContinuation::OAuthBearer if !acknowledged_error => {
                        acknowledged_error = true;
                        self.write_line(&command::oauthbearer_error_ack(), deadline, name)
                            .await?;
                    }
                    StrayContinuation::CancelSasl | StrayContinuation::OAuthBearer => {
                        self.write_line(&command::sasl_cancel(), deadline, name)
                            .await?;
                    }
                    StrayContinuation::Reject => {
                        return self.track(Err(Error::Protocol("unexpected continuation request")));
                    }
                },
                _ => return self.track(Err(Error::Protocol("unknown response kind"))),
            }
        }
    }

    /// Sends a command that answers with `+` and then stays open (IDLE).
    /// Returns its tag, or the tagged response if the server refused it.
    pub(crate) async fn start_open_command(
        &mut self,
        command: &impl Command,
        options: EncodeOptions,
        on_untagged: &mut (dyn FnMut(Untagged) + Send),
    ) -> Result<Result<nebula_imap_proto::types::Tag, Tagged>, Error> {
        if self.is_broken() {
            return Err(Error::Broken);
        }
        let encoded = command::encode(command, self.tags.next_tag(), options)?;
        let name = encoded.name();
        let tag = encoded.tag().clone();
        tracing::debug!(command = name, tag = %tag, "IMAP command");
        let deadline = self.command_deadline();
        self.begin_command();
        // Cleared by the tagged response: here if refused, else by
        // `finish_command` when IDLE ends.
        self.in_flight = true;
        for chunk in encoded.chunks() {
            self.write(chunk.bytes(), deadline, name).await?;
        }
        match self
            .await_continuation(&tag, deadline, name, on_untagged)
            .await?
        {
            None => Ok(Ok(tag)),
            Some(tagged) => {
                self.in_flight = false;
                Ok(Err(tagged))
            }
        }
    }

    /// Waits for `+`. Returns the tagged response if the command ended
    /// instead.
    async fn await_continuation(
        &mut self,
        tag: &nebula_imap_proto::types::Tag,
        deadline: Deadline,
        name: &'static str,
        on_untagged: &mut (dyn FnMut(Untagged) + Send),
    ) -> Result<Option<Tagged>, Error> {
        loop {
            match self.read_response(deadline, name).await? {
                Response::Continuation(_) => return Ok(None),
                Response::Untagged(data) => on_untagged(data),
                Response::Tagged(tagged) if &tagged.tag == tag => return Ok(Some(tagged)),
                Response::Tagged(_) => {
                    return self.track(Err(Error::Protocol("tagged response for another command")));
                }
                _ => return self.track(Err(Error::Protocol("unknown response kind"))),
            }
        }
    }
}

/// Turns a tagged response into `Ok` or a [`Refusal`].
pub(crate) fn expect_ok(command: &'static str, tagged: Tagged) -> Result<Tagged, Error> {
    match tagged.status {
        Completion::Ok => Ok(tagged),
        status => Err(Error::Refused(Refusal {
            command,
            status,
            code: tagged.code,
            text: tagged.text,
        })),
    }
}
