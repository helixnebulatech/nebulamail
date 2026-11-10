// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JMAP primitive types (RFC 8620 §1): ids, integers, dates, states,
//! keywords.

mod data_type;
mod date;
mod id;
mod int;
mod jscalendar;
mod keyword;
pub(crate) mod serde_util;
mod set;
mod state;

pub use data_type::{DataType, DataTypeError, OtherDataType};
pub use date::{Date, DateError, UtcDate};
pub use id::{
    AccountId, AddressBookId, BlobId, CalendarEventId, CalendarId, ContactCardId, CreationId,
    EmailId, EmailSubmissionId, Id, IdError, IdKind, IdentityId, MailboxId, ParticipantIdentityId,
    PrincipalId, PushSubscriptionId, QuotaId, SieveScriptId, ThreadId, VacationResponseId,
};
pub use int::{Int, IntError, PositiveInt, UnsignedInt};
pub use jscalendar::{Duration, JsCalendarValueError, LocalDateTime, TimeZoneId};
pub use keyword::{Keyword, KeywordError};
pub use set::TrueSet;
pub use state::{CallId, State};

/// A string that is not one of the values a closed JMAP enum allows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind}: {value:?}")]
pub struct UnknownValue {
    kind: &'static str,
    value: String,
}

impl UnknownValue {
    /// Longest part of the rejected value kept for the message, so a huge
    /// string from a client is not copied into every error.
    const MAX_ECHO: usize = 64;

    pub(crate) fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.chars().take(Self::MAX_ECHO).collect(),
        }
    }

    /// What was being parsed (e.g. `"mailbox role"`).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The rejected value (cut to 64 characters).
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}
