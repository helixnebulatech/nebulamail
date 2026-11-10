// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Errors of the mail layer. None of them carries a password or message
//! content; server texts (`Refused`) may be shown to the user.

use nebula_imap::{Error as ImapError, Refusal};
use nebula_store::StoreError;
use net_guard::GuardError;

/// Why the server can't be used right now. Worth retrying later.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Unavailable {
    /// `net-guard` refused the address, or the TCP connection failed.
    #[error(transparent)]
    Connect(#[from] GuardError),
    /// IMAP failed: network, TLS, timeout, BYE, a broken connection…
    #[error(transparent)]
    Imap(ImapError),
    /// Recent connection attempts failed: waiting before the next one.
    #[error("waiting before reconnecting to the mail server")]
    Backoff,
    /// The account is closing: the sync stopped between two commands.
    #[error("the account is closing")]
    Stopping,
    /// A login with credentials that were replaced while it ran was
    /// refused: the next one uses the new credentials.
    #[error("the login credentials changed meanwhile")]
    CredentialsChanged,
    /// The memory for whole messages is in use by other large messages
    /// (or another big message of the account is being loaded): the
    /// bridge, not the server, is busy.
    #[error("the bridge is busy with other large messages; try again later")]
    Busy,
}

/// Errors of the mail layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MailError {
    /// The server refused the login: the password changed or was revoked.
    /// The session must end; nothing is retried with these credentials.
    #[error("the mail server refused the login")]
    AuthFailed,
    /// The server can't be reached or the connection broke. Retry later.
    #[error("the mail server is unavailable: {0}")]
    Unavailable(#[from] Unavailable),
    /// The email, mailbox or message is not there (any more).
    #[error("{0} not found")]
    NotFound(&'static str),
    /// The server (or this bridge) can't do it: missing extension, a
    /// keyword the mailbox can't store…
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    /// The server answered NO/BAD to an operation (name taken, over quota…).
    #[error("{0}")]
    Refused(Refusal),
    /// The mailbox still has messages on the server and the caller asked
    /// to keep them (JMAP `mailboxHasEmail`).
    #[error("the mailbox has emails")]
    MailboxHasEmail,
    /// The request itself is not valid (no mailbox left, bad name…).
    #[error("invalid request: {0}")]
    Invalid(&'static str),
    /// The bridge configuration can't work (bad TLS name, insecure TLS on a
    /// non-loopback server…).
    #[error("mail server configuration: {0}")]
    Config(ImapError),
    /// Database error.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl MailError {
    /// Whether trying again later may work.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }
}

impl From<ImapError> for MailError {
    fn from(error: ImapError) -> Self {
        match error {
            ImapError::AuthFailed { .. } => Self::AuthFailed,
            ImapError::Refused(refusal) => Self::Refused(refusal),
            ImapError::Unsupported(what) => Self::Unsupported(what),
            ImapError::NoAuthMechanism => Self::Unsupported("authentication mechanism"),
            ImapError::Encode(_) => Self::Invalid("value cannot be sent to the server"),
            error @ (ImapError::TlsConfig(_)
            | ImapError::InvalidServerName
            | ImapError::InsecureTlsNotLoopback
            | ImapError::StartTlsUnavailable) => Self::Config(error),
            other => Self::Unavailable(Unavailable::Imap(other)),
        }
    }
}

impl From<GuardError> for MailError {
    fn from(error: GuardError) -> Self {
        Self::Unavailable(Unavailable::Connect(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        let auth = ImapError::AuthFailed {
            code: None,
            text: "no".to_owned(),
        };
        assert!(matches!(MailError::from(auth), MailError::AuthFailed));
        let closed = MailError::from(ImapError::ConnectionClosed);
        assert!(closed.is_retryable());
        assert!(matches!(
            MailError::from(ImapError::Unsupported("MOVE")),
            MailError::Unsupported("MOVE")
        ));
        assert!(matches!(
            MailError::from(ImapError::InsecureTlsNotLoopback),
            MailError::Config(_)
        ));
        assert!(!MailError::NotFound("email").is_retryable());
    }
}
