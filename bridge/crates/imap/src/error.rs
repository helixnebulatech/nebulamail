// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Errors of the IMAP client.

use std::fmt;

use nebula_imap_proto::response::{Completion, ResponseCode};
use nebula_imap_proto::{DecodeError, EncodeError};

/// The server answered a command with NO or BAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The command (`SELECT`, `UID MOVE`…).
    pub command: &'static str,
    /// NO or BAD.
    pub status: Completion,
    /// The response code, if any (`TRYCREATE`, `OVERQUOTA`…).
    pub code: Option<ResponseCode>,
    /// The server's text (for humans; may be shown to the user).
    pub text: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = match self.status {
            Completion::No => "NO",
            Completion::Bad => "BAD",
            _ => "OK",
        };
        write!(f, "{} failed: {status} {}", self.command, self.text)
    }
}

/// Errors of the IMAP client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Network error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The TLS configuration could not be built.
    #[error("TLS configuration error: {0}")]
    TlsConfig(#[from] rustls::Error),
    /// The server name is not a valid DNS name or IP address.
    #[error("invalid server name")]
    InvalidServerName,
    /// No answer in time.
    #[error("timed out during {0}")]
    Timeout(&'static str),
    /// The server closed the connection.
    #[error("the server closed the connection")]
    ConnectionClosed,
    /// The server said BYE (shutdown, idle timeout…).
    #[error("the server closed the connection: {0}")]
    Bye(String),
    /// A response broke the size limits.
    #[error("response rejected: {0}")]
    Decode(#[from] DecodeError),
    /// A command could not be encoded safely.
    #[error("command rejected: {0}")]
    Encode(#[from] EncodeError),
    /// The server broke the protocol (unexpected response).
    #[error("protocol error: {0}")]
    Protocol(&'static str),
    /// STARTTLS mode, but the server does not offer STARTTLS. We never go on
    /// in plaintext.
    #[error("the server does not offer STARTTLS")]
    StartTlsUnavailable,
    /// PREAUTH greeting before TLS: refused, it would skip STARTTLS.
    #[error("PREAUTH greeting on a plaintext connection")]
    PreAuthOnPlaintext,
    /// Data arrived together with the STARTTLS answer, before TLS started:
    /// a man in the middle may have injected it.
    #[error("unexpected data before the TLS handshake")]
    InjectedBeforeTls,
    /// Certificate checks can only be turned off for loopback peers.
    #[error("TLS verification can only be disabled for loopback servers")]
    InsecureTlsNotLoopback,
    /// The server is not IMAP4rev1/IMAP4rev2.
    #[error("the server does not speak IMAP4rev1 or IMAP4rev2")]
    NotImap4,
    /// No authentication method we can use (LOGINDISABLED without PLAIN…).
    #[error("no usable authentication mechanism")]
    NoAuthMechanism,
    /// Wrong user name or password (or the server refused the login).
    #[error("authentication failed: {text}")]
    AuthFailed {
        /// The code, usually `AUTHENTICATIONFAILED`.
        code: Option<ResponseCode>,
        /// The server's text.
        text: String,
    },
    /// The server answered NO or BAD.
    #[error("{0}")]
    Refused(Refusal),
    /// The server lacks a capability the operation needs.
    #[error("the server does not support {0}")]
    Unsupported(&'static str),
    /// SELECT finished without UIDVALIDITY.
    #[error("the server did not send UIDVALIDITY")]
    MissingUidValidity,
    /// The command ended with OK, but some of its data was larger than the
    /// limits and was dropped: the result would be incomplete. Not fatal:
    /// the connection is still in step with the server (except after a
    /// state change, see [`StateError`]).
    #[error("{0}: a response was larger than the limits")]
    ResponseTooLarge(&'static str),
    /// An earlier fatal error left the connection unusable.
    #[error("the connection is broken by an earlier error")]
    Broken,
}

impl Error {
    /// Whether the connection is unusable after this error.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        !matches!(
            self,
            Self::Refused(_)
                | Self::AuthFailed { .. }
                | Self::Unsupported(_)
                | Self::Encode(_)
                | Self::NoAuthMechanism
                | Self::ResponseTooLarge(_)
        )
    }
}

/// A state change (login, select, close…) that failed.
///
/// It carries the connection in its previous (or fallback) state when it is
/// still usable, for example after a NO, and nothing when the error was
/// fatal (the connection is dropped). Boxed, so `Result`s stay small.
pub struct StateError<C>(Box<StateErrorInner<C>>);

struct StateErrorInner<C> {
    error: Error,
    client: Option<C>,
}

impl<C> StateError<C> {
    pub(crate) fn fatal(error: impl Into<Error>) -> Self {
        Self(Box::new(StateErrorInner {
            error: error.into(),
            client: None,
        }))
    }

    /// The error of a state change (LOGIN, SELECT, CLOSE…), with the
    /// client in its fallback state if still usable.
    ///
    /// [`Error::ResponseTooLarge`] means the server answered OK: for it the
    /// state did change, while the client would go back to the old one.
    /// Both sides would disagree (EXPUNGE without a selected mailbox…), so
    /// the connection is dropped, as for a fatal error.
    pub(crate) fn new(error: Error, client: C) -> Self {
        let usable = !error.is_fatal() && !matches!(error, Error::ResponseTooLarge(_));
        let client = usable.then_some(client);
        Self(Box::new(StateErrorInner { error, client }))
    }

    /// What went wrong.
    #[must_use]
    pub fn error(&self) -> &Error {
        &self.0.error
    }

    /// Whether the connection came back usable.
    #[must_use]
    pub fn has_client(&self) -> bool {
        self.0.client.is_some()
    }

    /// The error and, if still usable, the connection.
    #[must_use]
    pub fn into_parts(self) -> (Error, Option<C>) {
        let inner = *self.0;
        (inner.error, inner.client)
    }
}

impl<C> fmt::Debug for StateError<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateError")
            .field("error", &self.0.error)
            .field("usable", &self.0.client.is_some())
            .finish()
    }
}

impl<C> fmt::Display for StateError<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0.error, f)
    }
}

impl<C> std::error::Error for StateError<C> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0.error)
    }
}

impl<C> From<StateError<C>> for Error {
    fn from(error: StateError<C>) -> Self {
        error.into_parts().0
    }
}
