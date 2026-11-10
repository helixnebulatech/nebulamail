// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Session resource (RFC 8620 §2), served at `/.well-known/jmap`.

use std::collections::BTreeMap;
use std::fmt;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::method::Collation;
use crate::request::{Capability, MethodLimits, RequestLimits};
use crate::types::{AccountId, Duration, PrincipalId, State, UnsignedInt, UtcDate};

/// `urn:ietf:params:jmap:core` (RFC 8620 §2): the server's limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreCapability {
    /// Largest blob upload, in bytes.
    pub max_size_upload: UnsignedInt,
    /// Concurrent uploads per account.
    pub max_concurrent_upload: UnsignedInt,
    /// Largest API request body, in bytes.
    pub max_size_request: UnsignedInt,
    /// Concurrent API requests.
    pub max_concurrent_requests: UnsignedInt,
    /// Method calls per request.
    pub max_calls_in_request: UnsignedInt,
    /// Ids per `/get`.
    pub max_objects_in_get: UnsignedInt,
    /// Objects per `/set`.
    pub max_objects_in_set: UnsignedInt,
    /// Collations the server supports for sorting and filtering.
    pub collation_algorithms: Vec<Collation>,
}

impl CoreCapability {
    /// The limits [`Request::parse`](crate::Request::parse) enforces.
    #[must_use]
    pub fn request_limits(&self) -> RequestLimits {
        RequestLimits {
            max_size_request: self.max_size_request.to_usize_saturating(),
            max_calls_in_request: self.max_calls_in_request.to_usize_saturating(),
        }
    }

    /// The limits [`MethodCall::parse`](crate::MethodCall::parse)
    /// enforces.
    #[must_use]
    pub fn method_limits(&self) -> MethodLimits {
        MethodLimits {
            max_objects_in_get: self.max_objects_in_get.to_usize_saturating(),
            max_objects_in_set: self.max_objects_in_set.to_usize_saturating(),
            max_size_request: self.max_size_request.to_usize_saturating(),
        }
    }
}

/// Account-level `urn:ietf:params:jmap:mail` (RFC 8621 §1.3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailAccountCapability {
    /// Most mailboxes one Email can be in (`None`: no limit).
    pub max_mailboxes_per_email: Option<UnsignedInt>,
    /// Deepest mailbox nesting (`None`: no limit).
    pub max_mailbox_depth: Option<UnsignedInt>,
    /// Longest mailbox name, in UTF-8 bytes.
    pub max_size_mailbox_name: UnsignedInt,
    /// Largest total size of attachments when creating an Email.
    pub max_size_attachments_per_email: UnsignedInt,
    /// Sort properties `Email/query` supports.
    pub email_query_sort_options: Vec<String>,
    /// Whether the user may create top-level mailboxes.
    pub may_create_top_level_mailbox: bool,
}

/// Account-level `urn:ietf:params:jmap:submission` (RFC 8621 §1.3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionAccountCapability {
    /// Longest delay for a future send, in seconds (0: not supported).
    pub max_delayed_send: UnsignedInt,
    /// SMTP extensions the submission server supports (EHLO keyword →
    /// arguments).
    pub submission_extensions: BTreeMap<String, Vec<String>>,
}

/// Account-level `urn:ietf:params:jmap:calendars`
/// (draft-ietf-jmap-calendars §2; the fields Stalwart sends).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarsAccountCapability {
    /// Most calendars one event can be in (`None`: no limit).
    pub max_calendars_per_event: Option<UnsignedInt>,
    /// Earliest date-time an event may have.
    pub min_date_time: UtcDate,
    /// Latest date-time an event may have.
    pub max_date_time: UtcDate,
    /// Longest `after`–`before` range with `expandRecurrences`.
    pub max_expanded_query_duration: Duration,
    /// Most participants in one event (`None`: no limit).
    pub max_participants_per_event: Option<UnsignedInt>,
    /// Whether the user may create calendars here.
    pub may_create_calendar: bool,
}

/// Account-level `urn:ietf:params:jmap:contacts` (RFC 9610 §1.4.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactsAccountCapability {
    /// Most address books one card can be in (`None`: no limit).
    pub max_address_books_per_card: Option<UnsignedInt>,
    /// Whether the user may create address books here.
    pub may_create_address_book: bool,
}

/// Account-level `urn:ietf:params:jmap:sieve` (RFC 9661 §2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SieveAccountCapability {
    /// The server's name and version.
    pub implementation: String,
    /// Longest script name in bytes.
    pub max_size_script_name: UnsignedInt,
    /// Largest script in bytes, if limited.
    pub max_size_script: Option<UnsignedInt>,
    /// Most scripts, if limited.
    pub max_number_scripts: Option<UnsignedInt>,
    /// Most `redirect` actions per run, if limited.
    pub max_number_redirects: Option<UnsignedInt>,
    /// The Sieve extensions scripts may `require`.
    pub sieve_extensions: Vec<String>,
    /// The `enotify` methods, if `enotify` is supported.
    pub notification_methods: Option<Vec<String>>,
    /// The `extlists` lists, if supported.
    pub external_lists: Option<Vec<String>>,
}

/// Account-level `urn:ietf:params:jmap:principals` (RFC 9670 §1.5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalsAccountCapability {
    /// The user's own principal, if it is in this account.
    pub current_user_principal_id: Option<PrincipalId>,
}

/// Account-level `urn:ietf:params:jmap:principals:availability`
/// (draft-ietf-jmap-calendars): `Principal/getAvailability` works here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalsAvailabilityAccountCapability {
    /// Longest `utcStart`–`utcEnd` range one call may ask for.
    pub max_availability_duration: Duration,
}

/// Account-level [`Capability::Bridge`]: which of the bridge's own
/// calendar extras this account's server offers
/// so a client shows them only there, and the mail server's
/// latest alerts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeAccountCapability {
    /// Calendars can be published at a public link (`isPublished`).
    pub calendar_publishing: bool,
    /// Webcal subscriptions can be read and added (`subscriptionUrl`).
    pub calendar_subscriptions: bool,
    /// `CalendarTrash/get` and `CalendarTrash/set` work.
    pub calendar_trash: bool,
    /// The IMAP server's `[ALERT]`s, which "MUST be presented to the user"
    /// (RFC 3501 §7.1, RFC 9051 §7.1); oldest first, only written when
    /// there are some. In the session, so a new one changes its state and
    /// clients fetch it again (RFC 8620 §2).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<ServerAlert>,
}

/// One `[ALERT]` of the account's mail server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerAlert {
    /// Stable while it is shown, to remember that the user closed it.
    pub id: String,
    /// Untrusted server text, already cleaned of control characters and
    /// cut to a few hundred characters: show it as plain text.
    pub text: String,
    /// When it arrived.
    pub received_at: UtcDate,
}

/// Session-level [`Capability::Bridge`]: the bridge's own extras that do
/// not depend on the account. `{}` when none is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSessionCapability {
    /// Held submissions may go out after the user signs out
    /// (`EmailSubmission`'s vendor property `sendWhenSignedOut`). Only
    /// written when on.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub send_when_signed_out: bool,
}

/// Capabilities of one account. Each can be on or off per account (e.g.
/// calendars only when the account's CalDAV server answered).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per capability without data, as in the JSON"
)]
pub struct AccountCapabilities {
    /// Mail, if the account has it.
    pub mail: Option<MailAccountCapability>,
    /// Submission, if the account can send.
    pub submission: Option<SubmissionAccountCapability>,
    /// Whether the account has quotas (RFC 9425; the value is `{}`).
    pub quota: bool,
    /// Whether the account can send and parse MDNs (RFC 9007; `{}`).
    pub mdn: bool,
    /// Sieve scripts (RFC 9661), if the account has them.
    pub sieve: Option<SieveAccountCapability>,
    /// Whether the account has a vacation response (RFC 8621 §8; `{}`).
    pub vacation_response: bool,
    /// Contacts, if the account has them (RFC 9610).
    pub contacts: Option<ContactsAccountCapability>,
    /// Whether `ContactCard/parse` works (`{}`; only with `contacts`).
    pub contacts_parse: bool,
    /// Calendars, if the account has them (draft-ietf-jmap-calendars).
    pub calendars: Option<CalendarsAccountCapability>,
    /// Whether `CalendarEvent/parse` works (`{}`; only with `calendars`).
    pub calendars_parse: bool,
    /// Principals (RFC 9670), if the account has them.
    pub principals: Option<PrincipalsAccountCapability>,
    /// Free-busy of principals (only with `principals`).
    pub principals_availability: Option<PrincipalsAvailabilityAccountCapability>,
    /// Whether `emailPush` works for this account (Stalwart 1.0's
    /// `urn:ietf:params:jmap:emailpush`, `{}`; only with `mail`).
    pub email_push: bool,
    /// The bridge's own extras, on the bridge (not part of
    /// [`AccountCapabilities::supported`]: the session announces the
    /// capability by itself).
    pub bridge: Option<BridgeAccountCapability>,
}

impl AccountCapabilities {
    /// The capabilities this account has (without core), in the order
    /// they are written. The `:parse` ones count only with their parent.
    #[must_use]
    pub fn supported(&self) -> Vec<Capability> {
        let contacts = self.contacts.is_some();
        let calendars = self.calendars.is_some();
        let principals = self.principals.is_some();
        let mail = self.mail.is_some();
        [
            (self.mail.is_some(), Capability::Mail),
            (self.submission.is_some(), Capability::Submission),
            (self.quota, Capability::Quota),
            (self.mdn, Capability::Mdn),
            (self.sieve.is_some(), Capability::Sieve),
            (self.vacation_response, Capability::VacationResponse),
            (contacts, Capability::Contacts),
            (contacts && self.contacts_parse, Capability::ContactsParse),
            (calendars, Capability::Calendars),
            (
                calendars && self.calendars_parse,
                Capability::CalendarsParse,
            ),
            (principals, Capability::Principals),
            (
                principals && self.principals_availability.is_some(),
                Capability::PrincipalsAvailability,
            ),
            (mail && self.email_push, Capability::EmailPush),
        ]
        .into_iter()
        .filter_map(|(on, capability)| on.then_some(capability))
        .collect()
    }
}

impl Serialize for AccountCapabilities {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(mail) = &self.mail {
            map.serialize_entry(Capability::Mail.as_str(), mail)?;
        }
        if let Some(submission) = &self.submission {
            map.serialize_entry(Capability::Submission.as_str(), submission)?;
        }
        if self.quota {
            map.serialize_entry(Capability::Quota.as_str(), &Empty)?;
        }
        if self.mdn {
            map.serialize_entry(Capability::Mdn.as_str(), &Empty)?;
        }
        if let Some(sieve) = &self.sieve {
            map.serialize_entry(Capability::Sieve.as_str(), sieve)?;
        }
        if self.vacation_response {
            map.serialize_entry(Capability::VacationResponse.as_str(), &Empty)?;
        }
        if let Some(contacts) = &self.contacts {
            map.serialize_entry(Capability::Contacts.as_str(), contacts)?;
            if self.contacts_parse {
                map.serialize_entry(Capability::ContactsParse.as_str(), &Empty)?;
            }
        }
        if let Some(calendars) = &self.calendars {
            map.serialize_entry(Capability::Calendars.as_str(), calendars)?;
            if self.calendars_parse {
                map.serialize_entry(Capability::CalendarsParse.as_str(), &Empty)?;
            }
        }
        if let Some(principals) = &self.principals {
            map.serialize_entry(Capability::Principals.as_str(), principals)?;
            if let Some(availability) = &self.principals_availability {
                map.serialize_entry(Capability::PrincipalsAvailability.as_str(), availability)?;
            }
        }
        if self.mail.is_some() && self.email_push {
            map.serialize_entry(Capability::EmailPush.as_str(), &Empty)?;
        }
        if let Some(bridge) = &self.bridge {
            map.serialize_entry(Capability::Bridge.as_str(), bridge)?;
        }
        map.end()
    }
}

/// An account (RFC 8620 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// A name for the user (usually the email address).
    pub name: String,
    /// Whether this is the user's own account.
    pub is_personal: bool,
    /// Whether the whole account is read-only.
    pub is_read_only: bool,
    /// What the account supports.
    pub account_capabilities: AccountCapabilities,
}

/// Session-level capabilities. Besides core, none has session-level data
/// (`{}`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per capability without session data, as in the JSON"
)]
pub struct SessionCapabilities {
    /// `urn:ietf:params:jmap:core`.
    pub core: CoreCapability,
    /// Announce `urn:ietf:params:jmap:mail`.
    pub mail: bool,
    /// Announce `urn:ietf:params:jmap:submission`.
    pub submission: bool,
    /// Announce `urn:ietf:params:jmap:quota`.
    pub quota: bool,
    /// Announce `urn:ietf:params:jmap:mdn`.
    pub mdn: bool,
    /// Announce `urn:ietf:params:jmap:sieve`.
    pub sieve: bool,
    /// Announce `urn:ietf:params:jmap:vacationresponse`.
    pub vacation_response: bool,
    /// Announce `urn:ietf:params:jmap:contacts`.
    pub contacts: bool,
    /// Announce `urn:ietf:params:jmap:contacts:parse`.
    pub contacts_parse: bool,
    /// Announce `urn:ietf:params:jmap:calendars`.
    pub calendars: bool,
    /// Announce `urn:ietf:params:jmap:calendars:parse`.
    pub calendars_parse: bool,
    /// Announce `urn:ietf:params:jmap:principals`.
    pub principals: bool,
    /// Announce `urn:ietf:params:jmap:principals:availability`.
    pub principals_availability: bool,
    /// Announce [`Capability::Bridge`] with these extras (the bridge sets
    /// it; not from the account).
    pub bridge: Option<BridgeSessionCapability>,
    /// Announce [`Capability::WebPushVapid`] with this
    /// `applicationServerKey` (base64url of the VAPID public key).
    pub web_push_vapid: Option<String>,
    /// Announce [`Capability::EmailPush`].
    pub email_push: bool,
}

impl SessionCapabilities {
    /// The session capabilities of a session with one account: what the
    /// account has.
    #[must_use]
    pub fn for_account(core: CoreCapability, account: &AccountCapabilities) -> Self {
        let has = account.supported();
        let on = |capability: Capability| has.contains(&capability);
        Self {
            core,
            mail: on(Capability::Mail),
            submission: on(Capability::Submission),
            quota: on(Capability::Quota),
            mdn: on(Capability::Mdn),
            sieve: on(Capability::Sieve),
            vacation_response: on(Capability::VacationResponse),
            contacts: on(Capability::Contacts),
            contacts_parse: on(Capability::ContactsParse),
            calendars: on(Capability::Calendars),
            calendars_parse: on(Capability::CalendarsParse),
            principals: on(Capability::Principals),
            principals_availability: on(Capability::PrincipalsAvailability),
            bridge: None,
            web_push_vapid: None,
            email_push: on(Capability::EmailPush),
        }
    }

    /// The capabilities announced, i.e. what `using` may contain.
    #[must_use]
    pub fn supported(&self) -> Vec<Capability> {
        let mut list = vec![Capability::Core];
        for (on, capability) in [
            (self.mail, Capability::Mail),
            (self.submission, Capability::Submission),
            (self.quota, Capability::Quota),
            (self.mdn, Capability::Mdn),
            (self.sieve, Capability::Sieve),
            (self.vacation_response, Capability::VacationResponse),
            (self.contacts, Capability::Contacts),
            (self.contacts_parse, Capability::ContactsParse),
            (self.calendars, Capability::Calendars),
            (self.calendars_parse, Capability::CalendarsParse),
            (self.principals, Capability::Principals),
            (
                self.principals_availability,
                Capability::PrincipalsAvailability,
            ),
            (self.bridge.is_some(), Capability::Bridge),
            (self.web_push_vapid.is_some(), Capability::WebPushVapid),
            (self.email_push, Capability::EmailPush),
        ] {
            if on {
                list.push(capability);
            }
        }
        list
    }
}

/// `{"applicationServerKey": "…"}` (RFC 9749 §3).
struct WebPushVapid<'a> {
    key: &'a str,
}

impl Serialize for WebPushVapid<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("applicationServerKey", self.key)?;
        map.end()
    }
}

/// Serializes as `{}`.
struct Empty;

impl Serialize for Empty {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_map(Some(0))?.end()
    }
}

impl Serialize for SessionCapabilities {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry(Capability::Core.as_str(), &self.core)?;
        for capability in self.supported().iter().skip(1) {
            match (capability, &self.web_push_vapid, &self.bridge) {
                (Capability::WebPushVapid, Some(key), _) => {
                    map.serialize_entry(capability.as_str(), &WebPushVapid { key })?;
                }
                (Capability::Bridge, _, Some(bridge)) => {
                    map.serialize_entry(capability.as_str(), bridge)?;
                }
                _ => map.serialize_entry(capability.as_str(), &Empty)?,
            }
        }
        map.end()
    }
}

/// Why a URL template is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TemplateError {
    /// A required `{variable}` is missing.
    #[error("URL template lacks {{{0}}}")]
    MissingVariable(&'static str),
    /// Spaces or control characters.
    #[error("URL template has spaces or control characters")]
    InvalidCharacter,
}

fn check_template(template: &str, variables: &[&'static str]) -> Result<(), TemplateError> {
    if template
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(TemplateError::InvalidCharacter);
    }
    for variable in variables {
        if !template.contains(&format!("{{{variable}}}")) {
            return Err(TemplateError::MissingVariable(variable));
        }
    }
    Ok(())
}

macro_rules! url_template {
    ($(#[$doc:meta])* $name:ident, [$($variable:literal),*]) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Variables the template must contain.
            pub const VARIABLES: &'static [&'static str] = &[$($variable),*];

            /// Checks and wraps a template.
            ///
            /// # Errors
            /// [`TemplateError`] if a variable is missing or it has spaces.
            pub fn new(template: impl Into<String>) -> Result<Self, TemplateError> {
                let template = template.into();
                check_template(&template, Self::VARIABLES)?;
                Ok(Self(template))
            }

            /// The template.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.0).finish()
            }
        }
    };
}

url_template!(
    /// `downloadUrl` (RFC 6570 level 1): `{accountId}`, `{blobId}`,
    /// `{type}`, `{name}`.
    DownloadUrl,
    ["accountId", "blobId", "type", "name"]
);
url_template!(
    /// `uploadUrl`: `{accountId}`.
    UploadUrl,
    ["accountId"]
);
url_template!(
    /// `eventSourceUrl`: `{types}`, `{closeafter}`, `{ping}`.
    EventSourceUrl,
    ["types", "closeafter", "ping"]
);

/// The Session object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// What the server supports.
    pub capabilities: SessionCapabilities,
    /// The accounts the user can use.
    pub accounts: BTreeMap<AccountId, Account>,
    /// The main account for each capability.
    pub primary_accounts: BTreeMap<Capability, AccountId>,
    /// The user's login name.
    pub username: String,
    /// Where to `POST` API requests.
    pub api_url: String,
    /// Where to download blobs.
    pub download_url: DownloadUrl,
    /// Where to upload blobs.
    pub upload_url: UploadUrl,
    /// Where to open the `EventSource` for push.
    pub event_source_url: EventSourceUrl,
    /// Changes when anything above changes.
    pub state: State,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn n(value: u32) -> UnsignedInt {
        UnsignedInt::from(value)
    }

    fn core() -> CoreCapability {
        CoreCapability {
            max_size_upload: n(50_000_000),
            max_concurrent_upload: n(8),
            max_size_request: n(10_000_000),
            max_concurrent_requests: n(8),
            max_calls_in_request: n(32),
            max_objects_in_get: n(256),
            max_objects_in_set: n(128),
            collation_algorithms: vec![
                Collation::AsciiNumeric,
                Collation::AsciiCasemap,
                Collation::UnicodeCasemap,
            ],
        }
    }

    /// RFC 8620 §2.1, reduced to the capabilities the bridge has.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one RFC example, field by field")]
    fn rfc_example_shape() {
        let account_id: AccountId = "A13824".parse().unwrap();
        let session = Session {
            capabilities: SessionCapabilities {
                core: core(),
                mail: true,
                submission: true,
                quota: false,
                mdn: false,
                sieve: false,
                vacation_response: false,
                contacts: false,
                contacts_parse: false,
                calendars: false,
                calendars_parse: false,
                principals: false,
                principals_availability: false,
                bridge: None,
                web_push_vapid: None,
                email_push: false,
            },
            accounts: [(
                account_id.clone(),
                Account {
                    name: "john@example.com".to_owned(),
                    is_personal: true,
                    is_read_only: false,
                    account_capabilities: AccountCapabilities {
                        mail: Some(MailAccountCapability {
                            max_mailboxes_per_email: None,
                            max_mailbox_depth: Some(n(10)),
                            max_size_mailbox_name: n(255),
                            max_size_attachments_per_email: n(50_000_000),
                            email_query_sort_options: vec!["receivedAt".to_owned()],
                            may_create_top_level_mailbox: true,
                        }),
                        submission: Some(SubmissionAccountCapability {
                            max_delayed_send: n(0),
                            submission_extensions: BTreeMap::new(),
                        }),
                        quota: false,
                        mdn: false,
                        ..AccountCapabilities::default()
                    },
                },
            )]
            .into(),
            primary_accounts: [
                (Capability::Mail, account_id.clone()),
                (Capability::Submission, account_id),
            ]
            .into(),
            username: "john@example.com".to_owned(),
            api_url: "https://jmap.example.com/api/".to_owned(),
            download_url: DownloadUrl::new(
                "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
            )
            .unwrap(),
            upload_url: UploadUrl::new("https://jmap.example.com/upload/{accountId}/").unwrap(),
            event_source_url: EventSourceUrl::new(
                "https://jmap.example.com/eventsource/?types={types}&closeafter={closeafter}&ping={ping}",
            )
            .unwrap(),
            state: State::new("75128aab4b1b"),
        };
        assert_eq!(
            serde_json::to_value(&session).unwrap(),
            json!({
                "capabilities": {
                    "urn:ietf:params:jmap:core": {
                        "maxSizeUpload": 50_000_000,
                        "maxConcurrentUpload": 8,
                        "maxSizeRequest": 10_000_000,
                        "maxConcurrentRequests": 8,
                        "maxCallsInRequest": 32,
                        "maxObjectsInGet": 256,
                        "maxObjectsInSet": 128,
                        "collationAlgorithms": ["i;ascii-numeric", "i;ascii-casemap", "i;unicode-casemap"]
                    },
                    "urn:ietf:params:jmap:mail": {},
                    "urn:ietf:params:jmap:submission": {}
                },
                "accounts": {
                    "A13824": {
                        "name": "john@example.com",
                        "isPersonal": true,
                        "isReadOnly": false,
                        "accountCapabilities": {
                            "urn:ietf:params:jmap:mail": {
                                "maxMailboxesPerEmail": null,
                                "maxMailboxDepth": 10,
                                "maxSizeMailboxName": 255,
                                "maxSizeAttachmentsPerEmail": 50_000_000,
                                "emailQuerySortOptions": ["receivedAt"],
                                "mayCreateTopLevelMailbox": true
                            },
                            "urn:ietf:params:jmap:submission": {
                                "maxDelayedSend": 0,
                                "submissionExtensions": {}
                            }
                        }
                    }
                },
                "primaryAccounts": {
                    "urn:ietf:params:jmap:mail": "A13824",
                    "urn:ietf:params:jmap:submission": "A13824"
                },
                "username": "john@example.com",
                "apiUrl": "https://jmap.example.com/api/",
                "downloadUrl": "https://jmap.example.com/download/{accountId}/{blobId}/{name}?accept={type}",
                "uploadUrl": "https://jmap.example.com/upload/{accountId}/",
                "eventSourceUrl": "https://jmap.example.com/eventsource/?types={types}&closeafter={closeafter}&ping={ping}",
                "state": "75128aab4b1b"
            })
        );
    }

    /// The calendar and contact capabilities of Stalwart 1.0's session for
    /// alice (2026-10-27), field by field.
    #[test]
    fn calendars_and_contacts_like_stalwart() {
        let account = AccountCapabilities {
            contacts: Some(ContactsAccountCapability {
                max_address_books_per_card: None,
                may_create_address_book: true,
            }),
            contacts_parse: true,
            calendars: Some(CalendarsAccountCapability {
                max_calendars_per_event: None,
                min_date_time: "0001-01-01T00:00:00Z".parse().unwrap(),
                max_date_time: "9999-12-31T23:59:59Z".parse().unwrap(),
                max_expanded_query_duration: "P52W1D".parse().unwrap(),
                max_participants_per_event: Some(n(20)),
                may_create_calendar: true,
            }),
            calendars_parse: true,
            ..AccountCapabilities::default()
        };
        assert_eq!(
            serde_json::to_value(&account).unwrap(),
            json!({
                "urn:ietf:params:jmap:contacts": {
                    "maxAddressBooksPerCard": null, "mayCreateAddressBook": true
                },
                "urn:ietf:params:jmap:contacts:parse": {},
                "urn:ietf:params:jmap:calendars": {
                    "maxCalendarsPerEvent": null,
                    "minDateTime": "0001-01-01T00:00:00Z",
                    "maxDateTime": "9999-12-31T23:59:59Z",
                    "maxExpandedQueryDuration": "P52W1D",
                    "maxParticipantsPerEvent": 20,
                    "mayCreateCalendar": true
                },
                "urn:ietf:params:jmap:calendars:parse": {}
            })
        );
        let session = SessionCapabilities::for_account(core(), &account);
        assert_eq!(
            session.supported(),
            [
                Capability::Core,
                Capability::Contacts,
                Capability::ContactsParse,
                Capability::Calendars,
                Capability::CalendarsParse
            ]
        );
        let value = serde_json::to_value(&session).unwrap();
        assert_eq!(value["urn:ietf:params:jmap:calendars:parse"], json!({}));
        assert!(value.get("urn:ietf:params:jmap:mail").is_none());

        // `:parse` without its parent is not announced.
        let orphan = AccountCapabilities {
            calendars_parse: true,
            contacts_parse: true,
            ..AccountCapabilities::default()
        };
        assert!(orphan.supported().is_empty());
        assert_eq!(serde_json::to_value(&orphan).unwrap(), json!({}));
    }

    /// Stalwart 0.16.27's `principals` and `principals:availability` for
    /// alice (2026-10-27); availability only counts with principals.
    #[test]
    fn principals_like_stalwart() {
        let account = AccountCapabilities {
            principals: Some(PrincipalsAccountCapability {
                current_user_principal_id: Some("c".parse().unwrap()),
            }),
            principals_availability: Some(PrincipalsAvailabilityAccountCapability {
                max_availability_duration: "P52W1D".parse().unwrap(),
            }),
            ..AccountCapabilities::default()
        };
        assert_eq!(
            serde_json::to_value(&account).unwrap(),
            json!({
                "urn:ietf:params:jmap:principals": {"currentUserPrincipalId": "c"},
                "urn:ietf:params:jmap:principals:availability": {
                    "maxAvailabilityDuration": "P52W1D"
                }
            })
        );
        let session = SessionCapabilities::for_account(core(), &account);
        assert_eq!(
            session.supported(),
            [
                Capability::Core,
                Capability::Principals,
                Capability::PrincipalsAvailability
            ]
        );
        let value = serde_json::to_value(&session).unwrap();
        assert_eq!(
            value["urn:ietf:params:jmap:principals:availability"],
            json!({})
        );

        let orphan = AccountCapabilities {
            principals_availability: account.principals_availability.clone(),
            ..AccountCapabilities::default()
        };
        assert!(orphan.supported().is_empty());
        assert_eq!(serde_json::to_value(&orphan).unwrap(), json!({}));
    }

    #[test]
    fn templates_and_limits() {
        assert_eq!(
            UploadUrl::new("https://x/upload/"),
            Err(TemplateError::MissingVariable("accountId"))
        );
        assert_eq!(
            DownloadUrl::new("https://x/{accountId}/{blobId}/{name} {type}"),
            Err(TemplateError::InvalidCharacter)
        );
        let limits = core().request_limits();
        assert_eq!(limits.max_calls_in_request, 32);
        assert_eq!(core().method_limits().max_objects_in_set, 128);
        let capabilities = SessionCapabilities {
            core: core(),
            mail: true,
            submission: false,
            quota: true,
            mdn: false,
            sieve: false,
            vacation_response: false,
            contacts: false,
            contacts_parse: false,
            calendars: false,
            calendars_parse: false,
            principals: false,
            principals_availability: false,
            bridge: Some(BridgeSessionCapability::default()),
            web_push_vapid: Some("BKey".to_owned()),
            email_push: true,
        };
        assert_eq!(
            capabilities.supported(),
            [
                Capability::Core,
                Capability::Mail,
                Capability::Quota,
                Capability::Bridge,
                Capability::WebPushVapid,
                Capability::EmailPush
            ]
        );
        // What mail/email-push.ts reads (session and own account).
        assert_eq!(
            serde_json::to_value(&capabilities).unwrap()["urn:ietf:params:jmap:emailpush"],
            json!({})
        );
        // Only with mail.
        let own = AccountCapabilities {
            email_push: true,
            ..AccountCapabilities::default()
        };
        assert!(!own.supported().contains(&Capability::EmailPush));
        assert!(
            serde_json::to_value(&own)
                .unwrap()
                .get("urn:ietf:params:jmap:emailpush")
                .is_none()
        );
        let value = serde_json::to_value(&capabilities).unwrap();
        assert_eq!(value["https://helixnebula.tech/jmap/bridge"], json!({}));
        // With scheduled mail sent after sign-out.
        let with_signed_out = SessionCapabilities {
            bridge: Some(BridgeSessionCapability {
                send_when_signed_out: true,
            }),
            ..capabilities.clone()
        };
        assert_eq!(
            serde_json::to_value(&with_signed_out).unwrap()["https://helixnebula.tech/jmap/bridge"],
            json!({"sendWhenSignedOut": true})
        );
        // What lib/web-push.ts reads (`vapidKey`).
        assert_eq!(
            value["urn:ietf:params:jmap:webpush-vapid"],
            json!({"applicationServerKey": "BKey"})
        );
    }
}
