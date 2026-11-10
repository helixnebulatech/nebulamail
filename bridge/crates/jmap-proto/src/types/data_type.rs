// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Data type names (`"Email"`, `"Mailbox"`…), used by push and quotas.

use std::fmt;
use std::str::FromStr;

use crate::macros::serde_via_str;

/// A JMAP data type name (RFC 8620 §1.6.1, §7.1).
///
/// Types this crate knows are variants; any other well-formed name (e.g.
/// `FileNode`, which a client may subscribe to even if the server doesn't
/// have it) is kept in [`DataType::Other`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DataType {
    /// `Core`.
    Core,
    /// `Mailbox`.
    Mailbox,
    /// `Thread`.
    Thread,
    /// `Email`.
    Email,
    /// `EmailDelivery`: new mail arrived (RFC 8621 §1.5).
    EmailDelivery,
    /// `SearchSnippet`.
    SearchSnippet,
    /// `Identity`.
    Identity,
    /// `EmailSubmission`.
    EmailSubmission,
    /// `VacationResponse`.
    VacationResponse,
    /// `Quota` (RFC 9425).
    Quota,
    /// `Calendar` (draft-ietf-jmap-calendars).
    Calendar,
    /// `CalendarEvent`.
    CalendarEvent,
    /// `ParticipantIdentity`.
    ParticipantIdentity,
    /// `CalendarAlert`: not a state, but the name a client puts in the
    /// push `types` to get alerts (draft-ietf-jmap-calendars §6.4).
    CalendarAlert,
    /// `AddressBook` (RFC 9610).
    AddressBook,
    /// `ContactCard` (RFC 9610).
    ContactCard,
    /// Another type name: letters, digits and `_`, 1 to 64 characters.
    Other(OtherDataType),
}

/// A data type name this crate doesn't know (only made by parsing).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OtherDataType(String);

impl OtherDataType {
    /// The type name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a string is not a data type name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid data type name")]
pub struct DataTypeError;

impl DataType {
    /// The type name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Core => "Core",
            Self::Mailbox => "Mailbox",
            Self::Thread => "Thread",
            Self::Email => "Email",
            Self::EmailDelivery => "EmailDelivery",
            Self::SearchSnippet => "SearchSnippet",
            Self::Identity => "Identity",
            Self::EmailSubmission => "EmailSubmission",
            Self::VacationResponse => "VacationResponse",
            Self::Quota => "Quota",
            Self::Calendar => "Calendar",
            Self::CalendarEvent => "CalendarEvent",
            Self::ParticipantIdentity => "ParticipantIdentity",
            Self::CalendarAlert => "CalendarAlert",
            Self::AddressBook => "AddressBook",
            Self::ContactCard => "ContactCard",
            Self::Other(name) => name.as_str(),
        }
    }
}

impl FromStr for DataType {
    type Err = DataTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Core" => Self::Core,
            "Mailbox" => Self::Mailbox,
            "Thread" => Self::Thread,
            "Email" => Self::Email,
            "EmailDelivery" => Self::EmailDelivery,
            "SearchSnippet" => Self::SearchSnippet,
            "Identity" => Self::Identity,
            "EmailSubmission" => Self::EmailSubmission,
            "VacationResponse" => Self::VacationResponse,
            "Quota" => Self::Quota,
            "Calendar" => Self::Calendar,
            "CalendarEvent" => Self::CalendarEvent,
            "ParticipantIdentity" => Self::ParticipantIdentity,
            "CalendarAlert" => Self::CalendarAlert,
            "AddressBook" => Self::AddressBook,
            "ContactCard" => Self::ContactCard,
            other => {
                let valid = (1..=64).contains(&other.len())
                    && other
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if !valid {
                    return Err(DataTypeError);
                }
                Self::Other(OtherDataType(other.to_owned()))
            }
        })
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

serde_via_str!(DataType);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_other_types() {
        assert_eq!("Email".parse::<DataType>().unwrap(), DataType::Email);
        assert_eq!(
            "FileNode".parse::<DataType>().unwrap(),
            DataType::Other(OtherDataType("FileNode".to_owned()))
        );
        for (name, known) in [
            ("Calendar", DataType::Calendar),
            ("CalendarEvent", DataType::CalendarEvent),
            ("ParticipantIdentity", DataType::ParticipantIdentity),
            ("CalendarAlert", DataType::CalendarAlert),
            ("AddressBook", DataType::AddressBook),
            ("ContactCard", DataType::ContactCard),
        ] {
            let parsed: DataType = name.parse().unwrap();
            assert_eq!(parsed, known);
            assert_eq!(parsed.as_str(), name);
        }
        for bad in ["", "a b", "a,b", "*", &"x".repeat(65)] {
            assert!(bad.parse::<DataType>().is_err(), "{bad}");
        }
    }
}
