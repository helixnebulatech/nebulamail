// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! XML namespaces and the properties we ask for.

/// The namespaces we read and write. Elements are matched by namespace
/// URI and local name, never by prefix (a server may use any prefix, or
/// none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// `DAV:` (RFC 4918).
    Dav,
    /// `urn:ietf:params:xml:ns:caldav` (RFC 4791, RFC 6638).
    CalDav,
    /// `urn:ietf:params:xml:ns:carddav` (RFC 6352).
    CardDav,
    /// `http://calendarserver.org/ns/` (`getctag`, sharing).
    CalendarServer,
    /// `http://apple.com/ns/ical/` (`calendar-color`, `calendar-order`).
    AppleIcal,
    /// `http://owncloud.org/ns` (Nextcloud's sharing: `oc:share`,
    /// `oc:invite`).
    OwnCloud,
    /// `http://sabredav.org/ns` (`email-address`, a principal search
    /// property of sabre/dav and Nextcloud).
    Sabre,
    /// `http://nextcloud.com/ns` (Nextcloud's calendar trash bin:
    /// `deleted-at`, `calendar-uri`).
    Nextcloud,
}

impl Namespace {
    /// The namespace URI.
    #[must_use]
    pub const fn uri(self) -> &'static str {
        match self {
            Self::Dav => "DAV:",
            Self::CalDav => "urn:ietf:params:xml:ns:caldav",
            Self::CardDav => "urn:ietf:params:xml:ns:carddav",
            Self::CalendarServer => "http://calendarserver.org/ns/",
            Self::AppleIcal => "http://apple.com/ns/ical/",
            Self::OwnCloud => "http://owncloud.org/ns",
            Self::Sabre => "http://sabredav.org/ns",
            Self::Nextcloud => "http://nextcloud.com/ns",
        }
    }

    /// The prefix used in the bodies we write (declared on the root).
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Dav => "D",
            Self::CalDav => "C",
            Self::CardDav => "CR",
            Self::CalendarServer => "CS",
            Self::AppleIcal => "I",
            Self::OwnCloud => "O",
            Self::Sabre => "S",
            Self::Nextcloud => "NC",
        }
    }

    /// Every namespace, in declaration order.
    pub const ALL: [Self; 8] = [
        Self::Dav,
        Self::CalDav,
        Self::CardDav,
        Self::CalendarServer,
        Self::AppleIcal,
        Self::OwnCloud,
        Self::Sabre,
        Self::Nextcloud,
    ];

    pub(crate) fn from_uri(uri: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ns| ns.uri() == uri)
    }
}

/// A property we can request (PROPFIND, multiget, sync-collection) or set
/// (PROPPATCH, MKCALENDAR).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Property {
    /// `DAV:resourcetype`.
    ResourceType,
    /// `DAV:displayname`.
    DisplayName,
    /// `DAV:getetag`.
    GetETag,
    /// `DAV:getcontenttype`.
    GetContentType,
    /// `DAV:sync-token` (RFC 6578).
    SyncToken,
    /// `DAV:current-user-principal` (RFC 5397).
    CurrentUserPrincipal,
    /// `DAV:current-user-privilege-set` (RFC 3744).
    CurrentUserPrivilegeSet,
    /// `DAV:supported-report-set` (RFC 3253).
    SupportedReportSet,
    /// `CS:getctag`.
    GetCTag,
    /// `C:calendar-home-set`.
    CalendarHomeSet,
    /// `C:calendar-user-address-set` (RFC 6638).
    CalendarUserAddressSet,
    /// `C:schedule-outbox-URL` (RFC 6638).
    ScheduleOutboxUrl,
    /// `C:schedule-inbox-URL` (RFC 6638).
    ScheduleInboxUrl,
    /// `C:calendar-description`.
    CalendarDescription,
    /// `C:calendar-timezone`.
    CalendarTimezone,
    /// `C:supported-calendar-component-set`.
    SupportedCalendarComponentSet,
    /// `C:calendar-data`.
    CalendarData,
    /// `CR:addressbook-home-set`.
    AddressbookHomeSet,
    /// `CR:addressbook-description`.
    AddressbookDescription,
    /// `CR:supported-address-data`.
    SupportedAddressData,
    /// `CR:address-data`.
    AddressData,
    /// `I:calendar-color`.
    CalendarColor,
    /// `I:calendar-order`.
    CalendarOrder,
    /// `DAV:principal-collection-set` (RFC 3744 §5.8): where the server's
    /// principals are (for principal searches).
    PrincipalCollectionSet,
    /// `C:calendar-user-type` (RFC 6638 §2.4.2): `INDIVIDUAL`, `GROUP`,
    /// `RESOURCE`, `ROOM`…
    CalendarUserType,
    /// `DAV:owner` (RFC 3744 §5.1): the principal that owns it.
    Owner,
    /// `DAV:acl` (RFC 3744 §5.5).
    Acl,
    /// `CS:invite` (Calendar Server sharing): who it is shared with.
    Invite,
    /// `CS:allowed-sharing-modes`: whether it can be shared.
    AllowedSharingModes,
    /// `O:invite` (Nextcloud sharing): who it is shared with.
    OcInvite,
    /// `S:email-address`: a principal's address, searchable in sabre/dav
    /// and Nextcloud (never read as a value).
    EmailAddress,
    /// `C:calendar-timezone-id` (RFC 7809 §5.2): the calendar's IANA zone.
    CalendarTimezoneId,
    /// `C:default-alarm-vevent-datetime` (draft-daboo-valarm-extensions
    /// §9): `VALARM`s for new events with a time.
    DefaultAlarmVeventDatetime,
    /// `C:default-alarm-vevent-date`: `VALARM`s for new all-day events.
    DefaultAlarmVeventDate,
    /// `CS:source`: where a webcal subscription comes from (a URL the
    /// bridge never fetches).
    Source,
    /// `CS:publish-url`: the public link of a published calendar
    /// (Nextcloud), only shown.
    PublishUrl,
    /// `NC:deleted-at`: when something went to Nextcloud's trash bin.
    DeletedAt,
    /// `NC:calendar-uri`: the calendar a deleted event was in (its last
    /// path segment).
    CalendarUri,
    /// `NC:trash-bin-retention-duration`: seconds things stay in the
    /// trash bin.
    TrashBinRetentionDuration,
}

impl Property {
    /// Every property (for tests and lookups).
    pub const ALL: [Self; 39] = [
        Self::ResourceType,
        Self::DisplayName,
        Self::GetETag,
        Self::GetContentType,
        Self::SyncToken,
        Self::CurrentUserPrincipal,
        Self::CurrentUserPrivilegeSet,
        Self::SupportedReportSet,
        Self::GetCTag,
        Self::CalendarHomeSet,
        Self::CalendarUserAddressSet,
        Self::ScheduleOutboxUrl,
        Self::ScheduleInboxUrl,
        Self::CalendarDescription,
        Self::CalendarTimezone,
        Self::SupportedCalendarComponentSet,
        Self::CalendarData,
        Self::AddressbookHomeSet,
        Self::AddressbookDescription,
        Self::SupportedAddressData,
        Self::AddressData,
        Self::CalendarColor,
        Self::CalendarOrder,
        Self::PrincipalCollectionSet,
        Self::CalendarUserType,
        Self::Owner,
        Self::Acl,
        Self::Invite,
        Self::AllowedSharingModes,
        Self::OcInvite,
        Self::EmailAddress,
        Self::CalendarTimezoneId,
        Self::DefaultAlarmVeventDatetime,
        Self::DefaultAlarmVeventDate,
        Self::Source,
        Self::PublishUrl,
        Self::DeletedAt,
        Self::CalendarUri,
        Self::TrashBinRetentionDuration,
    ];

    /// Namespace and local name.
    #[must_use]
    pub const fn name(self) -> (Namespace, &'static str) {
        use Namespace::{
            AppleIcal, CalDav, CalendarServer, CardDav, Dav, Nextcloud, OwnCloud, Sabre,
        };
        match self {
            Self::ResourceType => (Dav, "resourcetype"),
            Self::DisplayName => (Dav, "displayname"),
            Self::GetETag => (Dav, "getetag"),
            Self::GetContentType => (Dav, "getcontenttype"),
            Self::SyncToken => (Dav, "sync-token"),
            Self::CurrentUserPrincipal => (Dav, "current-user-principal"),
            Self::CurrentUserPrivilegeSet => (Dav, "current-user-privilege-set"),
            Self::SupportedReportSet => (Dav, "supported-report-set"),
            Self::GetCTag => (CalendarServer, "getctag"),
            Self::CalendarHomeSet => (CalDav, "calendar-home-set"),
            Self::CalendarUserAddressSet => (CalDav, "calendar-user-address-set"),
            Self::ScheduleOutboxUrl => (CalDav, "schedule-outbox-URL"),
            Self::ScheduleInboxUrl => (CalDav, "schedule-inbox-URL"),
            Self::CalendarDescription => (CalDav, "calendar-description"),
            Self::CalendarTimezone => (CalDav, "calendar-timezone"),
            Self::SupportedCalendarComponentSet => (CalDav, "supported-calendar-component-set"),
            Self::CalendarData => (CalDav, "calendar-data"),
            Self::AddressbookHomeSet => (CardDav, "addressbook-home-set"),
            Self::AddressbookDescription => (CardDav, "addressbook-description"),
            Self::SupportedAddressData => (CardDav, "supported-address-data"),
            Self::AddressData => (CardDav, "address-data"),
            Self::CalendarColor => (AppleIcal, "calendar-color"),
            Self::CalendarOrder => (AppleIcal, "calendar-order"),
            Self::PrincipalCollectionSet => (Dav, "principal-collection-set"),
            Self::CalendarUserType => (CalDav, "calendar-user-type"),
            Self::Owner => (Dav, "owner"),
            Self::Acl => (Dav, "acl"),
            Self::Invite => (CalendarServer, "invite"),
            Self::AllowedSharingModes => (CalendarServer, "allowed-sharing-modes"),
            Self::OcInvite => (OwnCloud, "invite"),
            Self::EmailAddress => (Sabre, "email-address"),
            Self::CalendarTimezoneId => (CalDav, "calendar-timezone-id"),
            Self::DefaultAlarmVeventDatetime => (CalDav, "default-alarm-vevent-datetime"),
            Self::DefaultAlarmVeventDate => (CalDav, "default-alarm-vevent-date"),
            Self::Source => (CalendarServer, "source"),
            Self::PublishUrl => (CalendarServer, "publish-url"),
            Self::DeletedAt => (Nextcloud, "deleted-at"),
            Self::CalendarUri => (Nextcloud, "calendar-uri"),
            Self::TrashBinRetentionDuration => (Nextcloud, "trash-bin-retention-duration"),
        }
    }

    /// The property with this exact namespace URI and local name.
    #[must_use]
    pub fn from_name(namespace: Option<&str>, local: &str) -> Option<Self> {
        let namespace = Namespace::from_uri(namespace?)?;
        Self::ALL
            .into_iter()
            .find(|p| p.name() == (namespace, local))
    }

    /// The property with this local name in any namespace. Only for the
    /// statuses of a PROPPATCH answer: Cyrus answers in the wrong namespace
    /// (cyrus-imapd#4489, a quirk libdav works around too). Never used for
    /// values.
    pub(crate) fn from_local_name(local: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name().1 == local)
    }
}

#[cfg(test)]
mod tests {
    use super::{Namespace, Property};

    #[test]
    fn names_are_unique() {
        for (i, a) in Property::ALL.iter().enumerate() {
            for b in Property::ALL.iter().skip(i + 1) {
                assert_ne!(a.name(), b.name());
            }
        }
    }

    #[test]
    fn lookup_needs_the_namespace() {
        assert_eq!(
            Property::from_name(Some("DAV:"), "getetag"),
            Some(Property::GetETag)
        );
        assert_eq!(Property::from_name(None, "getetag"), None);
        assert_eq!(
            Property::from_name(Some(Namespace::CalDav.uri()), "getetag"),
            None
        );
        assert_eq!(Property::from_name(Some("dav:"), "getetag"), None);
    }
}
