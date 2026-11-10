// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-object errors of `/set`, `Email/import` (RFC 8620 §5.3, RFC 8621).

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::macros::string_enum;
use crate::types::{BlobId, Id, UnsignedInt};

string_enum! {
    /// The `type` of a [`SetError`].
    pub enum SetErrorType ("set error type") {
        /// Would violate an ACL or other policy.
        Forbidden = "forbidden",
        /// Would exceed a quota.
        OverQuota = "overQuota",
        /// The object would be too big.
        TooLarge = "tooLarge",
        /// Too many objects created recently.
        RateLimit = "rateLimit",
        /// The id to update or destroy does not exist.
        NotFound = "notFound",
        /// The `PatchObject` is not a valid patch.
        InvalidPatch = "invalidPatch",
        /// The object is also destroyed in this call, so the update was
        /// skipped.
        WillDestroy = "willDestroy",
        /// Some properties are invalid (listed in `properties`).
        InvalidProperties = "invalidProperties",
        /// A singleton can't be created or destroyed.
        Singleton = "singleton",
        /// A duplicate of an existing object (`existingId`).
        AlreadyExists = "alreadyExists",
        /// `Mailbox/set` destroy: the mailbox has child mailboxes.
        MailboxHasChild = "mailboxHasChild",
        /// `Mailbox/set` destroy: the mailbox has emails.
        MailboxHasEmail = "mailboxHasEmail",
        /// `Email/set` create: some blob ids don't exist (`notFound`).
        BlobNotFound = "blobNotFound",
        /// Too many keywords on the Email.
        TooManyKeywords = "tooManyKeywords",
        /// The Email would be in too many mailboxes.
        TooManyMailboxes = "tooManyMailboxes",
        /// `EmailSubmission/set`: the Email to send is invalid.
        InvalidEmail = "invalidEmail",
        /// More recipients than allowed (`maxRecipients`).
        TooManyRecipients = "tooManyRecipients",
        /// The envelope has no recipients.
        NoRecipients = "noRecipients",
        /// Some recipients are not valid (`invalidRecipients`).
        InvalidRecipients = "invalidRecipients",
        /// The envelope sender is not allowed.
        ForbiddenMailFrom = "forbiddenMailFrom",
        /// The `From` header is not allowed.
        ForbiddenFrom = "forbiddenFrom",
        /// The user may not send right now.
        ForbiddenToSend = "forbiddenToSend",
        /// `EmailSubmission/set` update: the message can't be unsent.
        CannotUnsend = "cannotUnsend",
        /// `MDN/send`: the Email already has `$mdnsent` (RFC 9007).
        MdnAlreadySent = "mdnAlreadySent",
        /// `Calendar/set` destroy: the calendar has events and
        /// `onDestroyRemoveEvents` is false (draft-ietf-jmap-calendars).
        CalendarHasEvent = "calendarHasEvent",
        /// `CalendarEvent/set` with `sendSchedulingMessages`: no way to
        /// reach some participant (draft-ietf-jmap-calendars).
        NoSupportedScheduleMethods = "noSupportedScheduleMethods",
        /// `AddressBook/set` destroy: the book has cards and
        /// `onDestroyRemoveContents` is false (RFC 9610 §2.3).
        AddressBookHasContents = "addressBookHasContents",
        /// `SieveScript/set` create/update and `/validate`: the script is
        /// not valid Sieve (RFC 9661 §2.4, §2.6; registered in §6.3.1).
        /// Stalwart 1.0.1 still sends `invalidScript`.
        InvalidSieve = "invalidSieve",
        /// `SieveScript/set` destroy: the script is the active one
        /// (RFC 9661 §2.4, §6.3.2). Stalwart says `scriptIsActive`.
        SieveIsActive = "sieveIsActive",
    }
}

/// A `SetError` (RFC 8620 §5.3): why one create, update or destroy failed.
///
/// The extra properties some types require are only set by their
/// constructors, so e.g. a `blobNotFound` always has its `notFound` list.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind}")]
pub struct SetError {
    kind: SetErrorType,
    description: Option<String>,
    extra: Extra,
}

/// The type-specific member of a [`SetError`], if any.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Extra {
    None,
    /// `properties` (`invalidProperties`, `invalidEmail`).
    Properties(Vec<String>),
    /// `existingId` (`alreadyExists`).
    ExistingId(Id),
    /// `notFound` (`blobNotFound`).
    NotFound(Vec<BlobId>),
    /// `maxRecipients` (`tooManyRecipients`).
    MaxRecipients(UnsignedInt),
    /// `invalidRecipients` (`invalidRecipients`).
    InvalidRecipients(Vec<String>),
    /// `maxSize` (`tooLarge` of a submission).
    MaxSize(UnsignedInt),
}

impl SetError {
    /// An error of this type with no extra properties.
    #[must_use]
    pub fn new(kind: SetErrorType) -> Self {
        Self {
            kind,
            description: None,
            extra: Extra::None,
        }
    }

    fn with_extra(kind: SetErrorType, extra: Extra) -> Self {
        Self {
            kind,
            description: None,
            extra,
        }
    }

    /// Adds a description for developers.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// `invalidProperties` listing every invalid property.
    #[must_use]
    pub fn invalid_properties<I, P>(properties: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        let list = properties.into_iter().map(Into::into).collect();
        Self::with_extra(SetErrorType::InvalidProperties, Extra::Properties(list))
    }

    /// `invalidPatch` with a description.
    #[must_use]
    pub fn invalid_patch(description: impl Into<String>) -> Self {
        Self::new(SetErrorType::InvalidPatch).with_description(description)
    }

    /// `invalidEmail` listing the invalid Email properties.
    #[must_use]
    pub fn invalid_email<I, P>(properties: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        let list = properties.into_iter().map(Into::into).collect();
        Self::with_extra(SetErrorType::InvalidEmail, Extra::Properties(list))
    }

    /// `alreadyExists` with the id of the existing object.
    #[must_use]
    pub fn already_exists(existing_id: Id) -> Self {
        Self::with_extra(SetErrorType::AlreadyExists, Extra::ExistingId(existing_id))
    }

    /// `blobNotFound` with every blob id that was not found.
    #[must_use]
    pub fn blob_not_found(not_found: Vec<BlobId>) -> Self {
        Self::with_extra(SetErrorType::BlobNotFound, Extra::NotFound(not_found))
    }

    /// `tooManyRecipients` with the maximum allowed.
    #[must_use]
    pub fn too_many_recipients(max_recipients: UnsignedInt) -> Self {
        Self::with_extra(
            SetErrorType::TooManyRecipients,
            Extra::MaxRecipients(max_recipients),
        )
    }

    /// `invalidRecipients` with the invalid addresses.
    #[must_use]
    pub fn invalid_recipients(invalid_recipients: Vec<String>) -> Self {
        Self::with_extra(
            SetErrorType::InvalidRecipients,
            Extra::InvalidRecipients(invalid_recipients),
        )
    }

    /// `tooLarge` for a submission, with the largest message size allowed
    /// (RFC 8621 §7.5).
    #[must_use]
    pub fn too_large_to_send(max_size: UnsignedInt) -> Self {
        Self::with_extra(SetErrorType::TooLarge, Extra::MaxSize(max_size))
    }

    /// `notFound`.
    #[must_use]
    pub fn not_found() -> Self {
        Self::new(SetErrorType::NotFound)
    }

    /// The error type.
    #[must_use]
    pub fn kind(&self) -> SetErrorType {
        self.kind
    }

    /// The description, if any.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The invalid properties (`invalidProperties`, `invalidEmail`).
    #[must_use]
    pub fn properties(&self) -> &[String] {
        match &self.extra {
            Extra::Properties(list) => list,
            Extra::None
            | Extra::ExistingId(_)
            | Extra::NotFound(_)
            | Extra::MaxRecipients(_)
            | Extra::InvalidRecipients(_)
            | Extra::MaxSize(_) => &[],
        }
    }
}

impl From<SetErrorType> for SetError {
    fn from(kind: SetErrorType) -> Self {
        Self::new(kind)
    }
}

impl Serialize for SetError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", &self.kind)?;
        if let Some(description) = &self.description {
            map.serialize_entry("description", description)?;
        }
        match &self.extra {
            Extra::None => {}
            Extra::Properties(list) => map.serialize_entry("properties", list)?,
            Extra::ExistingId(id) => map.serialize_entry("existingId", id)?,
            Extra::NotFound(ids) => map.serialize_entry("notFound", ids)?,
            Extra::MaxRecipients(max) => map.serialize_entry("maxRecipients", max)?,
            Extra::InvalidRecipients(list) => map.serialize_entry("invalidRecipients", list)?,
            Extra::MaxSize(max) => map.serialize_entry("maxSize", max)?,
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_shapes() {
        let cases = [
            (SetError::not_found(), json!({"type": "notFound"})),
            (
                SetError::invalid_properties(["mailboxIds"]).with_description("empty"),
                json!({"type": "invalidProperties", "description": "empty", "properties": ["mailboxIds"]}),
            ),
            (
                SetError::already_exists("e1".parse().unwrap()),
                json!({"type": "alreadyExists", "existingId": "e1"}),
            ),
            (
                SetError::blob_not_found(vec!["b1".parse().unwrap()]),
                json!({"type": "blobNotFound", "notFound": ["b1"]}),
            ),
            (
                SetError::too_many_recipients(UnsignedInt::from(50_u32)),
                json!({"type": "tooManyRecipients", "maxRecipients": 50}),
            ),
            (
                SetError::invalid_recipients(vec!["x@".to_owned()]),
                json!({"type": "invalidRecipients", "invalidRecipients": ["x@"]}),
            ),
            (
                SetError::too_large_to_send(UnsignedInt::from(1000_u32)),
                json!({"type": "tooLarge", "maxSize": 1000}),
            ),
            (
                SetError::invalid_email(["to"]),
                json!({"type": "invalidEmail", "properties": ["to"]}),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(serde_json::to_value(&error).unwrap(), expected);
        }
        for kind in SetErrorType::ALL {
            assert_eq!(
                serde_json::to_value(SetError::new(*kind)).unwrap()["type"],
                kind.as_str()
            );
        }
    }

    /// The names IANA registered for RFC 9661 (§6.3), not Stalwart's.
    #[test]
    fn sieve_error_names() {
        assert_eq!(SetErrorType::InvalidSieve.as_str(), "invalidSieve");
        assert_eq!(SetErrorType::SieveIsActive.as_str(), "sieveIsActive");
    }
}
