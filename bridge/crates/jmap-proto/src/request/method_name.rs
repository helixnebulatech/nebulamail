// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Method names (`"Email/get"`…).

use std::fmt;
use std::str::FromStr;

use super::Capability;
use crate::macros::serde_via_str;

/// The methods the bridge implements (mail, calendars,
/// contacts and principals), plus
/// [`MethodName::Unknown`] for anything else (answered with
/// `unknownMethod`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MethodName {
    /// `Core/echo`.
    CoreEcho,
    /// `Mailbox/get`.
    MailboxGet,
    /// `Mailbox/changes`.
    MailboxChanges,
    /// `Mailbox/query`.
    MailboxQuery,
    /// `Mailbox/queryChanges` (RFC 8621 §2.4).
    MailboxQueryChanges,
    /// `Mailbox/set`.
    MailboxSet,
    /// `Email/get`.
    EmailGet,
    /// `Email/changes`.
    EmailChanges,
    /// `Email/query`.
    EmailQuery,
    /// `Email/queryChanges`.
    EmailQueryChanges,
    /// `Email/set`.
    EmailSet,
    /// `Email/import`.
    EmailImport,
    /// `Email/copy` (RFC 8621 §4.7).
    EmailCopy,
    /// `Email/parse` (RFC 8621 §4.9).
    EmailParse,
    /// `Blob/copy` (RFC 8620 §6.3).
    BlobCopy,
    /// `Thread/get`.
    ThreadGet,
    /// `Thread/changes`.
    ThreadChanges,
    /// `SearchSnippet/get`.
    SearchSnippetGet,
    /// `Identity/get`.
    IdentityGet,
    /// `Identity/changes`.
    IdentityChanges,
    /// `Identity/set`.
    IdentitySet,
    /// `EmailSubmission/get`.
    EmailSubmissionGet,
    /// `EmailSubmission/query`.
    EmailSubmissionQuery,
    /// `EmailSubmission/queryChanges`.
    EmailSubmissionQueryChanges,
    /// `EmailSubmission/set`.
    EmailSubmissionSet,
    /// `Quota/get` (RFC 9425).
    QuotaGet,
    /// `Quota/changes` (RFC 9425).
    QuotaChanges,
    /// `Quota/query` (RFC 9425).
    QuotaQuery,
    /// `Quota/queryChanges` (RFC 9425 §4.5).
    QuotaQueryChanges,
    /// `MDN/send` (RFC 9007).
    MdnSend,
    /// `MDN/parse` (RFC 9007).
    MdnParse,
    /// `Calendar/get`.
    CalendarGet,
    /// `Calendar/changes`.
    CalendarChanges,
    /// `Calendar/set`.
    CalendarSet,
    /// `CalendarEvent/get`.
    CalendarEventGet,
    /// `CalendarEvent/changes`.
    CalendarEventChanges,
    /// `CalendarEvent/query`.
    CalendarEventQuery,
    /// `CalendarEvent/queryChanges`.
    CalendarEventQueryChanges,
    /// `CalendarEvent/set`.
    CalendarEventSet,
    /// `CalendarEvent/parse`.
    CalendarEventParse,
    /// `CalendarEvent/counter` (the bridge's own).
    CalendarEventCounter,
    /// `CalendarEvent/declineCounter` (the bridge's own).
    CalendarEventDeclineCounter,
    /// `CalendarTrash/get` (the bridge's own: Nextcloud's trash bin).
    CalendarTrashGet,
    /// `CalendarTrash/set` (the bridge's own).
    CalendarTrashSet,
    /// `ParticipantIdentity/get`.
    ParticipantIdentityGet,
    /// `AddressBook/get`.
    AddressBookGet,
    /// `AddressBook/changes`.
    AddressBookChanges,
    /// `AddressBook/set`.
    AddressBookSet,
    /// `ContactCard/get`.
    ContactCardGet,
    /// `ContactCard/changes`.
    ContactCardChanges,
    /// `ContactCard/query`.
    ContactCardQuery,
    /// `ContactCard/queryChanges`.
    ContactCardQueryChanges,
    /// `ContactCard/set`.
    ContactCardSet,
    /// `ContactCard/parse`.
    ContactCardParse,
    /// `Principal/get` (RFC 9670).
    PrincipalGet,
    /// `Principal/query` (RFC 9670).
    PrincipalQuery,
    /// `Principal/changes` (RFC 9670).
    PrincipalChanges,
    /// `Principal/getAvailability` (draft-ietf-jmap-calendars).
    PrincipalGetAvailability,
    /// `SieveScript/get` (RFC 9661).
    SieveScriptGet,
    /// `SieveScript/changes` (RFC 9661).
    SieveScriptChanges,
    /// `SieveScript/query` (RFC 9661).
    SieveScriptQuery,
    /// `SieveScript/set` (RFC 9661).
    SieveScriptSet,
    /// `SieveScript/validate` (RFC 9661).
    SieveScriptValidate,
    /// `VacationResponse/get` (RFC 8621 §8).
    VacationResponseGet,
    /// `VacationResponse/set` (RFC 8621 §8).
    VacationResponseSet,
    /// `PushSubscription/get` (RFC 8620 §7.2.1).
    PushSubscriptionGet,
    /// `PushSubscription/set` (RFC 8620 §7.2.2).
    PushSubscriptionSet,
    /// Any other name, kept as sent.
    Unknown(UnknownMethodName),
}

/// A method name this crate doesn't know (only made by parsing).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnknownMethodName(String);

impl UnknownMethodName {
    /// The name as sent.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl MethodName {
    /// The method name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::CoreEcho => "Core/echo",
            Self::MailboxGet => "Mailbox/get",
            Self::MailboxChanges => "Mailbox/changes",
            Self::MailboxQuery => "Mailbox/query",
            Self::MailboxQueryChanges => "Mailbox/queryChanges",
            Self::MailboxSet => "Mailbox/set",
            Self::EmailGet => "Email/get",
            Self::EmailChanges => "Email/changes",
            Self::EmailQuery => "Email/query",
            Self::EmailQueryChanges => "Email/queryChanges",
            Self::EmailSet => "Email/set",
            Self::EmailImport => "Email/import",
            Self::EmailCopy => "Email/copy",
            Self::EmailParse => "Email/parse",
            Self::BlobCopy => "Blob/copy",
            Self::ThreadGet => "Thread/get",
            Self::ThreadChanges => "Thread/changes",
            Self::SearchSnippetGet => "SearchSnippet/get",
            Self::IdentityGet => "Identity/get",
            Self::IdentityChanges => "Identity/changes",
            Self::IdentitySet => "Identity/set",
            Self::EmailSubmissionGet => "EmailSubmission/get",
            Self::EmailSubmissionQuery => "EmailSubmission/query",
            Self::EmailSubmissionQueryChanges => "EmailSubmission/queryChanges",
            Self::EmailSubmissionSet => "EmailSubmission/set",
            Self::QuotaGet => "Quota/get",
            Self::QuotaChanges => "Quota/changes",
            Self::QuotaQuery => "Quota/query",
            Self::QuotaQueryChanges => "Quota/queryChanges",
            Self::MdnSend => "MDN/send",
            Self::MdnParse => "MDN/parse",
            Self::CalendarGet => "Calendar/get",
            Self::CalendarChanges => "Calendar/changes",
            Self::CalendarSet => "Calendar/set",
            Self::CalendarEventGet => "CalendarEvent/get",
            Self::CalendarEventChanges => "CalendarEvent/changes",
            Self::CalendarEventQuery => "CalendarEvent/query",
            Self::CalendarEventQueryChanges => "CalendarEvent/queryChanges",
            Self::CalendarEventSet => "CalendarEvent/set",
            Self::CalendarEventParse => "CalendarEvent/parse",
            Self::CalendarEventCounter => "CalendarEvent/counter",
            Self::CalendarEventDeclineCounter => "CalendarEvent/declineCounter",
            Self::CalendarTrashGet => "CalendarTrash/get",
            Self::CalendarTrashSet => "CalendarTrash/set",
            Self::ParticipantIdentityGet => "ParticipantIdentity/get",
            Self::AddressBookGet => "AddressBook/get",
            Self::AddressBookChanges => "AddressBook/changes",
            Self::AddressBookSet => "AddressBook/set",
            Self::ContactCardGet => "ContactCard/get",
            Self::ContactCardChanges => "ContactCard/changes",
            Self::ContactCardQuery => "ContactCard/query",
            Self::ContactCardQueryChanges => "ContactCard/queryChanges",
            Self::ContactCardSet => "ContactCard/set",
            Self::ContactCardParse => "ContactCard/parse",
            Self::PrincipalGet => "Principal/get",
            Self::PrincipalQuery => "Principal/query",
            Self::PrincipalChanges => "Principal/changes",
            Self::PrincipalGetAvailability => "Principal/getAvailability",
            Self::SieveScriptGet => "SieveScript/get",
            Self::SieveScriptChanges => "SieveScript/changes",
            Self::SieveScriptQuery => "SieveScript/query",
            Self::SieveScriptSet => "SieveScript/set",
            Self::SieveScriptValidate => "SieveScript/validate",
            Self::VacationResponseGet => "VacationResponse/get",
            Self::VacationResponseSet => "VacationResponse/set",
            Self::PushSubscriptionGet => "PushSubscription/get",
            Self::PushSubscriptionSet => "PushSubscription/set",
            Self::Unknown(name) => name.as_str(),
        }
    }

    /// The capability that must be in `using` for this method, or `None`
    /// for unknown methods. For `CalendarEvent/parse` and
    /// `ContactCard/parse` it is the `:parse` one (clients send both).
    #[must_use]
    pub fn capability(&self) -> Option<Capability> {
        Some(match self {
            Self::CoreEcho
            | Self::BlobCopy
            | Self::PushSubscriptionGet
            | Self::PushSubscriptionSet => Capability::Core,
            Self::MailboxGet
            | Self::MailboxChanges
            | Self::MailboxQuery
            | Self::MailboxQueryChanges
            | Self::MailboxSet
            | Self::EmailGet
            | Self::EmailChanges
            | Self::EmailQuery
            | Self::EmailQueryChanges
            | Self::EmailSet
            | Self::EmailImport
            | Self::EmailCopy
            | Self::EmailParse
            | Self::ThreadGet
            | Self::ThreadChanges
            | Self::SearchSnippetGet => Capability::Mail,
            Self::IdentityGet
            | Self::IdentityChanges
            | Self::IdentitySet
            | Self::EmailSubmissionGet
            | Self::EmailSubmissionQuery
            | Self::EmailSubmissionQueryChanges
            | Self::EmailSubmissionSet => Capability::Submission,
            Self::QuotaGet | Self::QuotaChanges | Self::QuotaQuery | Self::QuotaQueryChanges => {
                Capability::Quota
            }
            Self::MdnSend | Self::MdnParse => Capability::Mdn,
            Self::SieveScriptGet
            | Self::SieveScriptChanges
            | Self::SieveScriptQuery
            | Self::SieveScriptSet
            | Self::SieveScriptValidate => Capability::Sieve,
            Self::VacationResponseGet | Self::VacationResponseSet => Capability::VacationResponse,
            Self::CalendarGet
            | Self::CalendarChanges
            | Self::CalendarSet
            | Self::CalendarEventGet
            | Self::CalendarEventChanges
            | Self::CalendarEventQuery
            | Self::CalendarEventQueryChanges
            | Self::CalendarEventSet
            | Self::ParticipantIdentityGet => Capability::Calendars,
            Self::CalendarEventParse => Capability::CalendarsParse,
            Self::CalendarEventCounter
            | Self::CalendarEventDeclineCounter
            | Self::CalendarTrashGet
            | Self::CalendarTrashSet => Capability::Bridge,
            Self::AddressBookGet
            | Self::AddressBookChanges
            | Self::AddressBookSet
            | Self::ContactCardGet
            | Self::ContactCardChanges
            | Self::ContactCardQuery
            | Self::ContactCardQueryChanges
            | Self::ContactCardSet => Capability::Contacts,
            Self::ContactCardParse => Capability::ContactsParse,
            Self::PrincipalGet | Self::PrincipalQuery | Self::PrincipalChanges => {
                Capability::Principals
            }
            Self::PrincipalGetAvailability => Capability::PrincipalsAvailability,
            Self::Unknown(_) => return None,
        })
    }
}

/// Never fails: unknown names become [`MethodName::Unknown`].
impl FromStr for MethodName {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Core/echo" => Self::CoreEcho,
            "Mailbox/get" => Self::MailboxGet,
            "Mailbox/changes" => Self::MailboxChanges,
            "Mailbox/query" => Self::MailboxQuery,
            "Mailbox/queryChanges" => Self::MailboxQueryChanges,
            "Mailbox/set" => Self::MailboxSet,
            "Email/get" => Self::EmailGet,
            "Email/changes" => Self::EmailChanges,
            "Email/query" => Self::EmailQuery,
            "Email/queryChanges" => Self::EmailQueryChanges,
            "Email/set" => Self::EmailSet,
            "Email/import" => Self::EmailImport,
            "Email/copy" => Self::EmailCopy,
            "Email/parse" => Self::EmailParse,
            "Blob/copy" => Self::BlobCopy,
            "Thread/get" => Self::ThreadGet,
            "Thread/changes" => Self::ThreadChanges,
            "SearchSnippet/get" => Self::SearchSnippetGet,
            "Identity/get" => Self::IdentityGet,
            "Identity/changes" => Self::IdentityChanges,
            "Identity/set" => Self::IdentitySet,
            "EmailSubmission/get" => Self::EmailSubmissionGet,
            "EmailSubmission/query" => Self::EmailSubmissionQuery,
            "EmailSubmission/queryChanges" => Self::EmailSubmissionQueryChanges,
            "EmailSubmission/set" => Self::EmailSubmissionSet,
            "Quota/get" => Self::QuotaGet,
            "Quota/changes" => Self::QuotaChanges,
            "Quota/query" => Self::QuotaQuery,
            "Quota/queryChanges" => Self::QuotaQueryChanges,
            "MDN/send" => Self::MdnSend,
            "MDN/parse" => Self::MdnParse,
            "SieveScript/get" => Self::SieveScriptGet,
            "SieveScript/changes" => Self::SieveScriptChanges,
            "SieveScript/query" => Self::SieveScriptQuery,
            "SieveScript/set" => Self::SieveScriptSet,
            "SieveScript/validate" => Self::SieveScriptValidate,
            "VacationResponse/get" => Self::VacationResponseGet,
            "VacationResponse/set" => Self::VacationResponseSet,
            "Calendar/get" => Self::CalendarGet,
            "Calendar/changes" => Self::CalendarChanges,
            "Calendar/set" => Self::CalendarSet,
            "CalendarEvent/get" => Self::CalendarEventGet,
            "CalendarEvent/changes" => Self::CalendarEventChanges,
            "CalendarEvent/query" => Self::CalendarEventQuery,
            "CalendarEvent/queryChanges" => Self::CalendarEventQueryChanges,
            "CalendarEvent/set" => Self::CalendarEventSet,
            "CalendarEvent/parse" => Self::CalendarEventParse,
            "CalendarEvent/counter" => Self::CalendarEventCounter,
            "CalendarEvent/declineCounter" => Self::CalendarEventDeclineCounter,
            "CalendarTrash/get" => Self::CalendarTrashGet,
            "CalendarTrash/set" => Self::CalendarTrashSet,
            "ParticipantIdentity/get" => Self::ParticipantIdentityGet,
            "AddressBook/get" => Self::AddressBookGet,
            "AddressBook/changes" => Self::AddressBookChanges,
            "AddressBook/set" => Self::AddressBookSet,
            "ContactCard/get" => Self::ContactCardGet,
            "ContactCard/changes" => Self::ContactCardChanges,
            "ContactCard/query" => Self::ContactCardQuery,
            "ContactCard/queryChanges" => Self::ContactCardQueryChanges,
            "ContactCard/set" => Self::ContactCardSet,
            "ContactCard/parse" => Self::ContactCardParse,
            "Principal/get" => Self::PrincipalGet,
            "Principal/query" => Self::PrincipalQuery,
            "Principal/changes" => Self::PrincipalChanges,
            "Principal/getAvailability" => Self::PrincipalGetAvailability,
            "PushSubscription/get" => Self::PushSubscriptionGet,
            "PushSubscription/set" => Self::PushSubscriptionSet,
            other => Self::Unknown(UnknownMethodName(other.to_owned())),
        })
    }
}

impl fmt::Display for MethodName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

serde_via_str!(MethodName);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown() {
        let name: MethodName = "Email/get".parse().unwrap();
        assert_eq!(name, MethodName::EmailGet);
        assert_eq!(name.capability(), Some(Capability::Mail));
        let unknown: MethodName = "Foo/bar".parse().unwrap();
        assert_eq!(unknown.as_str(), "Foo/bar");
        assert_eq!(unknown.capability(), None);
        for (name, capability) in [
            ("Calendar/get", Capability::Calendars),
            ("CalendarEvent/query", Capability::Calendars),
            ("ParticipantIdentity/get", Capability::Calendars),
            ("CalendarEvent/parse", Capability::CalendarsParse),
            ("CalendarEvent/counter", Capability::Bridge),
            ("CalendarEvent/declineCounter", Capability::Bridge),
            ("AddressBook/set", Capability::Contacts),
            ("ContactCard/query", Capability::Contacts),
            ("ContactCard/parse", Capability::ContactsParse),
            ("Principal/get", Capability::Principals),
            ("Principal/query", Capability::Principals),
            ("Principal/changes", Capability::Principals),
            (
                "Principal/getAvailability",
                Capability::PrincipalsAvailability,
            ),
            ("PushSubscription/get", Capability::Core),
            ("PushSubscription/set", Capability::Core),
            ("Mailbox/queryChanges", Capability::Mail),
            ("Email/copy", Capability::Mail),
            ("Email/parse", Capability::Mail),
            ("Blob/copy", Capability::Core),
        ] {
            let parsed: MethodName = name.parse().unwrap();
            assert_eq!(parsed.as_str(), name);
            assert_eq!(parsed.capability(), Some(capability), "{name}");
        }
        assert_eq!(
            "EmailSubmission/set"
                .parse::<MethodName>()
                .unwrap()
                .capability(),
            Some(Capability::Submission)
        );
    }
}
