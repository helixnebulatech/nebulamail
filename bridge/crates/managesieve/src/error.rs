// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Errors of the ManageSieve client.

use std::fmt;

use nebula_managesieve_proto::{DecodeError, ResponseCode, one_line};

/// The server answered a command with `NO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The command (`PUTSCRIPT`, `SETACTIVE`…).
    pub command: &'static str,
    /// The response code, if any (`NONEXISTENT`, `QUOTA/MAXSIZE`…).
    pub code: Option<ResponseCode>,
    /// The server's text, cleaned but with its line breaks (for showing
    /// to the user). The `Display` puts it on one line, for logs.
    pub text: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed: {}", self.command, one_line(&self.text))
    }
}

/// Errors of the ManageSieve client.
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
    /// Certificate checks can only be turned off for loopback peers.
    #[error("TLS verification can only be disabled for loopback servers")]
    InsecureTlsNotLoopback,
    /// `net-guard` refused the destination or could not connect.
    #[error("connection refused by the network guard: {0}")]
    Guard(#[from] net_guard::GuardError),
    /// No answer in time.
    #[error("timed out during {0}")]
    Timeout(&'static str),
    /// The server closed the connection.
    #[error("the server closed the connection")]
    ConnectionClosed,
    /// The server said BYE (its text on one line).
    #[error("the server closed the connection: {0}")]
    Bye(String),
    /// A line broke the grammar or the size limits.
    #[error("response rejected: {0}")]
    Decode(#[from] DecodeError),
    /// The server broke the protocol (an unexpected line).
    #[error("protocol error: {0}")]
    Protocol(&'static str),
    /// STARTTLS mode, but the server does not offer it (or refused it). We
    /// never go on in plaintext.
    #[error("the server does not offer STARTTLS")]
    StartTlsUnavailable,
    /// Data arrived together with the STARTTLS answer, before TLS started:
    /// a man in the middle may have injected it.
    #[error("unexpected data before the TLS handshake")]
    InjectedBeforeTls,
    /// No authentication mechanism we can use.
    #[error("no usable authentication mechanism")]
    NoAuthMechanism,
    /// Wrong user name or password (the server's text on one line).
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    /// The server refused a command.
    #[error("{0}")]
    Refused(Refusal),
    /// The server lacks something we need.
    #[error("unsupported by the server: {0}")]
    Unsupported(&'static str),
    /// An earlier fatal error left the connection unusable.
    #[error("the connection is broken by an earlier error")]
    Broken,
}

impl Error {
    /// After this error the connection is out of sync or closed.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        !matches!(
            self,
            Self::AuthFailed(_) | Self::Refused(_) | Self::Unsupported(_) | Self::NoAuthMechanism
        )
    }

    /// The response code of a refusal.
    #[must_use]
    pub fn code(&self) -> Option<&ResponseCode> {
        match self {
            Self::Refused(refusal) => refusal.code.as_ref(),
            _ => None,
        }
    }
}
