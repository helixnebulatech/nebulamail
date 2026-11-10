// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Errors of the groupware layer. None of them carries calendar or contact
//! content (titles, attendees, addresses never reach the logs), nor hrefs.

use nebula_crypto::CryptoError;
use nebula_dav::DavError;
use nebula_store::StoreError;

use crate::model::convert::Format;

/// Why an iCalendar object, a vCard, or their JSON form can't be converted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConvertError {
    /// Bigger than the limit for its format.
    #[error("{format} too large: {size} bytes (limit {limit})")]
    TooLarge {
        format: Format,
        size: usize,
        limit: usize,
    },
    /// iCalendar and vCard must be UTF-8 (the DAV client decodes first).
    #[error("{format} is not valid UTF-8")]
    NotUtf8 { format: Format },
    /// Not one well-formed object of that format.
    #[error("invalid {format}")]
    Invalid { format: Format },
    /// The JSON form can't be written back (missing or wrong properties).
    #[error("cannot convert {format} back")]
    Export { format: Format },
    /// calcard panicked on this input: treated as invalid, never retried.
    #[error("the {format} converter failed on this input")]
    Panicked { format: Format },
}

/// Errors of the sync and the service.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GroupwareError {
    /// The DAV server refused the account's login (401). Not retried until
    /// the user logs in again; mail keeps working.
    #[error("the DAV server refused the login")]
    Unauthorized,
    /// The DAV client failed (network, limits, a hostile answer…).
    #[error(transparent)]
    Dav(DavError),
    /// Database error.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Sealing or opening the local copy failed.
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    /// No discovery yet (the account is starting).
    #[error("calendars and contacts are not discovered yet")]
    NotDiscovered,
    /// A blocking task (conversion, sealing) panicked or was cancelled.
    #[error("background task failed")]
    Task,
    /// The resource or collection is not in the local copy (any more), or
    /// is not of the expected kind.
    #[error("not found")]
    NotFound,
    /// The server refused to set some collection properties (PROPPATCH or
    /// extended MKCOL): none were changed.
    #[error("the DAV server refused {} properties", .0.len())]
    PropertiesRefused(Vec<nebula_dav::proto::Property>),
    /// A resource without a strong etag, even after reading it again: it
    /// can't be changed safely (writes are always conditional
    /// and `If-Match` needs a strong etag, RFC 9110 §13.1.1).
    #[error("the DAV server gives no strong etag for this resource")]
    NoEtag,
    /// The local copy is not text (or does not open with the account's
    /// key): it is read again from the server.
    #[error("the local copy of a resource is unreadable")]
    Unreadable,
    /// A share can't be changed.
    #[error(transparent)]
    Share(crate::sharing::ShareError),
    /// The server does not offer this (publishing, subscriptions or the
    /// trash bin elsewhere than Nextcloud), or not for this calendar.
    #[error("the calendar server does not offer this")]
    Unsupported,
    /// A collection to delete only if empty has something in it on the
    /// server (even what the local copy doesn't show).
    #[error("the collection is not empty")]
    HasContents,
}

impl GroupwareError {
    /// Someone else changed or created the resource meanwhile (412): read
    /// it again and retry once.
    #[must_use]
    pub const fn is_conflict(&self) -> bool {
        matches!(self, Self::Dav(DavError::PreconditionFailed))
    }
}

impl From<DavError> for GroupwareError {
    fn from(error: DavError) -> Self {
        match error {
            DavError::Unauthorized => Self::Unauthorized,
            other => Self::Dav(other),
        }
    }
}
