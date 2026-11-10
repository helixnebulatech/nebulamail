// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JMAP ids (RFC 8620 §1.2) and one newtype per kind of id, so a mailbox id
//! can't be passed where an email id is expected.

use std::fmt;
use std::str::FromStr;

use crate::macros::serde_via_str;

/// A JMAP `Id` (RFC 8620 §1.2): 1 to 255 characters from the URL-safe
/// base64 alphabet (`A-Z a-z 0-9 - _`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(String);

/// Why a string is not a valid [`Id`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IdError {
    /// The string is empty.
    #[error("id is empty")]
    Empty,
    /// The string is longer than 255 bytes.
    #[error("id is longer than 255 characters")]
    TooLong,
    /// The string contains a character outside `A-Z a-z 0-9 - _`.
    #[error("id contains a character that is not allowed")]
    InvalidCharacter,
}

impl Id {
    /// Longest id allowed by RFC 8620 §1.2.
    pub const MAX_LEN: usize = 255;

    /// The id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Id {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(IdError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(IdError::TooLong);
        }
        if !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(IdError::InvalidCharacter);
        }
        Ok(Self(s.to_owned()))
    }
}

impl TryFrom<String> for Id {
    type Error = IdError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Id").field(&self.0).finish()
    }
}

serde_via_str!(Id);

/// An id of one particular kind of object (`MailboxId`, `EmailId`…).
///
/// All kinds wrap an [`Id`]; converting between kinds is explicit
/// (`MailboxId::from(id)`), never implicit.
pub trait IdKind:
    Clone
    + Ord
    + From<Id>
    + AsRef<Id>
    + fmt::Display
    + fmt::Debug
    + FromStr<Err = IdError>
    + serde::Serialize
    + serde::de::DeserializeOwned
{
    /// The plain id.
    fn as_id(&self) -> &Id {
        self.as_ref()
    }
}

macro_rules! id_kind {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Id);

        impl $name {
            /// The id as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }

            /// The plain id.
            #[must_use]
            pub fn into_id(self) -> Id {
                self.0
            }
        }

        impl From<Id> for $name {
            fn from(id: Id) -> Self {
                Self(id)
            }
        }

        impl From<$name> for Id {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl AsRef<Id> for $name {
            fn as_ref(&self) -> &Id {
                &self.0
            }
        }

        impl IdKind for $name {}

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0.as_str())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.0.as_str()).finish()
            }
        }

        serde_via_str!($name);
    };
}

id_kind!(
    /// The id of an account (RFC 8620 §1.6.2).
    AccountId
);
id_kind!(
    /// The id of a Mailbox (RFC 8621 §2).
    MailboxId
);
id_kind!(
    /// The id of an Email (RFC 8621 §4).
    EmailId
);
id_kind!(
    /// The id of a Thread (RFC 8621 §3).
    ThreadId
);
id_kind!(
    /// The id of a blob: raw message, attachment, upload (RFC 8620 §6).
    BlobId
);
id_kind!(
    /// The id of an Identity (RFC 8621 §6).
    IdentityId
);
id_kind!(
    /// The id of an `EmailSubmission` (RFC 8621 §7).
    EmailSubmissionId
);
id_kind!(
    /// The id of a Quota (RFC 9425).
    QuotaId
);
id_kind!(
    /// The id of a `SieveScript` (RFC 9661).
    SieveScriptId
);
id_kind!(
    /// The id of a `VacationResponse` (always `singleton`, RFC 8621 §8).
    VacationResponseId
);
id_kind!(
    /// The id of a Calendar (draft-ietf-jmap-calendars).
    CalendarId
);
id_kind!(
    /// The id of a `CalendarEvent`, or of one occurrence of a recurring
    /// event when the query expands recurrences.
    CalendarEventId
);
id_kind!(
    /// The id of a `ParticipantIdentity` (draft-ietf-jmap-calendars).
    ParticipantIdentityId
);
id_kind!(
    /// The id of an `AddressBook` (RFC 9610).
    AddressBookId
);
id_kind!(
    /// The id of a `ContactCard` (RFC 9610).
    ContactCardId
);
id_kind!(
    /// The id of a Principal (a user or group), as in `shareWith`.
    PrincipalId
);
id_kind!(
    /// The id of a `PushSubscription` (RFC 8620 §7.2).
    PushSubscriptionId
);
id_kind!(
    /// A *creation id*: the temporary id a client gives to an object it
    /// creates in `/set` (RFC 8620 §5.3), referenced elsewhere as `#id`.
    CreationId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_ids() {
        for s in ["a", "Mf1a2b-_Z9", &"x".repeat(255)] {
            assert_eq!(s.parse::<Id>().unwrap().as_str(), s);
        }
    }

    #[test]
    fn rejects_invalid_ids() {
        assert_eq!("".parse::<Id>(), Err(IdError::Empty));
        assert_eq!("x".repeat(256).parse::<Id>(), Err(IdError::TooLong));
        for s in ["a b", "a/b", "a+b", "a=b", "ñ", "a\0", "#k1"] {
            assert_eq!(s.parse::<Id>(), Err(IdError::InvalidCharacter), "{s:?}");
        }
    }

    #[test]
    fn serde_round_trip() {
        let id: MailboxId = serde_json::from_str("\"m1\"").unwrap();
        assert_eq!(id.as_str(), "m1");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"m1\"");
        assert!(serde_json::from_str::<EmailId>("\"a b\"").is_err());
        assert!(serde_json::from_str::<EmailId>("1").is_err());
    }
}
