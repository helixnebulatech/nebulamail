// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed property values. Opaque server values are checked once, when
//! parsed, so they can be sent back in a header or in XML as they are.

use std::fmt;

/// Printable ASCII only: no CR/LF (header injection), no controls.
fn is_printable_ascii(text: &str) -> bool {
    text.bytes().all(|b| (0x20..0x7F).contains(&b))
}

/// An entity tag (`ETag` header, `DAV:getetag`), sent back in `If-Match`.
///
/// Normally `"…"` or `W/"…"` (RFC 9110 §8.8.3). Some servers give it
/// without quotes; that is kept as it is (the server compares its own
/// spelling), but only if it has no spaces, quotes or commas, which would
/// change the meaning of `If-Match`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ETag(String);

impl ETag {
    /// Longest entity tag accepted.
    pub const MAX_LEN: usize = 1024;

    /// Parses an entity tag (surrounding whitespace removed).
    ///
    /// ```
    /// use nebula_dav_proto::ETag;
    ///
    /// assert_eq!(ETag::parse(" \"abc\" ").unwrap().as_str(), "\"abc\"");
    /// assert!(ETag::parse("W/\"1-2\"").is_some());
    /// assert!(ETag::parse("abc123").is_some());
    /// assert!(ETag::parse("\"a\"\r\nX-Evil: 1").is_none());
    /// assert!(ETag::parse("\"a\", \"b\"").is_none());
    /// assert!(ETag::parse("").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'));
        if text.is_empty() || text.len() > Self::MAX_LEN || !is_printable_ascii(text) {
            return None;
        }
        let opaque = text.strip_prefix("W/").unwrap_or(text);
        let quoted = opaque
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'));
        let valid = match quoted {
            Some(inner) => !inner.contains('"'),
            None => !opaque.contains(['"', ' ', ',']),
        };
        valid.then(|| Self(text.to_owned()))
    }

    /// The tag as the server wrote it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A weak tag (`W/"…"`, RFC 9110 §8.8.1): it only says two
    /// representations are equivalent, never that they are the same
    /// bytes.
    #[must_use]
    pub fn is_weak(&self) -> bool {
        self.0.starts_with("W/")
    }

    /// The tag for an `If-Match` header: `None` if it is weak, since
    /// `If-Match` uses the strong comparison (RFC 9110 §13.1.1) and a weak
    /// tag never matches. An unquoted one (some servers write `getetag`
    /// so) goes quoted, as RFC 9110 §8.8.3's `entity-tag` is.
    ///
    /// ```
    /// use nebula_dav_proto::ETag;
    ///
    /// let tag = |t: &str| ETag::parse(t).unwrap().if_match();
    /// assert_eq!(tag("\"a1\"").as_deref(), Some("\"a1\""));
    /// assert_eq!(tag("a1").as_deref(), Some("\"a1\""));
    /// assert_eq!(tag("W/\"a1\""), None);
    /// ```
    #[must_use]
    pub fn if_match(&self) -> Option<String> {
        if self.is_weak() {
            None
        } else if self.0.starts_with('"') {
            Some(self.0.clone())
        } else {
            Some(format!("\"{}\"", self.0))
        }
    }
}

impl fmt::Debug for ETag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ETag({})", self.0)
    }
}

macro_rules! opaque_token {
    ($(#[$doc:meta])* $name:ident, $max:expr) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            /// Longest value accepted.
            pub const MAX_LEN: usize = $max;

            /// Parses the value (surrounding whitespace removed): not
            /// empty, not too long, printable ASCII.
            #[must_use]
            pub fn parse(text: &str) -> Option<Self> {
                let text = text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r'));
                (!text.is_empty() && text.len() <= Self::MAX_LEN && is_printable_ascii(text))
                    .then(|| Self(text.to_owned()))
            }

            /// The value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}

opaque_token!(
    /// A `DAV:sync-token` (RFC 6578): a URI the server gives and gets back.
    ///
    /// ```
    /// use nebula_dav_proto::SyncToken;
    ///
    /// assert!(SyncToken::parse("http://radicale.org/ns/sync/abc").is_some());
    /// assert!(SyncToken::parse("a\nb").is_none());
    /// ```
    SyncToken,
    4096
);

opaque_token!(
    /// A `CS:getctag`: changes whenever anything in the collection does.
    CTag,
    4096
);

/// An `I:calendar-color`: `#RRGGBB` or `#RRGGBBAA`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CalendarColor {
    /// Red.
    pub red: u8,
    /// Green.
    pub green: u8,
    /// Blue.
    pub blue: u8,
    /// Alpha, if given.
    pub alpha: Option<u8>,
}

impl CalendarColor {
    /// Parses `#RRGGBB` or `#RRGGBBAA` (Apple writes the alpha).
    ///
    /// ```
    /// use nebula_dav_proto::CalendarColor;
    ///
    /// let color = CalendarColor::parse("#1E90FFFF").unwrap();
    /// assert_eq!((color.red, color.alpha), (0x1e, Some(0xff)));
    /// assert_eq!(color.to_string(), "#1E90FFFF");
    /// assert!(CalendarColor::parse("red").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |i: usize| {
            hex.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        };
        match hex.len() {
            6 => Some(Self {
                red: byte(0)?,
                green: byte(2)?,
                blue: byte(4)?,
                alpha: None,
            }),
            8 => Some(Self {
                red: byte(0)?,
                green: byte(2)?,
                blue: byte(4)?,
                alpha: Some(byte(6)?),
            }),
            _ => None,
        }
    }
}

impl fmt::Display for CalendarColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.red, self.green, self.blue)?;
        if let Some(alpha) = self.alpha {
            write!(f, "{alpha:02X}")?;
        }
        Ok(())
    }
}

/// An iCalendar component a calendar may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Component {
    /// `VEVENT`.
    Event,
    /// `VTODO`.
    Todo,
    /// `VJOURNAL`.
    Journal,
    /// `VFREEBUSY`.
    FreeBusy,
    /// `VAVAILABILITY`.
    Availability,
}

impl Component {
    /// The iCalendar name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Event => "VEVENT",
            Self::Todo => "VTODO",
            Self::Journal => "VJOURNAL",
            Self::FreeBusy => "VFREEBUSY",
            Self::Availability => "VAVAILABILITY",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [
            Self::Event,
            Self::Todo,
            Self::Journal,
            Self::FreeBusy,
            Self::Availability,
        ]
        .into_iter()
        .find(|c| c.as_str().eq_ignore_ascii_case(name))
    }
}

/// `C:supported-calendar-component-set`. Unknown components are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentSet(Vec<Component>);

impl ComponentSet {
    /// A set of these components (duplicates dropped).
    #[must_use]
    pub fn new(components: &[Component]) -> Self {
        let mut set = Self::default();
        for &component in components {
            set.insert(component);
        }
        set
    }

    pub(crate) fn insert_name(&mut self, name: &str) {
        if let Some(component) = Component::parse(name) {
            self.insert(component);
        }
    }

    fn insert(&mut self, component: Component) {
        if !self.0.contains(&component) {
            self.0.push(component);
        }
    }

    /// Holds this component.
    #[must_use]
    pub fn contains(&self, component: Component) -> bool {
        self.0.contains(&component)
    }

    /// The components.
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.0
    }
}

/// A privilege of `DAV:current-user-privilege-set` (RFC 3744 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Privilege {
    /// `DAV:read`.
    Read,
    /// `DAV:write` (includes the next three).
    Write,
    /// `DAV:write-properties`.
    WriteProperties,
    /// `DAV:write-content`.
    WriteContent,
    /// `DAV:bind`: create members.
    Bind,
    /// `DAV:unbind`: delete members.
    Unbind,
    /// `DAV:read-acl`.
    ReadAcl,
    /// `DAV:write-acl`.
    WriteAcl,
    /// `DAV:read-current-user-privilege-set`.
    ReadCurrentUserPrivilegeSet,
    /// `DAV:all`.
    All,
    /// `C:read-free-busy` (RFC 4791 §6.1.1).
    ReadFreeBusy,
    /// `C:schedule-send` (RFC 6638).
    ScheduleSend,
    /// `C:schedule-deliver` (RFC 6638).
    ScheduleDeliver,
    /// `DAV:share` (draft-pot-webdav-resource-sharing; sabre/dav gives it
    /// to the owner of a calendar it can share).
    Share,
}

impl Privilege {
    const ALL: [Self; 14] = [
        Self::Read,
        Self::Write,
        Self::WriteProperties,
        Self::WriteContent,
        Self::Bind,
        Self::Unbind,
        Self::ReadAcl,
        Self::WriteAcl,
        Self::ReadCurrentUserPrivilegeSet,
        Self::All,
        Self::ReadFreeBusy,
        Self::ScheduleSend,
        Self::ScheduleDeliver,
        Self::Share,
    ];

    pub(crate) const fn qualified_name(self) -> (crate::Namespace, &'static str) {
        use crate::Namespace::{CalDav, Dav};
        match self {
            Self::Read => (Dav, "read"),
            Self::Write => (Dav, "write"),
            Self::WriteProperties => (Dav, "write-properties"),
            Self::WriteContent => (Dav, "write-content"),
            Self::Bind => (Dav, "bind"),
            Self::Unbind => (Dav, "unbind"),
            Self::ReadAcl => (Dav, "read-acl"),
            Self::WriteAcl => (Dav, "write-acl"),
            Self::ReadCurrentUserPrivilegeSet => (Dav, "read-current-user-privilege-set"),
            Self::All => (Dav, "all"),
            Self::ReadFreeBusy => (CalDav, "read-free-busy"),
            Self::ScheduleSend => (CalDav, "schedule-send"),
            Self::ScheduleDeliver => (CalDav, "schedule-deliver"),
            Self::Share => (Dav, "share"),
        }
    }

    pub(crate) fn from_name(namespace: crate::Namespace, local: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.qualified_name() == (namespace, local))
    }

    const fn bit(self) -> u16 {
        let index = match self {
            Self::Read => 0,
            Self::Write => 1,
            Self::WriteProperties => 2,
            Self::WriteContent => 3,
            Self::Bind => 4,
            Self::Unbind => 5,
            Self::ReadAcl => 6,
            Self::WriteAcl => 7,
            Self::ReadCurrentUserPrivilegeSet => 8,
            Self::All => 9,
            Self::ReadFreeBusy => 10,
            Self::ScheduleSend => 11,
            Self::ScheduleDeliver => 12,
            Self::Share => 13,
        };
        1 << index
    }
}

/// The privileges the user has on a resource. Aggregates are expanded
/// when asked: `all` implies everything, `write` implies
/// `write-properties`, `write-content`, `bind` and `unbind`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PrivilegeSet(u16);

impl PrivilegeSet {
    /// A set with these privileges.
    #[must_use]
    pub fn new(privileges: &[Privilege]) -> Self {
        let mut set = Self::default();
        for &privilege in privileges {
            set.insert(privilege);
        }
        set
    }

    pub(crate) fn insert(&mut self, privilege: Privilege) {
        self.0 |= privilege.bit();
    }

    /// The server listed this privilege or an aggregate that contains it.
    #[must_use]
    pub const fn allows(self, privilege: Privilege) -> bool {
        if self.listed(Privilege::All) || self.listed(privilege) {
            return true;
        }
        match privilege {
            Privilege::WriteProperties
            | Privilege::WriteContent
            | Privilege::Bind
            | Privilege::Unbind => self.listed(Privilege::Write),
            _ => false,
        }
    }

    const fn listed(self, privilege: Privilege) -> bool {
        self.0 & privilege.bit() != 0
    }

    /// May change existing resources (`write-content`).
    #[must_use]
    pub const fn can_write_content(self) -> bool {
        self.allows(Privilege::WriteContent)
    }

    /// May create resources (`bind`).
    #[must_use]
    pub const fn can_create(self) -> bool {
        self.allows(Privilege::Bind)
    }

    /// May delete resources (`unbind`).
    #[must_use]
    pub const fn can_delete(self) -> bool {
        self.allows(Privilege::Unbind)
    }

    /// May read (`read`).
    #[must_use]
    pub const fn can_read(self) -> bool {
        self.allows(Privilege::Read)
    }
}

/// A report of `DAV:supported-report-set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Report {
    /// `DAV:sync-collection` (RFC 6578).
    SyncCollection,
    /// `C:calendar-multiget`.
    CalendarMultiget,
    /// `C:calendar-query`.
    CalendarQuery,
    /// `C:free-busy-query`.
    FreeBusyQuery,
    /// `CR:addressbook-multiget`.
    AddressbookMultiget,
    /// `CR:addressbook-query`.
    AddressbookQuery,
}

impl Report {
    const ALL: [Self; 6] = [
        Self::SyncCollection,
        Self::CalendarMultiget,
        Self::CalendarQuery,
        Self::FreeBusyQuery,
        Self::AddressbookMultiget,
        Self::AddressbookQuery,
    ];

    const fn name(self) -> (crate::Namespace, &'static str) {
        use crate::Namespace::{CalDav, CardDav, Dav};
        match self {
            Self::SyncCollection => (Dav, "sync-collection"),
            Self::CalendarMultiget => (CalDav, "calendar-multiget"),
            Self::CalendarQuery => (CalDav, "calendar-query"),
            Self::FreeBusyQuery => (CalDav, "free-busy-query"),
            Self::AddressbookMultiget => (CardDav, "addressbook-multiget"),
            Self::AddressbookQuery => (CardDav, "addressbook-query"),
        }
    }

    pub(crate) fn from_name(namespace: crate::Namespace, local: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|r| r.name() == (namespace, local))
    }

    const fn bit(self) -> u8 {
        let index = match self {
            Self::SyncCollection => 0,
            Self::CalendarMultiget => 1,
            Self::CalendarQuery => 2,
            Self::FreeBusyQuery => 3,
            Self::AddressbookMultiget => 4,
            Self::AddressbookQuery => 5,
        };
        1 << index
    }
}

/// The reports a collection supports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ReportSet(u8);

impl ReportSet {
    /// A set with these reports.
    #[must_use]
    pub fn new(reports: &[Report]) -> Self {
        let mut set = Self::default();
        for &report in reports {
            set.insert(report);
        }
        set
    }

    pub(crate) fn insert(&mut self, report: Report) {
        self.0 |= report.bit();
    }

    /// The report is supported.
    #[must_use]
    pub const fn contains(self, report: Report) -> bool {
        self.0 & report.bit() != 0
    }
}

/// `DAV:resourcetype`: what kind of collection (if any) a resource is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags of one XML element, like the RFC"
)]
pub struct ResourceType {
    /// `DAV:collection`.
    pub collection: bool,
    /// `DAV:principal`.
    pub principal: bool,
    /// `C:calendar`.
    pub calendar: bool,
    /// `CR:addressbook`.
    pub addressbook: bool,
    /// `C:schedule-inbox`.
    pub schedule_inbox: bool,
    /// `C:schedule-outbox`.
    pub schedule_outbox: bool,
    /// `CS:subscribed` (a webcal subscription: read-only, never fetched).
    pub subscribed: bool,
    /// `CS:shared`: someone else's calendar shared with the user
    /// (Calendar Server sharing; the copy in the user's home).
    pub shared: bool,
    /// `CS:shared-owner`: the user's calendar, shared with others.
    pub shared_owner: bool,
    /// `NC:deleted-calendar`: a calendar in Nextcloud's trash bin (still
    /// listed in its home, never shown as a calendar).
    pub deleted_calendar: bool,
}

/// One `CR:address-data-type` of `CR:supported-address-data`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AddressDataType {
    /// `content-type` (`text/vcard` if absent, RFC 6352 §10.4.2).
    pub content_type: String,
    /// `version` (`3.0` if absent).
    pub version: String,
}

impl AddressDataType {
    /// It is plain vCard (`text/vcard`, any case), not jCard or another
    /// type.
    #[must_use]
    pub fn is_vcard(&self) -> bool {
        self.content_type.trim().eq_ignore_ascii_case("text/vcard")
    }

    /// It is vCard 4.0 as text: what [`VcardRequest::Version4`] asks for.
    #[must_use]
    pub fn is_vcard4(&self) -> bool {
        self.is_vcard() && self.version.trim() == "4.0"
    }
}

/// Which vCard to ask a CardDAV server for when reading cards
/// (`address-data` in a multiget, RFC 6352 §10.4; `Accept` on GET).
///
/// Some servers store what they are sent in another version and give it
/// back as stored unless asked: Nextcloud (sabre/dav) keeps a 4.0 card as
/// 3.0, with `KIND:group` turned into `X-ADDRESSBOOKSERVER-KIND`. Ask for
/// 4.0 only when the address book lists it in `supported-address-data`
/// ([`AddressDataType::is_vcard4`]); a server that does not convert sends
/// the card as stored anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum VcardRequest {
    /// Whatever the server stores (no version asked for).
    #[default]
    AsStored,
    /// vCard 4.0 (`content-type="text/vcard" version="4.0"`).
    Version4,
}

impl VcardRequest {
    /// [`Self::Version4`] if one of `types` is vCard 4.0 as text.
    #[must_use]
    pub fn for_supported(types: &[AddressDataType]) -> Self {
        if types.iter().any(AddressDataType::is_vcard4) {
            Self::Version4
        } else {
            Self::AsStored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ETag, Privilege, PrivilegeSet, Report, ReportSet};

    #[test]
    fn etags() {
        assert!(ETag::parse("\"\"").is_some());
        assert!(ETag::parse("W/abc").is_some());
        assert!(ETag::parse("\"a\"b\"").is_none());
        assert!(ETag::parse("ab c").is_none());
        assert!(ETag::parse("\"caf\u{e9}\"").is_none());
        assert!(ETag::parse(&format!("\"{}\"", "a".repeat(ETag::MAX_LEN))).is_none());
    }

    #[test]
    fn privilege_aggregates() {
        let write = PrivilegeSet::new(&[Privilege::Read, Privilege::Write]);
        assert!(write.can_write_content() && write.can_create() && write.can_delete());
        assert!(!write.allows(Privilege::WriteAcl));
        let all = PrivilegeSet::new(&[Privilege::All]);
        assert!(all.allows(Privilege::WriteAcl) && all.can_read());
        let read = PrivilegeSet::new(&[Privilege::Read]);
        assert!(!read.can_write_content() && read.can_read());
        assert!(!PrivilegeSet::default().can_read());
    }

    #[test]
    fn report_sets() {
        let set = ReportSet::new(&[Report::SyncCollection, Report::AddressbookMultiget]);
        assert!(set.contains(Report::SyncCollection));
        assert!(!set.contains(Report::CalendarMultiget));
    }
}
