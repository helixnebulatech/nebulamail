// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The types of the CalDAV/CardDAV data. The store knows nothing of DAV:
//! the groupware layer parses what the server says and hands typed values
//! over; hrefs arrive already normalised.

use std::fmt;

use crate::{CollectionId, DataType, ResourceId, StoreError, UnixTime};

/// A calendar or an address book. The number is what the database stores:
/// never reuse one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CollectionKind {
    /// A CalDAV calendar: JMAP `Calendar`, its resources `CalendarEvent`.
    Calendar,
    /// A CardDAV address book: JMAP `AddressBook`, its resources
    /// `ContactCard`.
    AddressBook,
}

impl CollectionKind {
    pub(crate) const fn code(self) -> i32 {
        match self {
            Self::Calendar => 1,
            Self::AddressBook => 2,
        }
    }

    pub(crate) const fn from_code(code: i32) -> Result<Self, StoreError> {
        match code {
            1 => Ok(Self::Calendar),
            2 => Ok(Self::AddressBook),
            _ => Err(StoreError::Corrupt("collection kind")),
        }
    }

    /// The data type of the collections themselves.
    #[must_use]
    pub const fn data_type(self) -> DataType {
        match self {
            Self::Calendar => DataType::Calendar,
            Self::AddressBook => DataType::AddressBook,
        }
    }

    /// The data type of their resources.
    #[must_use]
    pub const fn resource_data_type(self) -> DataType {
        match self {
            Self::Calendar => DataType::CalendarEvent,
            Self::AddressBook => DataType::ContactCard,
        }
    }
}

/// Where a collection or resource is: an absolute `http(s)` URL as the
/// groupware layer normalised it (`nebula_dav_proto::Href`), so one
/// location has one spelling. Hrefs may name things (`/cal/alice/doctor/`),
/// so `Debug` hides them (no hrefs in the logs).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DavHref(String);

impl DavHref {
    /// Longest href stored: the DAV client's limit plus the origin.
    pub const MAX_LEN: usize = 8 * 1024 + 512;

    /// An href from its URL text.
    ///
    /// # Errors
    /// [`StoreError::Invalid`] unless it is an `http://` or `https://` URL
    /// of at most [`Self::MAX_LEN`] bytes without spaces or controls.
    pub fn new(url: &str) -> Result<Self, StoreError> {
        let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
        let chars_ok = !url.chars().any(|c| c.is_control() || c.is_whitespace());
        if !scheme_ok || !chars_ok || url.len() > Self::MAX_LEN {
            return Err(StoreError::Invalid("href"));
        }
        Ok(Self(url.to_owned()))
    }

    /// The URL text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_db(text: String) -> Result<Self, StoreError> {
        Self::new(&text)
            .map(|_| Self(text))
            .map_err(|_| StoreError::Corrupt("href"))
    }
}

impl fmt::Debug for DavHref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DavHref(..)")
    }
}

/// A set of flags stored as one integer.
macro_rules! flags {
    ($(#[$doc:meta])* $name:ident { $($(#[$flag_doc:meta])* $flag:ident = $bit:expr,)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
        pub struct $name(u8);

        impl $name {
            $($(#[$flag_doc])* pub const $flag: Self = Self($bit);)*
            const ALL: u8 = 0 $(| $bit)*;

            /// No flag.
            #[must_use]
            pub const fn empty() -> Self {
                Self(0)
            }

            /// Both sets together.
            #[must_use]
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            /// The flags set in both.
            #[must_use]
            pub const fn intersection(self, other: Self) -> Self {
                Self(self.0 & other.0)
            }

            /// Every flag of `other` is set.
            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            pub(crate) fn to_db(self) -> i32 {
                i32::from(self.0)
            }

            pub(crate) fn from_db(value: i32) -> Result<Self, StoreError> {
                u8::try_from(value)
                    .ok()
                    .filter(|bits| bits & !Self::ALL == 0)
                    .map(Self)
                    .ok_or(StoreError::Corrupt(stringify!($name)))
            }
        }
    };
}

flags!(
    /// What the user may do in a collection (`current-user-privilege-set`,
    /// aggregates already expanded).
    Rights {
        /// `read`.
        READ = 1,
        /// `write-content`: change resources.
        WRITE_CONTENT = 2,
        /// `write-properties`: rename, recolour.
        WRITE_PROPERTIES = 4,
        /// `bind`: create resources.
        BIND = 8,
        /// `unbind`: delete resources.
        UNBIND = 16,
        /// `read-free-busy` (RFC 4791 §6.1.1): see busy times only.
        READ_FREE_BUSY = 32,
        /// `write-acl`: share it (or, for a sharee, re-share it).
        WRITE_ACL = 64,
    }
);

/// How the user may share a collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShareMode {
    /// WebDAV ACL (RFC 3744): the `ACL` method (Stalwart).
    Acl,
    /// Calendar Server sharing: a `CS:share` POST (sabre/dav, Apple).
    CalendarServer,
    /// Nextcloud sharing: an `O:share` POST (calendars and address
    /// books).
    Nextcloud,
}

impl ShareMode {
    pub(crate) const fn code(self) -> i32 {
        match self {
            Self::Acl => 1,
            Self::CalendarServer => 2,
            Self::Nextcloud => 3,
        }
    }

    pub(crate) const fn from_code(code: i32) -> Result<Self, StoreError> {
        match code {
            1 => Ok(Self::Acl),
            2 => Ok(Self::CalendarServer),
            3 => Ok(Self::Nextcloud),
            _ => Err(StoreError::Corrupt("share mode")),
        }
    }
}

/// Who a collection is shared with: a principal URL or a `mailto:`
/// address, as the server names them.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sharee(String);

impl Sharee {
    /// Longest sharee kept.
    pub const MAX_LEN: usize = DavHref::MAX_LEN;

    /// A sharee from its text.
    ///
    /// # Errors
    /// [`StoreError::Invalid`] unless it is an `http(s)` URL or a
    /// `mailto:` of at most [`Self::MAX_LEN`] bytes without spaces or
    /// controls.
    pub fn new(text: &str) -> Result<Self, StoreError> {
        let scheme_ok = text.starts_with("https://")
            || text.starts_with("http://")
            || text.starts_with("mailto:");
        let chars_ok = !text.chars().any(|c| c.is_control() || c.is_whitespace());
        if !scheme_ok || !chars_ok || text.len() > Self::MAX_LEN {
            return Err(StoreError::Invalid("sharee"));
        }
        Ok(Self(text.to_owned()))
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Sharee {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sharee(..)")
    }
}

/// Most sharees kept per collection: as many as a `CS:invite` or
/// `O:invite` is read with (`nebula_dav_proto::MAX_SHAREES`). More are
/// refused, never cut (a replacement computed from a cut list would
/// remove whoever was left out).
pub const MAX_SHARES: usize = 10_000;

/// The sharing of a collection, as the last listing showed it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionSharing {
    /// The owner's principal, when it is someone else's collection shared
    /// with the user (`None`: the user's own).
    pub owner: Option<DavHref>,
    /// How the user may share it (`None`: not at all).
    pub mode: Option<ShareMode>,
    /// Who it is shared with and with what rights (at most
    /// [`MAX_SHARES`]).
    pub shares: Vec<(Sharee, Rights)>,
}

impl CollectionSharing {
    /// The shares as stored, or [`StoreError::Invalid`] with more than
    /// [`MAX_SHARES`] (never truncated).
    pub(crate) fn shares_to_db(&self) -> Result<Option<String>, StoreError> {
        if self.shares.is_empty() {
            return Ok(None);
        }
        if self.shares.len() > MAX_SHARES {
            return Err(StoreError::Invalid("shares"));
        }
        let list: Vec<(&str, i32)> = self
            .shares
            .iter()
            .map(|(who, rights)| (who.as_str(), rights.to_db()))
            .collect();
        serde_json::to_string(&list)
            .map(Some)
            .map_err(|_| StoreError::Invalid("shares"))
    }

    pub(crate) fn shares_from_db(text: Option<&str>) -> Result<Vec<(Sharee, Rights)>, StoreError> {
        let Some(text) = text else {
            return Ok(Vec::new());
        };
        let list: Vec<(String, i32)> =
            serde_json::from_str(text).map_err(|_| StoreError::Corrupt("shares"))?;
        if list.len() > MAX_SHARES {
            return Err(StoreError::Corrupt("shares"));
        }
        list.into_iter()
            .map(|(who, bits)| {
                Ok((
                    Sharee::new(&who).map_err(|_| StoreError::Corrupt("sharee"))?,
                    Rights::from_db(bits)?,
                ))
            })
            .collect()
    }
}

flags!(
    /// The components a calendar may hold
    /// (`supported-calendar-component-set`).
    Components {
        /// `VEVENT`.
        EVENT = 1,
        /// `VTODO`.
        TODO = 2,
        /// `VJOURNAL`.
        JOURNAL = 4,
        /// `VFREEBUSY`.
        FREEBUSY = 8,
        /// `VAVAILABILITY`.
        AVAILABILITY = 16,
    }
);

flags!(
    /// The vCard versions an address book takes (`supported-address-data`).
    VcardVersions {
        /// vCard 3.0 (RFC 2426).
        V3 = 1,
        /// vCard 4.0 (RFC 6350).
        V4 = 2,
    }
);

/// A URL outside the DAV server that the bridge only shows, never
/// fetches: a subscription's source or a published calendar's public
/// link. `http`, `https`, `webcal` or `webcals`, without spaces or
/// controls, at most [`ExternalUrl::MAX_LEN`] bytes (the groupware layer
/// checked the rest). `Debug` hides it: a public link is a secret.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ExternalUrl(String);

impl ExternalUrl {
    /// Longest URL kept.
    pub const MAX_LEN: usize = 2048;

    /// A URL from its text.
    ///
    /// # Errors
    /// [`StoreError::Invalid`] unless it has one of the schemes, no
    /// spaces or controls and at most [`Self::MAX_LEN`] bytes.
    pub fn new(url: &str) -> Result<Self, StoreError> {
        let scheme_ok = ["https://", "http://", "webcal://", "webcals://"]
            .iter()
            .any(|scheme| url.starts_with(scheme));
        let chars_ok = !url.chars().any(|c| c.is_control() || c.is_whitespace());
        if !scheme_ok || !chars_ok || url.len() > Self::MAX_LEN {
            return Err(StoreError::Invalid("external url"));
        }
        Ok(Self(url.to_owned()))
    }

    /// The URL text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ExternalUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ExternalUrl(..)")
    }
}

/// An [`ExternalUrl`] sealed by the groupware layer with the account's
/// data key (bound to the collection and to which link it is): the store
/// keeps only these opaque bytes, as with a resource's text
/// since a public link or a webcal source is a
/// capability. Compared by its bytes: the caller reuses the stored bytes
/// when the link has not changed, so a new nonce is no update.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SealedLink(Vec<u8>);

impl SealedLink {
    /// Longest sealed value kept: an [`ExternalUrl`] and the sealing's
    /// overhead fit with room to spare.
    pub const MAX_LEN: usize = 4096;

    /// Sealed bytes from the caller.
    ///
    /// # Errors
    /// [`StoreError::Invalid`] if empty or longer than [`Self::MAX_LEN`].
    pub fn new(bytes: Vec<u8>) -> Result<Self, StoreError> {
        if bytes.is_empty() || bytes.len() > Self::MAX_LEN {
            return Err(StoreError::Invalid("sealed link"));
        }
        Ok(Self(bytes))
    }

    /// The sealed bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    fn from_db(bytes: Vec<u8>) -> Result<Self, StoreError> {
        Self::new(bytes).map_err(|_| StoreError::Corrupt("sealed link"))
    }
}

impl fmt::Debug for SealedLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SealedLink(..)")
    }
}

/// Nextcloud's extras of a calendar; all
/// empty elsewhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalendarExtras {
    /// The user may publish it at a public link (`oc-calendar-publishing`,
    /// their own calendar).
    pub publishable: bool,
    /// Its public link, while published (`CS:publish-url`), sealed.
    pub publish_url: Option<SealedLink>,
    /// A webcal subscription (read-only; its events fetched by the
    /// server).
    pub is_subscription: bool,
    /// Where the subscription comes from (`CS:source`), sealed.
    pub subscription_source: Option<SealedLink>,
}

impl CalendarExtras {
    pub(crate) fn from_db(
        publishable: bool,
        publish_url: Option<Vec<u8>>,
        is_subscription: bool,
        subscription_source: Option<Vec<u8>>,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            publishable,
            publish_url: publish_url.map(SealedLink::from_db).transpose()?,
            is_subscription,
            subscription_source: subscription_source.map(SealedLink::from_db).transpose()?,
        })
    }
}

/// What the server says about a calendar or address book. Everything but
/// `supports_sync` is visible over JMAP: a change records an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionProps {
    /// Calendar or address book.
    pub kind: CollectionKind,
    /// Where it is.
    pub href: DavHref,
    /// `displayname`.
    pub name: Option<String>,
    /// `calendar-color` (`#RRGGBB` or `#RRGGBBAA`).
    pub color: Option<String>,
    /// `calendar-description` / `addressbook-description`.
    pub description: Option<String>,
    /// `calendar-order`.
    pub sort_order: Option<i64>,
    /// The calendar's zone: `calendar-timezone-id`, else the TZID of
    /// `calendar-timezone`.
    pub time_zone: Option<String>,
    /// `default-alarm-vevent-datetime`: `VALARM`s as the bridge wrote
    /// them (`None`: none).
    pub default_alarms_datetime: Option<String>,
    /// `default-alarm-vevent-date`, the same.
    pub default_alarms_date: Option<String>,
    /// `current-user-privilege-set` (`None`: not said).
    pub rights: Option<Rights>,
    /// `supported-calendar-component-set` (`None`: any).
    pub components: Option<Components>,
    /// `supported-address-data` (`None`: not said).
    pub vcard_versions: Option<VcardVersions>,
    /// `sync-collection` works here.
    pub supports_sync: bool,
    /// Whose it is and who it is shared with.
    pub sharing: CollectionSharing,
    /// Publishing and subscriptions (Nextcloud).
    pub extras: CalendarExtras,
}

impl CollectionProps {
    /// A JMAP-visible property differs.
    fn visible_differs(&self, other: &Self) -> bool {
        Self {
            supports_sync: other.supports_sync,
            ..self.clone()
        } != *other
    }

    pub(crate) fn needs_update(&self, other: &Self) -> (bool, bool) {
        (self != other, self.visible_differs(other))
    }
}

/// Where the last complete sync of a collection left off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollectionSync {
    /// `sync-token` (RFC 6578).
    pub sync_token: Option<String>,
    /// `getctag`.
    pub ctag: Option<String>,
    /// When it finished (`None`: never).
    pub synced_at: Option<UnixTime>,
}

/// A calendar or address book in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// Its id (the JMAP id).
    pub id: CollectionId,
    /// What the server said.
    pub props: CollectionProps,
    /// Sync state.
    pub sync: CollectionSync,
    /// Gone from the server since then (hidden).
    pub orphaned_at: Option<UnixTime>,
    /// JMAP `isVisible` (bridge only).
    pub is_visible: bool,
    /// JMAP `isSubscribed` (bridge only).
    pub is_subscribed: bool,
    /// JMAP `isDefault`: one per kind, the first by `sort_order`.
    pub is_default: bool,
}

/// What discovery found for an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavAccount {
    /// `current-user-principal`.
    pub principal: DavHref,
    /// `calendar-home-set`.
    pub calendar_homes: Vec<DavHref>,
    /// `addressbook-home-set`.
    pub addressbook_homes: Vec<DavHref>,
    /// `schedule-outbox-URL`.
    pub schedule_outbox: Option<DavHref>,
    /// `calendar-user-address-set` (`mailto:…`).
    pub user_addresses: Vec<String>,
    /// The server announced `calendar-auto-schedule`.
    pub auto_schedule: bool,
    /// When.
    pub discovered_at: UnixTime,
}

/// The indexes of a resource that parsed, the only parts kept in clear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceIndex {
    /// `UID`.
    pub uid: Option<String>,
    /// First start of any occurrence (`None`: unknown).
    pub utc_start: Option<UnixTime>,
    /// Last end of any occurrence (`None`: open-ended, or unknown).
    pub utc_end: Option<UnixTime>,
    /// Has `RRULE`, `RDATE` or overrides.
    pub recurring: bool,
    /// Contacts: the name to sort by.
    pub sort_name: Option<String>,
}

/// Whether a resource's text parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// It did: shown over JMAP, with these indexes.
    Valid(ResourceIndex),
    /// It did not: kept (its etag stops refetching it), never shown.
    Invalid,
}

impl Parsed {
    /// It parsed.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid(_))
    }

    fn index(&self) -> Option<&ResourceIndex> {
        match self {
            Self::Valid(index) => Some(index),
            Self::Invalid => None,
        }
    }

    pub(crate) fn uid(&self) -> Option<&str> {
        self.index().and_then(|index| index.uid.as_deref())
    }
}

/// A resource fetched from the server, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceWrite {
    /// Where it is.
    pub href: DavHref,
    /// Its etag, if the server gave one.
    pub etag: Option<String>,
    /// The text, sealed with the account's data key (`nebula-crypto`,
    /// `Field::DavResource`).
    pub sealed: Vec<u8>,
    /// Bytes of the text.
    pub size: u64,
    /// Its indexes, or that it did not parse.
    pub parsed: Parsed,
}

/// A resource in the store, without its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredResource {
    /// Its id (the JMAP id).
    pub id: ResourceId,
    /// Its collection.
    pub collection: CollectionId,
    /// Where it is.
    pub href: DavHref,
    /// Its etag, if known.
    pub etag: Option<String>,
    /// Bytes of the text.
    pub size: u64,
    /// Its indexes, or that it did not parse.
    pub parsed: Parsed,
    /// When it was last written.
    pub updated_at: UnixTime,
    /// Gone from the server since then (hidden).
    pub orphaned_at: Option<UnixTime>,
}

impl StoredResource {
    /// Shown over JMAP: on the server and parsed.
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        self.orphaned_at.is_none() && self.parsed.is_valid()
    }
}
