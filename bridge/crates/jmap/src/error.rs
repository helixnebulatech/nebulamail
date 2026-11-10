// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Errors. [`JmapError`] is what the crate's public API returns; inside a
//! method call, [`CallError`] collects every failure and becomes a JMAP
//! method-level error, and the per-object helpers turn mail and store
//! errors into `SetError`s.
//!
//! None of them carries message content or credentials. Server texts from
//! IMAP refusals may be shown to the user (the `mail` crate cleans them).

use nebula_jmap_proto::{MethodError, MethodErrorType, SetError, SetErrorType};
use nebula_mail::{MailError, Unavailable};
use nebula_smtp::SmtpError;
use nebula_store::StoreError;

/// Errors of the public API (building the backend).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JmapError {
    /// The SMTP settings can't work (TLS setup, host name).
    #[error("SMTP configuration: {0}")]
    Smtp(#[from] SmtpError),
}

/// Something that can't happen unless the code or the database is wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("internal error: {0}")]
pub(crate) struct Bug(pub(crate) &'static str);

/// Why a method call failed as a whole.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CallError {
    /// A JMAP error for the client (`invalidArguments`…).
    #[error("{0}")]
    Method(MethodError),
    /// The database failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The mail server or the mail layer failed.
    #[error(transparent)]
    Mail(#[from] MailError),
    /// The ManageSieve server or the connection to it failed.
    #[error(transparent)]
    Sieve(nebula_managesieve::Error),
    /// A bug.
    #[error(transparent)]
    Bug(#[from] Bug),
}

impl From<MethodError> for CallError {
    fn from(error: MethodError) -> Self {
        Self::Method(error)
    }
}

impl From<MethodErrorType> for CallError {
    fn from(kind: MethodErrorType) -> Self {
        Self::Method(MethodError::new(kind))
    }
}

impl CallError {
    /// The method-level error to answer with. Internal failures are logged
    /// here (without user data) and answered as `serverFail`.
    pub(crate) fn into_method_error(self) -> MethodError {
        match self {
            Self::Method(error) => error,
            Self::Mail(MailError::Unavailable(Unavailable::Busy)) => {
                tracing::warn!("bridge busy with other large messages");
                MethodError::new(MethodErrorType::ServerUnavailable).with_description(
                    "the server is busy with other large messages; try again later",
                )
            }
            Self::Mail(error) if error.is_retryable() => {
                tracing::warn!(error = %error, "mail server unavailable");
                MethodError::new(MethodErrorType::ServerUnavailable)
            }
            Self::Mail(MailError::AuthFailed) => {
                MethodError::new(MethodErrorType::ServerUnavailable)
                    .with_description("the mail server refused the login")
            }
            Self::Mail(
                MailError::NotFound(what) | MailError::Store(StoreError::NotFound(what)),
            )
            | Self::Store(StoreError::NotFound(what)) => {
                MethodError::server_fail(format!("{what} not found"))
            }
            Self::Mail(error) => {
                tracing::warn!(error = %error, "mail operation failed");
                MethodError::server_fail("mail server error")
            }
            Self::Sieve(nebula_managesieve::Error::AuthFailed(_)) => {
                MethodError::new(MethodErrorType::ServerUnavailable)
                    .with_description("the Sieve server refused the login")
            }
            Self::Sieve(error) if error.is_fatal() => {
                tracing::warn!(error = %error, "Sieve server unavailable");
                MethodError::new(MethodErrorType::ServerUnavailable)
                    .with_description("the Sieve server is unavailable; try again later")
            }
            Self::Sieve(error) => {
                tracing::warn!(error = %error, "Sieve operation failed");
                MethodError::server_fail("Sieve server error")
            }
            // A filter over the store's limits (`Email/query`): the
            // client's request, not a failure.
            Self::Store(StoreError::Invalid(why)) if why.starts_with("filter") => {
                MethodError::invalid_arguments(why)
            }
            Self::Store(error) => {
                tracing::error!(error = %error, "store failed");
                MethodError::server_fail("database error")
            }
            Self::Bug(bug) => {
                tracing::error!(error = %bug, "bug");
                MethodError::server_fail("internal error")
            }
        }
    }
}

/// The `SetError` for a mail-layer failure on one object.
pub(crate) fn mail_set_error(error: &MailError) -> SetError {
    match error {
        MailError::NotFound(_) | MailError::Store(StoreError::NotFound(_)) => SetError::not_found(),
        MailError::Invalid(why) => {
            SetError::new(SetErrorType::InvalidProperties).with_description(*why)
        }
        MailError::Unsupported(what) => SetError::new(SetErrorType::Forbidden)
            .with_description(format!("not supported by the mail server: {what}")),
        MailError::Refused(refusal) => SetError::new(SetErrorType::Forbidden)
            .with_description(format!("the mail server refused: {refusal}")),
        MailError::AuthFailed => SetError::new(SetErrorType::Forbidden)
            .with_description("the mail server refused the login"),
        MailError::MailboxHasEmail => SetError::new(SetErrorType::MailboxHasEmail),
        MailError::Unavailable(Unavailable::Busy) => busy_set_error(),
        other => {
            tracing::warn!(error = %other, "mail operation failed");
            SetError::new(SetErrorType::Forbidden)
                .with_description("the mail server is unavailable; try again later")
        }
    }
}

/// The `SetError` when the bridge's memory for whole messages is in use
/// (retryable, but `SetError` has no such type).
pub(crate) fn busy_set_error() -> SetError {
    SetError::new(SetErrorType::Forbidden)
        .with_description("the server is busy with other large messages; try again later")
}

/// The `SetError` for a store failure on one object.
pub(crate) fn store_set_error(error: &StoreError) -> SetError {
    match error {
        StoreError::NotFound(_) => SetError::not_found(),
        StoreError::Invalid(why) => {
            SetError::new(SetErrorType::InvalidProperties).with_description(*why)
        }
        StoreError::OverQuota(what) => {
            SetError::new(SetErrorType::OverQuota).with_description(format!("too many {what}"))
        }
        other => {
            tracing::error!(error = %other, "store failed");
            SetError::new(SetErrorType::Forbidden).with_description("database error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_errors() {
        let error = CallError::from(MethodError::invalid_arguments("x")).into_method_error();
        assert_eq!(error.kind(), MethodErrorType::InvalidArguments);
        let error = CallError::from(Bug("x")).into_method_error();
        assert_eq!(error.kind(), MethodErrorType::ServerFail);
        let error = CallError::from(MailError::AuthFailed).into_method_error();
        assert_eq!(error.kind(), MethodErrorType::ServerUnavailable);
        let error = CallError::from(MailError::Unavailable(Unavailable::Busy)).into_method_error();
        assert_eq!(error.kind(), MethodErrorType::ServerUnavailable);
        assert!(error.description().unwrap().contains("busy"));
        let error = CallError::from(StoreError::Invalid("filter too large")).into_method_error();
        assert_eq!(error.kind(), MethodErrorType::InvalidArguments);
        let error = CallError::from(StoreError::Invalid("upload size")).into_method_error();
        assert_eq!(error.kind(), MethodErrorType::ServerFail);
    }

    #[test]
    fn set_errors() {
        assert_eq!(
            mail_set_error(&MailError::NotFound("email")).kind(),
            SetErrorType::NotFound
        );
        assert_eq!(
            mail_set_error(&MailError::Invalid("name")).kind(),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            mail_set_error(&MailError::MailboxHasEmail).kind(),
            SetErrorType::MailboxHasEmail
        );
        let busy = mail_set_error(&MailError::Unavailable(Unavailable::Busy));
        assert_eq!(busy.kind(), SetErrorType::Forbidden);
        assert!(busy.description().unwrap().contains("try again later"));
        assert_eq!(
            store_set_error(&StoreError::NotFound("identity")).kind(),
            SetErrorType::NotFound
        );
        assert_eq!(
            store_set_error(&StoreError::OverQuota("identities")).kind(),
            SetErrorType::OverQuota
        );
    }
}
