// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! XML request bodies, the shapes libdav 0.11 sends (they work with
//! Radicale, Nextcloud/SabreDAV, Fastmail/Cyrus, iCloud, Stalwart…).
//!
//! Every element name is one of our constants; every value from outside
//! (names, colours, sync tokens, hrefs) is escaped, and a character XML
//! cannot carry is an [`EncodeError`], never silently dropped.

use std::num::NonZeroU32;

use crate::xml::Writer;
use crate::{
    CalendarColor, ComponentSet, EncodeError, Href, Namespace, Property, ResourceType, SyncToken,
    VcardRequest,
};

const DAV: Namespace = Namespace::Dav;

/// Content type of every XML body we send.
pub const XML_CONTENT_TYPE: &str = "application/xml; charset=utf-8";

/// Longest text value we write (descriptions, time zones).
pub const MAX_VALUE_LEN: usize = 1024 * 1024;

/// A property value to set (PROPPATCH, MKCALENDAR, extended MKCOL).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PropValue {
    /// `DAV:displayname`.
    DisplayName(String),
    /// `C:calendar-description`.
    CalendarDescription(String),
    /// `C:calendar-timezone` (a `VCALENDAR` with one `VTIMEZONE`).
    CalendarTimezone(String),
    /// `C:supported-calendar-component-set` (only when creating).
    SupportedComponents(ComponentSet),
    /// `I:calendar-color`.
    CalendarColor(CalendarColor),
    /// `I:calendar-order`.
    CalendarOrder(i32),
    /// `CR:addressbook-description`.
    AddressbookDescription(String),
    /// `C:calendar-timezone-id` (RFC 7809): an IANA zone name.
    CalendarTimezoneId(String),
    /// `C:default-alarm-vevent-datetime`: `VALARM` components, or empty
    /// for none (draft-daboo-valarm-extensions §9).
    DefaultAlarmVeventDatetime(String),
    /// `C:default-alarm-vevent-date`: the same for all-day events.
    DefaultAlarmVeventDate(String),
    /// `CS:source`: where a new subscription comes from (the DAV server
    /// fetches it, never the bridge).
    Source(crate::ExternalUrl),
}

impl PropValue {
    /// The property it sets.
    #[must_use]
    pub const fn property(&self) -> Property {
        match self {
            Self::DisplayName(_) => Property::DisplayName,
            Self::CalendarDescription(_) => Property::CalendarDescription,
            Self::CalendarTimezone(_) => Property::CalendarTimezone,
            Self::SupportedComponents(_) => Property::SupportedCalendarComponentSet,
            Self::CalendarColor(_) => Property::CalendarColor,
            Self::CalendarOrder(_) => Property::CalendarOrder,
            Self::AddressbookDescription(_) => Property::AddressbookDescription,
            Self::CalendarTimezoneId(_) => Property::CalendarTimezoneId,
            Self::DefaultAlarmVeventDatetime(_) => Property::DefaultAlarmVeventDatetime,
            Self::DefaultAlarmVeventDate(_) => Property::DefaultAlarmVeventDate,
            Self::Source(_) => Property::Source,
        }
    }

    fn write(&self, w: &mut Writer) -> Result<(), EncodeError> {
        let (namespace, local) = self.property().name();
        let text = |w: &mut Writer, value: &str| -> Result<(), EncodeError> {
            if value.len() > MAX_VALUE_LEN {
                return Err(EncodeError::Range);
            }
            w.text_element(namespace, local, value).map(|_| ())
        };
        match self {
            Self::DisplayName(value)
            | Self::CalendarDescription(value)
            | Self::CalendarTimezone(value)
            | Self::AddressbookDescription(value)
            | Self::CalendarTimezoneId(value)
            | Self::DefaultAlarmVeventDatetime(value)
            | Self::DefaultAlarmVeventDate(value) => text(w, value),
            Self::CalendarColor(color) => text(w, &color.to_string()),
            Self::CalendarOrder(order) => text(w, &order.to_string()),
            Self::SupportedComponents(set) => {
                w.start(namespace, local);
                for component in set.components() {
                    w.empty_with(Namespace::CalDav, "comp", &[("name", component.as_str())])?;
                }
                w.end();
                Ok(())
            }
            Self::Source(url) => {
                w.start(namespace, local);
                w.text_element(DAV, "href", url.as_str())?;
                w.end();
                Ok(())
            }
        }
    }
}

fn write_prop_names(w: &mut Writer, properties: &[Property]) {
    w.start(DAV, "prop");
    for property in properties {
        let (namespace, local) = property.name();
        w.empty(namespace, local);
    }
    w.end();
}

/// `PROPFIND` for these properties.
///
/// ```
/// use nebula_dav_proto::{request, Property};
///
/// let body = request::propfind(&[Property::CurrentUserPrincipal]);
/// assert!(body.ends_with("<D:prop><D:current-user-principal/></D:prop></D:propfind>"));
/// ```
#[must_use]
pub fn propfind(properties: &[Property]) -> String {
    let mut w = Writer::new(DAV, "propfind");
    write_prop_names(&mut w, properties);
    w.finish()
}

/// `PROPPATCH`: set some properties, remove others.
///
/// # Errors
///
/// A value XML cannot carry, or longer than [`MAX_VALUE_LEN`].
pub fn proppatch(set: &[PropValue], remove: &[Property]) -> Result<String, EncodeError> {
    let mut w = Writer::new(DAV, "propertyupdate");
    if !set.is_empty() {
        w.start(DAV, "set").start(DAV, "prop");
        for value in set {
            value.write(&mut w)?;
        }
        w.end().end();
    }
    if !remove.is_empty() {
        w.start(DAV, "remove");
        write_prop_names(&mut w, remove);
        w.end();
    }
    Ok(w.finish())
}

/// `REPORT sync-collection` (RFC 6578 §3.2), `sync-level` 1. Without a
/// token, the initial sync (everything). `limit` asks the server for at
/// most that many results (it may answer 507 and a new token).
///
/// ```
/// use nebula_dav_proto::{request, Property, SyncToken};
///
/// let token = SyncToken::parse("http://example.com/sync/1&2").unwrap();
/// let body = request::sync_collection(Some(&token), &[Property::GetETag], None).unwrap();
/// assert!(body.contains("<D:sync-token>http://example.com/sync/1&amp;2</D:sync-token>"));
/// let initial = request::sync_collection(None, &[Property::GetETag], None).unwrap();
/// assert!(initial.contains("<D:sync-token></D:sync-token>"));
/// ```
///
/// # Errors
///
/// Never in practice: a parsed [`SyncToken`] is printable ASCII.
pub fn sync_collection(
    token: Option<&SyncToken>,
    properties: &[Property],
    limit: Option<NonZeroU32>,
) -> Result<String, EncodeError> {
    let mut w = Writer::new(DAV, "sync-collection");
    w.text_element(DAV, "sync-token", token.map_or("", SyncToken::as_str))?;
    w.text_element(DAV, "sync-level", "1")?;
    if let Some(limit) = limit {
        w.start(DAV, "limit");
        w.text_element(DAV, "nresults", &limit.to_string())?;
        w.end();
    }
    write_prop_names(&mut w, properties);
    Ok(w.finish())
}

fn multiget(
    namespace: Namespace,
    local: &'static str,
    hrefs: &[Href],
    properties: &[Property],
) -> Result<String, EncodeError> {
    let mut w = Writer::new(namespace, local);
    write_prop_names(&mut w, properties);
    for href in hrefs {
        w.text_element(DAV, "href", href.path())?;
    }
    Ok(w.finish())
}

/// `REPORT calendar-multiget` (RFC 4791 §7.9) with `getetag` and
/// `calendar-data`. Hrefs go as canonical paths.
///
/// # Errors
///
/// Never in practice: a canonical path is ASCII.
pub fn calendar_multiget(hrefs: &[Href]) -> Result<String, EncodeError> {
    multiget(
        Namespace::CalDav,
        "calendar-multiget",
        hrefs,
        &[Property::GetETag, Property::CalendarData],
    )
}

/// `REPORT addressbook-multiget` (RFC 6352 §8.7) with `getetag` and
/// `address-data`, the cards as the server stores them.
///
/// # Errors
///
/// Never in practice: a canonical path is ASCII.
pub fn addressbook_multiget(hrefs: &[Href]) -> Result<String, EncodeError> {
    addressbook_multiget_as(hrefs, VcardRequest::AsStored)
}

/// `REPORT addressbook-multiget` asking for `wanted`: with
/// [`VcardRequest::Version4`], `address-data` says
/// `content-type="text/vcard" version="4.0"` (RFC 6352 §10.4).
///
/// ```
/// use nebula_dav_proto::{request, VcardRequest};
///
/// let body = request::addressbook_multiget_as(&[], VcardRequest::Version4).unwrap();
/// assert!(body.contains(r#"<CR:address-data content-type="text/vcard" version="4.0"/>"#));
/// let plain = request::addressbook_multiget_as(&[], VcardRequest::AsStored).unwrap();
/// assert!(plain.contains("<CR:address-data/>"));
/// ```
///
/// # Errors
///
/// Never in practice: a canonical path is ASCII.
pub fn addressbook_multiget_as(
    hrefs: &[Href],
    wanted: VcardRequest,
) -> Result<String, EncodeError> {
    match wanted {
        VcardRequest::AsStored => multiget(
            Namespace::CardDav,
            "addressbook-multiget",
            hrefs,
            &[Property::GetETag, Property::AddressData],
        ),
        VcardRequest::Version4 => {
            let mut w = Writer::new(Namespace::CardDav, "addressbook-multiget");
            w.start(DAV, "prop");
            let (namespace, local) = Property::GetETag.name();
            w.empty(namespace, local);
            let (namespace, local) = Property::AddressData.name();
            w.empty_with(
                namespace,
                local,
                &[("content-type", "text/vcard"), ("version", "4.0")],
            )?;
            w.end();
            for href in hrefs {
                w.text_element(DAV, "href", href.path())?;
            }
            Ok(w.finish())
        }
    }
}

/// Longest text a `principal-property-search` matches (an address).
pub const MAX_MATCH_LEN: usize = 1024;

/// `REPORT principal-search-property-set` (RFC 3744 §9.5): which
/// properties the server lets clients search. The root is written as an
/// empty element (`<D:principal-search-property-set/>`): sabre/dav
/// (Nextcloud) answers 400 to `<…></…>` ("must be empty").
///
/// ```
/// use nebula_dav_proto::request;
///
/// assert!(request::principal_search_property_set().ends_with("\"/>"));
/// ```
#[must_use]
pub fn principal_search_property_set() -> String {
    Writer::new(DAV, "principal-search-property-set").finish_empty()
}

/// `REPORT principal-property-search` (RFC 3744 §9.4) on a principal
/// collection: the principals whose `property` contains `text` (the
/// server decides how it matches; callers check the results), with
/// `properties`.
///
/// ```
/// use nebula_dav_proto::{request, Property};
///
/// let body = request::principal_property_search(
///     Property::CalendarUserAddressSet,
///     "bob@example.com",
///     &[Property::DisplayName],
/// )
/// .unwrap();
/// assert!(body.contains("<D:property-search><D:prop><C:calendar-user-address-set/></D:prop><D:match>bob@example.com</D:match></D:property-search>"));
/// ```
///
/// # Errors
///
/// An empty `text`, longer than [`MAX_MATCH_LEN`], or with characters XML
/// cannot carry.
pub fn principal_property_search(
    property: Property,
    text: &str,
    properties: &[Property],
) -> Result<String, EncodeError> {
    if text.is_empty() || text.len() > MAX_MATCH_LEN {
        return Err(EncodeError::Range);
    }
    let mut w = Writer::new(DAV, "principal-property-search");
    w.start(DAV, "property-search");
    write_prop_names(&mut w, &[property]);
    w.text_element(DAV, "match", text)?;
    w.end();
    write_prop_names(&mut w, properties);
    Ok(w.finish())
}

/// The body of an `ACL` request (RFC 3744 §8.1), see
/// [`Acl`](crate::Acl): these ACEs replace every editable one; protected
/// and inherited ACEs are left out.
///
/// ```
/// use url::Url;
/// use nebula_dav_proto::{Ace, Href, Privilege, request};
///
/// let base = Href::from_url(&Url::parse("https://dav.example.com/").unwrap()).unwrap();
/// let bob = Href::resolve("/dav/pal/bob%40example.com/", &base).unwrap();
/// let body = request::acl(&[Ace::grant(bob, &[Privilege::Read])]).unwrap();
/// assert!(body.contains("<D:ace><D:principal><D:href>/dav/pal/bob@example.com/</D:href></D:principal><D:grant><D:privilege><D:read/></D:privilege></D:grant></D:ace>"));
/// ```
///
/// # Errors
///
/// An ACE with an unknown principal or without privileges, or more than
/// [`MAX_ACES`](crate::MAX_ACES).
pub fn acl<'a>(aces: impl IntoIterator<Item = &'a crate::Ace>) -> Result<String, EncodeError> {
    crate::sharing::acl_body(aces)
}

/// The body of a Nextcloud sharing `POST` (`O:share`): `set` adds or
/// changes sharees (`O:read-write` or read), `remove` stops sharing with
/// others.
///
/// ```
/// use nebula_dav_proto::{OcPrincipal, ShareAccess, request};
///
/// let bob = OcPrincipal::parse("principal:principals/users/bob").unwrap();
/// let body = request::oc_share(&[(bob, ShareAccess::ReadWrite)], &[]).unwrap();
/// assert!(body.contains("<O:set><D:href>principal:principals/users/bob</D:href><O:read-write/></O:set>"));
/// ```
///
/// # Errors
///
/// Nothing to do, or more than
/// [`MAX_OC_SHARE_REQUEST`](crate::MAX_OC_SHARE_REQUEST) entries (a bigger
/// change goes in several requests).
pub fn oc_share(
    set: &[(crate::OcPrincipal, crate::ShareAccess)],
    remove: &[crate::OcPrincipal],
) -> Result<String, EncodeError> {
    crate::sharing::oc_share_body(set, remove)
}

/// The body of a Calendar Server sharing `POST` (`CS:share`): `set` adds or
/// changes sharees, `remove` stops sharing with others.
///
/// # Errors
///
/// Nothing to do, or more than
/// [`MAX_SHARE_REQUEST`](crate::MAX_SHARE_REQUEST) entries (a bigger
/// change goes in several requests).
pub fn share(
    set: &[(crate::Sharee, crate::ShareAccess)],
    remove: &[crate::Sharee],
) -> Result<String, EncodeError> {
    crate::sharing::share_body(set, remove)
}

fn write_set(
    w: &mut Writer,
    resource_type: Option<ResourceType>,
    values: &[PropValue],
) -> Result<(), EncodeError> {
    w.start(DAV, "set").start(DAV, "prop");
    if let Some(kind) = resource_type {
        w.start(DAV, "resourcetype");
        w.empty(DAV, "collection");
        if kind.calendar {
            w.empty(Namespace::CalDav, "calendar");
        }
        if kind.addressbook {
            w.empty(Namespace::CardDav, "addressbook");
        }
        if kind.subscribed {
            w.empty(Namespace::CalendarServer, "subscribed");
        }
        w.end();
    }
    for value in values {
        value.write(w)?;
    }
    w.end().end();
    Ok(())
}

/// `MKCALENDAR` (RFC 4791 §5.3.1) with initial properties. Without
/// properties there is no body (allowed by the RFC).
///
/// # Errors
///
/// A value XML cannot carry, or too long.
pub fn mkcalendar(values: &[PropValue]) -> Result<Option<String>, EncodeError> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut w = Writer::new(Namespace::CalDav, "mkcalendar");
    write_set(&mut w, None, values)?;
    Ok(Some(w.finish()))
}

/// What an extended `MKCOL` creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionKind {
    /// A calendar (libdav creates calendars this way too; some servers
    /// only have MKCALENDAR).
    Calendar,
    /// An address book (CardDAV has no MKADDRESSBOOK, RFC 6352 §6.3.1).
    Addressbook,
}

/// Extended `MKCOL` (RFC 5689 §5.1) with `resourcetype` and properties.
///
/// ```
/// use nebula_dav_proto::request::{self, CollectionKind, PropValue};
///
/// let body = request::mkcol(CollectionKind::Addressbook, &[PropValue::DisplayName("Friends & co".into())]).unwrap();
/// assert!(body.contains("<D:resourcetype><D:collection/><CR:addressbook/></D:resourcetype>"));
/// assert!(body.contains("<D:displayname>Friends &amp; co</D:displayname>"));
/// ```
///
/// # Errors
///
/// A value XML cannot carry, or too long.
pub fn mkcol(kind: CollectionKind, values: &[PropValue]) -> Result<String, EncodeError> {
    let resource_type = ResourceType {
        collection: true,
        calendar: kind == CollectionKind::Calendar,
        addressbook: kind == CollectionKind::Addressbook,
        ..ResourceType::default()
    };
    let mut w = Writer::new(DAV, "mkcol");
    write_set(&mut w, Some(resource_type), values)?;
    Ok(w.finish())
}

/// Extended `MKCOL` of a webcal subscription (`CS:subscribed`, as
/// Apple's and Nextcloud's clients create them) with its `CS:source`
/// and other properties (name, colour). The server fetches the source.
///
/// ```
/// use nebula_dav_proto::{request::{self, PropValue}, ExternalUrl};
///
/// let source = ExternalUrl::subscription("webcal://example.com/h.ics?a=1&b=2").unwrap();
/// let body = request::subscribe(&source, &[PropValue::DisplayName("Holidays".into())]).unwrap();
/// assert!(body.contains("<D:resourcetype><D:collection/><CS:subscribed/></D:resourcetype>"));
/// assert!(body.contains("<CS:source><D:href>webcal://example.com/h.ics?a=1&amp;b=2</D:href></CS:source>"));
/// ```
///
/// # Errors
///
/// A value XML cannot carry, or too long.
pub fn subscribe(source: &crate::ExternalUrl, values: &[PropValue]) -> Result<String, EncodeError> {
    let resource_type = ResourceType {
        collection: true,
        subscribed: true,
        ..ResourceType::default()
    };
    let mut all = Vec::with_capacity(values.len().saturating_add(1));
    all.extend(
        values
            .iter()
            .filter(|v| !matches!(v, PropValue::Source(_)))
            .cloned(),
    );
    all.push(PropValue::Source(source.clone()));
    let mut w = Writer::new(DAV, "mkcol");
    write_set(&mut w, Some(resource_type), &all)?;
    Ok(w.finish())
}

/// The `POST` body that publishes a calendar at a public link
/// (`CS:publish-calendar`, Nextcloud's `oc-calendar-publishing`) or stops
/// publishing it (`CS:unpublish-calendar`).
///
/// ```
/// use nebula_dav_proto::request;
///
/// assert!(request::publish_calendar(true).ends_with(r#"xmlns:NC="http://nextcloud.com/ns"/>"#));
/// assert!(request::publish_calendar(true).contains("<CS:publish-calendar "));
/// assert!(request::publish_calendar(false).contains("<CS:unpublish-calendar "));
/// ```
#[must_use]
pub fn publish_calendar(publish: bool) -> String {
    let local = if publish {
        "publish-calendar"
    } else {
        "unpublish-calendar"
    };
    Writer::new(Namespace::CalendarServer, local).finish_empty()
}

/// The `REPORT calendar-query` that lists Nextcloud's deleted events
/// (`<home>/trashbin/objects/`, which answers no `PROPFIND Depth 1`):
/// when each went to the bin, its calendar and, `with_data`, its data
/// (without it, only what restoring or deleting needs: the answer stays
/// small however big the events are).
///
/// ```
/// use nebula_dav_proto::request;
///
/// let body = request::trash_bin_query(true);
/// assert!(body.contains("<NC:deleted-at/><NC:calendar-uri/><D:getetag/><C:calendar-data/>"));
/// assert!(body.contains(r#"<C:filter><C:comp-filter name="VCALENDAR"/></C:filter>"#));
/// let hrefs = request::trash_bin_query(false);
/// assert!(hrefs.contains("<NC:deleted-at/><NC:calendar-uri/><D:getetag/></D:prop>"));
/// assert!(!hrefs.contains("calendar-data"));
/// ```
#[must_use]
pub fn trash_bin_query(with_data: bool) -> String {
    let mut w = Writer::new(Namespace::CalDav, "calendar-query");
    let all = [
        Property::DeletedAt,
        Property::CalendarUri,
        Property::GetETag,
        Property::CalendarData,
    ];
    let properties = if with_data {
        all.as_slice()
    } else {
        all.get(..3).unwrap_or_default()
    };
    write_prop_names(&mut w, properties);
    w.start(Namespace::CalDav, "filter");
    // A constant name: never fails.
    let _ = w.empty_with(Namespace::CalDav, "comp-filter", &[("name", "VCALENDAR")]);
    w.end();
    w.finish()
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{CollectionKind, PropValue, mkcalendar, mkcol, propfind, proppatch};
    use crate::{
        CalendarColor, Component, ComponentSet, EncodeError, Href, Property, XmlLimits, request,
    };

    const NAMESPACES: &str = concat!(
        r#" xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav""#,
        r#" xmlns:CR="urn:ietf:params:xml:ns:carddav" xmlns:CS="http://calendarserver.org/ns/""#,
        r#" xmlns:I="http://apple.com/ns/ical/" xmlns:O="http://owncloud.org/ns""#,
        r#" xmlns:S="http://sabredav.org/ns" xmlns:NC="http://nextcloud.com/ns""#,
    );

    #[test]
    fn propfind_shape() {
        assert_eq!(
            propfind(&[Property::ResourceType, Property::GetCTag]),
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?><D:propfind{NAMESPACES}><D:prop><D:resourcetype/><CS:getctag/></D:prop></D:propfind>"#
            )
        );
    }

    #[test]
    fn proppatch_shape_and_escaping() {
        let body = proppatch(
            &[
                PropValue::DisplayName("<Work> & \"co\"".into()),
                PropValue::CalendarColor(CalendarColor::parse("#ff0000").unwrap()),
                PropValue::CalendarOrder(-3),
            ],
            &[Property::CalendarDescription],
        )
        .unwrap();
        assert!(body.contains(concat!(
            "<D:set><D:prop><D:displayname>&lt;Work&gt; &amp; &quot;co&quot;</D:displayname>",
            "<I:calendar-color>#FF0000</I:calendar-color><I:calendar-order>-3</I:calendar-order>",
            "</D:prop></D:set><D:remove><D:prop><C:calendar-description/></D:prop></D:remove>",
        )));
        assert_eq!(
            proppatch(&[PropValue::DisplayName("a\u{1}b".into())], &[]),
            Err(EncodeError::XmlCharacter)
        );
        assert_eq!(
            proppatch(
                &[PropValue::CalendarDescription(
                    "a".repeat(request::MAX_VALUE_LEN + 1)
                )],
                &[]
            ),
            Err(EncodeError::Range)
        );
    }

    #[test]
    fn calendar_defaults_shape() {
        let body = proppatch(
            &[
                PropValue::CalendarTimezoneId("Europe/Madrid".into()),
                PropValue::DefaultAlarmVeventDatetime(
                    "BEGIN:VALARM\r\nTRIGGER:-PT5M\r\nEND:VALARM".into(),
                ),
                PropValue::DefaultAlarmVeventDate(String::new()),
            ],
            &[],
        )
        .unwrap();
        assert!(body.contains(concat!(
            "<C:calendar-timezone-id>Europe/Madrid</C:calendar-timezone-id>",
            "<C:default-alarm-vevent-datetime>BEGIN:VALARM",
        )));
        assert!(body.contains("<C:default-alarm-vevent-date></C:default-alarm-vevent-date>"));
    }

    #[test]
    fn mkcalendar_shape() {
        assert_eq!(mkcalendar(&[]), Ok(None));
        let body = mkcalendar(&[
            PropValue::DisplayName("Trabajo".into()),
            PropValue::SupportedComponents(ComponentSet::new(&[Component::Event])),
        ])
        .unwrap()
        .unwrap();
        assert!(body.contains("<C:mkcalendar"));
        assert!(body.contains(concat!(
            "<D:set><D:prop><D:displayname>Trabajo</D:displayname>",
            r#"<C:supported-calendar-component-set><C:comp name="VEVENT"/></C:supported-calendar-component-set>"#,
            "</D:prop></D:set></C:mkcalendar>",
        )));
        let calendar = mkcol(CollectionKind::Calendar, &[]).unwrap();
        assert!(calendar.contains("<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>"));
    }

    #[test]
    fn multiget_writes_canonical_paths() {
        let base = Href::from_url(&Url::parse("https://dav.example.com/cal/").unwrap()).unwrap();
        let hrefs = [
            Href::resolve("caf%c3%a9.ics", &base).unwrap(),
            Href::resolve("a&b.ics", &base).unwrap(),
        ];
        let body = request::calendar_multiget(&hrefs).unwrap();
        assert!(body.contains(concat!(
            "<D:prop><D:getetag/><C:calendar-data/></D:prop>",
            "<D:href>/cal/caf%C3%A9.ics</D:href><D:href>/cal/a&amp;b.ics</D:href>",
        )));
        let cards = request::addressbook_multiget(&hrefs[..1]).unwrap();
        assert!(cards.contains("<CR:addressbook-multiget"));
        assert!(cards.contains("<CR:address-data/>"));
    }

    /// Every body we write is well-formed and passes our own parser.
    #[test]
    fn bodies_parse_back() {
        let bodies = [
            propfind(&Property::ALL),
            proppatch(
                &[PropValue::CalendarTimezone(
                    "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".into(),
                )],
                &Property::ALL,
            )
            .unwrap(),
            request::sync_collection(None, &Property::ALL, std::num::NonZeroU32::new(10)).unwrap(),
            mkcol(
                CollectionKind::Addressbook,
                &[PropValue::AddressbookDescription("x".into())],
            )
            .unwrap(),
        ];
        for body in bodies {
            let document =
                crate::xml::parse_document(body.as_bytes(), &XmlLimits::default()).unwrap();
            assert_eq!(document.root_element().tag_name().namespace(), Some("DAV:"));
        }
    }
}
