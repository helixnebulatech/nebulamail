// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `207 Multi-Status` bodies (RFC 4918 §13, RFC 6578 §6.2).
//!
//! Only the structure the RFCs define is read: a `multistatus` has
//! `response` elements, each with `href`, then `status` or `propstat`
//! (each with `prop` and `status`), and maybe `error`. Elements are
//! matched by namespace URI (any prefix, or a default namespace, works the
//! same). Unknown properties are ignored,
//! known ones are typed here. A value that does not parse (a colour that
//! is not `#RRGGBB`, an etag with spaces) is treated as absent. A
//! `calendar-data`, `address-data` or `calendar-timezone` over its limit is
//! left out with 507 for that property ([`Response::status_of`]), without
//! failing the body. A `response` whose `<D:href>` does not resolve safely
//! is dropped (counted in [`Multistatus::refused`]) and never used; an
//! href inside a property (a home set, the principal) that does not
//! resolve fails the whole body, as those come only from discovery.

use roxmltree::Node;
use url::Url;

use crate::error_body::Precondition;
use crate::external::ExternalUrl;
use crate::sharing::{
    Acl, Invite, OcInvite, SharingModes, parse_acl, parse_invite, parse_oc_invite,
    parse_sharing_modes,
};
use crate::xml::{child, crlf, element_children, is, parse_document, text_of};
use crate::{
    AddressDataType, CTag, CalendarColor, CalendarUserType, ComponentSet, ETag, Href, Namespace,
    ParseError, Privilege, PrivilegeSet, Property, Report, ReportSet, ResourceType, Status,
    SyncToken, XmlLimits,
};

const DAV: Namespace = Namespace::Dav;

/// Most hrefs kept in a home set or an address set.
const MAX_SET_HREFS: usize = 32;
/// Longest display name, description, content type or address kept.
const MAX_SHORT_TEXT: usize = 4096;
/// Longest `calendar-timezone-id` kept (IANA names are under 40 bytes).
pub const MAX_TIMEZONE_ID: usize = 256;
/// Longest `default-alarm-vevent-*` value kept.
pub const MAX_DEFAULT_ALARMS: usize = 64 * 1024;

/// A parsed multistatus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Multistatus {
    /// One entry per `response`, in order.
    pub responses: Vec<Response>,
    /// Responses dropped because an `href` did not resolve safely
    /// ([`Href::resolve`]): a count for the logs, the hrefs are never kept.
    pub refused: usize,
    /// The new `sync-token` of a sync-collection report.
    pub sync_token: Option<SyncToken>,
}

/// One `response`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    href: Href,
    more_hrefs: Vec<Href>,
    /// The `status` of the whole response (a deleted member of a sync
    /// report has 404; a truncated report has 507 on the collection).
    pub status: Option<Status>,
    /// The values of the properties found (2xx propstats).
    pub properties: Properties,
    /// Properties in a propstat with another status (404: not there; 403:
    /// not allowed; 424…).
    pub failed: Vec<(Property, Status)>,
    /// The condition of an `error` element in the response.
    pub precondition: Option<Precondition>,
}

impl Response {
    /// The href (the first one: several only appear with a shared status).
    #[must_use]
    pub const fn href(&self) -> &Href {
        &self.href
    }

    /// Every href.
    pub fn hrefs(&self) -> impl Iterator<Item = &Href> {
        std::iter::once(&self.href).chain(&self.more_hrefs)
    }

    /// The status the server gave this property, if it mentioned it.
    #[must_use]
    pub fn status_of(&self, property: Property) -> Option<Status> {
        if self.properties.has(property) {
            return Some(Status::OK);
        }
        self.failed
            .iter()
            .find(|(p, _)| *p == property)
            .map(|(_, status)| *status)
    }
}

/// Typed values of the known properties of one response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Properties {
    /// `DAV:resourcetype`.
    pub resource_type: Option<ResourceType>,
    /// `DAV:displayname`.
    pub display_name: Option<String>,
    /// `DAV:getetag`.
    pub etag: Option<ETag>,
    /// `DAV:getcontenttype`.
    pub content_type: Option<String>,
    /// `DAV:sync-token`.
    pub sync_token: Option<SyncToken>,
    /// `CS:getctag`.
    pub ctag: Option<CTag>,
    /// `DAV:current-user-principal` (`None` also for `unauthenticated`).
    pub current_user_principal: Option<Href>,
    /// `C:calendar-home-set`.
    pub calendar_home_set: Vec<Href>,
    /// `CR:addressbook-home-set`.
    pub addressbook_home_set: Vec<Href>,
    /// `C:calendar-user-address-set`: URIs (`mailto:…`), not resolved.
    pub calendar_user_address_set: Vec<String>,
    /// `C:schedule-outbox-URL`.
    pub schedule_outbox_url: Option<Href>,
    /// `C:schedule-inbox-URL`.
    pub schedule_inbox_url: Option<Href>,
    /// `C:calendar-description`.
    pub calendar_description: Option<String>,
    /// `CR:addressbook-description`.
    pub addressbook_description: Option<String>,
    /// `C:calendar-timezone`, with CRLF line ends.
    pub calendar_timezone: Option<String>,
    /// `C:calendar-timezone-id` (RFC 7809): one token, no spaces or
    /// controls, at most [`MAX_TIMEZONE_ID`] bytes (else `None`).
    pub calendar_timezone_id: Option<String>,
    /// `C:default-alarm-vevent-datetime`, with CRLF line ends; `Some("")`
    /// when set to nothing. Longer than [`MAX_DEFAULT_ALARMS`]: `None`
    /// (left out, not an error: it is only a preference).
    pub default_alarm_datetime: Option<String>,
    /// `C:default-alarm-vevent-date`, as `default_alarm_datetime`.
    pub default_alarm_date: Option<String>,
    /// `C:supported-calendar-component-set`.
    pub supported_components: Option<ComponentSet>,
    /// `DAV:current-user-privilege-set`.
    pub privileges: Option<PrivilegeSet>,
    /// `DAV:supported-report-set`.
    pub supported_reports: Option<ReportSet>,
    /// `CR:supported-address-data`.
    pub supported_address_data: Option<Vec<AddressDataType>>,
    /// `C:calendar-data`, with CRLF line ends.
    pub calendar_data: Option<String>,
    /// `CR:address-data`, with CRLF line ends.
    pub address_data: Option<String>,
    /// `I:calendar-color`.
    pub calendar_color: Option<CalendarColor>,
    /// `I:calendar-order`.
    pub calendar_order: Option<i64>,
    /// `DAV:principal-collection-set`.
    pub principal_collection_set: Vec<Href>,
    /// `C:calendar-user-type`.
    pub calendar_user_type: Option<CalendarUserType>,
    /// `DAV:owner` (`None` also when its href does not resolve).
    pub owner: Option<Href>,
    /// `DAV:acl` (`None` also when it does not follow RFC 3744 or is
    /// over the limits).
    pub acl: Option<Acl>,
    /// `CS:invite`.
    pub invite: Option<Invite>,
    /// `CS:allowed-sharing-modes`.
    pub sharing_modes: Option<SharingModes>,
    /// `O:invite` (Nextcloud).
    pub oc_invite: Option<OcInvite>,
    /// `CS:source`: where a subscription comes from (`None` also when it
    /// is not an [`ExternalUrl`]).
    pub source: Option<ExternalUrl>,
    /// `CS:publish-url`: the public link of a published calendar.
    pub publish_url: Option<ExternalUrl>,
    /// `NC:deleted-at`: when it went to the trash bin (RFC 3339 text, at
    /// most 64 bytes, not checked further).
    pub deleted_at: Option<String>,
    /// `NC:calendar-uri`: the calendar a deleted event was in (one path
    /// segment, at most 256 bytes, no `/`).
    pub calendar_uri: Option<String>,
    /// `NC:trash-bin-retention-duration`, in seconds.
    pub trash_bin_retention: Option<u64>,
    present: Vec<Property>,
}

impl Properties {
    /// The server returned this property with a 2xx status (even if its
    /// value was empty or did not parse).
    #[must_use]
    pub fn has(&self, property: Property) -> bool {
        self.present.contains(&property)
    }
}

/// Parses a `207 Multi-Status` body. Hrefs are resolved against `base`,
/// the URL the request finally went to (after redirects).
///
/// # Errors
///
/// [`ParseError`]: the limits of `limits`, malformed or non-UTF-8 XML, a
/// DTD, a root that is not `DAV:multistatus`, a response without href, or
/// an href inside a property that [`Href::resolve`] refuses. A response
/// whose own href is refused is only dropped ([`Multistatus::refused`]).
pub fn parse_multistatus(
    body: &[u8],
    base: &Url,
    limits: &XmlLimits,
) -> Result<Multistatus, ParseError> {
    let base = Href::from_url(base)?;
    let document = parse_document(body, limits)?;
    let root = document.root_element();
    if !is(root, DAV, "multistatus") {
        return Err(ParseError::Invalid("root is not DAV:multistatus"));
    }
    let mut responses = Vec::new();
    let mut sync_token = None;
    let mut refused = 0_usize;
    for node in element_children(root) {
        if is(node, DAV, "response") {
            match parse_response(node, &base, limits)? {
                Some(response) => responses.push(response),
                None => refused += 1,
            }
        } else if is(node, DAV, "sync-token") {
            sync_token = text_of(node, SyncToken::MAX_LEN + 64)?
                .as_deref()
                .and_then(SyncToken::parse);
        }
    }
    Ok(Multistatus {
        responses,
        refused,
        sync_token,
    })
}

fn parse_status(node: Node<'_, '_>) -> Result<Status, ParseError> {
    text_of(node, 256)?
        .as_deref()
        .and_then(Status::parse_line)
        .ok_or(ParseError::Invalid("bad status line"))
}

fn parse_href(node: Node<'_, '_>, base: &Href) -> Result<Href, ParseError> {
    let text =
        text_of(node, Href::MAX_LEN + 64)?.ok_or(ParseError::Invalid("href with elements"))?;
    Ok(Href::resolve(&text, base)?)
}

/// One `response`; `None` if one of its `<D:href>` does not resolve safely
/// (too long, a dot segment, an encoded slash, another scheme…): that
/// response is dropped, the rest of the body is still read.
fn parse_response(
    node: Node<'_, '_>,
    base: &Href,
    limits: &XmlLimits,
) -> Result<Option<Response>, ParseError> {
    let mut hrefs = Vec::new();
    for item in element_children(node).filter(|item| is(*item, DAV, "href")) {
        match parse_href(item, base) {
            Ok(href) => hrefs.push(href),
            Err(ParseError::Href(_) | ParseError::ValueTooLarge) => return Ok(None),
            Err(other) => return Err(other),
        }
    }
    let mut status = None;
    let mut properties = Properties::default();
    let mut failed = Vec::new();
    let mut precondition = None;
    let mut propstats = 0_usize;
    for item in element_children(node) {
        if is(item, DAV, "status") {
            status = Some(parse_status(item)?);
        } else if is(item, DAV, "propstat") {
            propstats += 1;
            parse_propstat(item, base, limits, &mut properties, &mut failed)?;
        } else if is(item, DAV, "error") {
            precondition = Precondition::from_error_element(item);
        }
    }
    let mut hrefs = hrefs.into_iter();
    let href = hrefs
        .next()
        .ok_or(ParseError::Invalid("response without href"))?;
    let more_hrefs: Vec<Href> = hrefs.collect();
    if !more_hrefs.is_empty() && propstats > 0 {
        return Err(ParseError::Invalid("several hrefs with propstat"));
    }
    Ok(Some(Response {
        href,
        more_hrefs,
        status,
        properties,
        failed,
        precondition,
    }))
}

fn parse_propstat(
    node: Node<'_, '_>,
    base: &Href,
    limits: &XmlLimits,
    properties: &mut Properties,
    failed: &mut Vec<(Property, Status)>,
) -> Result<(), ParseError> {
    let status = child(node, DAV, "status")
        .ok_or(ParseError::Invalid("propstat without status"))
        .and_then(parse_status)?;
    let Some(prop) = child(node, DAV, "prop") else {
        return Ok(());
    };
    for value in element_children(prop) {
        let name = value.tag_name();
        let known = if status.is_success() {
            Property::from_name(name.namespace(), name.name())
        } else {
            // Statuses only: the Cyrus quirk (see `Property::from_local_name`).
            Property::from_name(name.namespace(), name.name())
                .or_else(|| Property::from_local_name(name.name()))
        };
        let Some(property) = known else {
            continue;
        };
        if !status.is_success() {
            failed.push((property, status));
        } else if !properties.has(property) {
            properties.present.push(property);
            match read_value(property, value, base, limits, properties) {
                // One resource's value over its limit: 507 for it, the
                // rest of the body is still read.
                Err(ParseError::ValueTooLarge)
                    if matches!(
                        property,
                        Property::CalendarData | Property::AddressData | Property::CalendarTimezone
                    ) =>
                {
                    properties.present.retain(|known| *known != property);
                    failed.push((property, Status::INSUFFICIENT_STORAGE));
                }
                other => other?,
            }
        }
    }
    Ok(())
}

/// A short text value, trimmed; `None` if too long or with control
/// characters (it is shown to the user or compared, never needed whole).
fn short_text(node: Node<'_, '_>) -> Option<String> {
    let text = text_of(node, MAX_SHORT_TEXT).ok().flatten()?;
    let text = text.trim();
    (!text.chars().any(char::is_control)).then(|| text.to_owned())
}

fn hrefs_in(node: Node<'_, '_>, base: &Href) -> Result<Vec<Href>, ParseError> {
    element_children(node)
        .filter(|c| is(*c, DAV, "href"))
        .take(MAX_SET_HREFS)
        .map(|c| parse_href(c, base))
        .collect()
}

fn read_value(
    property: Property,
    node: Node<'_, '_>,
    base: &Href,
    limits: &XmlLimits,
    out: &mut Properties,
) -> Result<(), ParseError> {
    match property {
        Property::ResourceType => out.resource_type = Some(resource_type(node)),
        Property::DisplayName => out.display_name = short_text(node),
        Property::GetETag => {
            out.etag = text_of(node, ETag::MAX_LEN + 64)
                .ok()
                .flatten()
                .as_deref()
                .and_then(ETag::parse);
        }
        Property::GetContentType => out.content_type = short_text(node),
        Property::SyncToken => {
            out.sync_token = text_of(node, SyncToken::MAX_LEN + 64)
                .ok()
                .flatten()
                .as_deref()
                .and_then(SyncToken::parse);
        }
        Property::GetCTag => {
            out.ctag = text_of(node, CTag::MAX_LEN + 64)
                .ok()
                .flatten()
                .as_deref()
                .and_then(CTag::parse);
        }
        Property::CurrentUserPrincipal => {
            out.current_user_principal = match child(node, DAV, "href") {
                Some(href) => Some(parse_href(href, base)?),
                None => None,
            };
        }
        Property::CalendarHomeSet => out.calendar_home_set = hrefs_in(node, base)?,
        Property::AddressbookHomeSet => out.addressbook_home_set = hrefs_in(node, base)?,
        Property::CalendarUserAddressSet => {
            out.calendar_user_address_set = element_children(node)
                .filter(|c| is(*c, DAV, "href"))
                .take(MAX_SET_HREFS)
                .filter_map(short_text)
                .filter(|address| !address.is_empty() && !address.contains(char::is_whitespace))
                .collect();
        }
        Property::ScheduleOutboxUrl | Property::ScheduleInboxUrl => {
            let href = match child(node, DAV, "href") {
                Some(href) => Some(parse_href(href, base)?),
                None => None,
            };
            if property == Property::ScheduleOutboxUrl {
                out.schedule_outbox_url = href;
            } else {
                out.schedule_inbox_url = href;
            }
        }
        Property::CalendarDescription => out.calendar_description = short_text(node),
        Property::AddressbookDescription => out.addressbook_description = short_text(node),
        Property::CalendarTimezone => {
            out.calendar_timezone = text_of(node, limits.max_value_bytes)?.map(|t| crlf(&t));
        }
        Property::CalendarTimezoneId
        | Property::DefaultAlarmVeventDatetime
        | Property::DefaultAlarmVeventDate => read_calendar_default(property, node, out),
        Property::CalendarData => {
            out.calendar_data = text_of(node, limits.max_value_bytes)?.map(|t| crlf(&t));
        }
        Property::AddressData => {
            out.address_data = text_of(node, limits.max_value_bytes)?.map(|t| crlf(&t));
        }
        Property::SupportedCalendarComponentSet
        | Property::CurrentUserPrivilegeSet
        | Property::SupportedReportSet
        | Property::SupportedAddressData => read_set(property, node, out),
        Property::CalendarColor => {
            out.calendar_color = short_text(node).as_deref().and_then(CalendarColor::parse);
        }
        Property::CalendarOrder => {
            out.calendar_order = short_text(node).and_then(|t| t.parse().ok());
        }
        Property::PrincipalCollectionSet => out.principal_collection_set = hrefs_in(node, base)?,
        Property::CalendarUserType => {
            out.calendar_user_type = short_text(node).as_deref().map(CalendarUserType::parse);
        }
        Property::Owner => {
            out.owner = child(node, DAV, "href").and_then(|href| parse_href(href, base).ok());
        }
        Property::Acl => out.acl = parse_acl(node, base),
        Property::Invite => out.invite = Some(parse_invite(node, base)),
        Property::AllowedSharingModes => out.sharing_modes = Some(parse_sharing_modes(node)),
        Property::OcInvite => out.oc_invite = Some(parse_oc_invite(node)),
        // Only searched by, never asked for.
        Property::EmailAddress => {}
        Property::Source
        | Property::PublishUrl
        | Property::DeletedAt
        | Property::CalendarUri
        | Property::TrashBinRetentionDuration => read_nextcloud(property, node, out),
    }
    Ok(())
}

/// Nextcloud's subscriptions, publishing and trash bin: shown, never
/// needed, so a value we don't keep is left out.
fn read_nextcloud(property: Property, node: Node<'_, '_>, out: &mut Properties) {
    match property {
        Property::Source | Property::PublishUrl => {
            let url = child(node, DAV, "href")
                .and_then(short_text)
                .as_deref()
                .and_then(ExternalUrl::parse);
            if property == Property::Source {
                out.source = url;
            } else {
                out.publish_url = url;
            }
        }
        Property::DeletedAt => {
            out.deleted_at = short_text(node).filter(|t| !t.is_empty() && t.len() <= 64);
        }
        Property::CalendarUri => {
            out.calendar_uri =
                short_text(node).filter(|t| !t.is_empty() && t.len() <= 256 && !t.contains('/'));
        }
        Property::TrashBinRetentionDuration => {
            out.trash_bin_retention = short_text(node).and_then(|t| t.parse().ok());
        }
        _ => {}
    }
}

/// A calendar's zone id and default alarms: preferences, so a value we
/// don't keep is left out, never an error.
fn read_calendar_default(property: Property, node: Node<'_, '_>, out: &mut Properties) {
    match property {
        Property::CalendarTimezoneId => {
            out.calendar_timezone_id = short_text(node).filter(|id| {
                !id.is_empty()
                    && id.len() <= MAX_TIMEZONE_ID
                    && !id.chars().any(|c| c.is_whitespace() || c.is_control())
            });
        }
        Property::DefaultAlarmVeventDatetime | Property::DefaultAlarmVeventDate => {
            let value = text_of(node, MAX_DEFAULT_ALARMS)
                .ok()
                .flatten()
                .map(|t| crlf(t.trim()));
            if property == Property::DefaultAlarmVeventDatetime {
                out.default_alarm_datetime = value;
            } else {
                out.default_alarm_date = value;
            }
        }
        _ => {}
    }
}

/// The properties whose value is a set of elements.
fn read_set(property: Property, node: Node<'_, '_>, out: &mut Properties) {
    match property {
        Property::SupportedCalendarComponentSet => {
            let mut set = ComponentSet::default();
            for comp in element_children(node).filter(|c| is(*c, Namespace::CalDav, "comp")) {
                if let Some(name) = comp.attribute("name") {
                    set.insert_name(name);
                }
            }
            out.supported_components = Some(set);
        }
        Property::CurrentUserPrivilegeSet => {
            let mut set = PrivilegeSet::default();
            for privilege in element_children(node).filter(|c| is(*c, DAV, "privilege")) {
                for name in element_children(privilege) {
                    let Some(namespace) = name.tag_name().namespace().and_then(Namespace::from_uri)
                    else {
                        continue;
                    };
                    if let Some(p) = Privilege::from_name(namespace, name.tag_name().name()) {
                        set.insert(p);
                    }
                }
            }
            out.privileges = Some(set);
        }
        Property::SupportedReportSet => {
            let mut set = ReportSet::default();
            let reports = element_children(node)
                .filter(|c| is(*c, DAV, "supported-report"))
                .filter_map(|c| child(c, DAV, "report"));
            for report in reports {
                for name in element_children(report) {
                    let Some(namespace) = name.tag_name().namespace().and_then(Namespace::from_uri)
                    else {
                        continue;
                    };
                    if let Some(r) = Report::from_name(namespace, name.tag_name().name()) {
                        set.insert(r);
                    }
                }
            }
            out.supported_reports = Some(set);
        }
        Property::SupportedAddressData => {
            let types = element_children(node)
                .filter(|c| is(*c, Namespace::CardDav, "address-data-type"))
                .take(16)
                .map(|c| AddressDataType {
                    content_type: c
                        .attribute("content-type")
                        .unwrap_or("text/vcard")
                        .chars()
                        .take(128)
                        .collect(),
                    version: c
                        .attribute("version")
                        .unwrap_or("3.0")
                        .chars()
                        .take(16)
                        .collect(),
                })
                .collect();
            out.supported_address_data = Some(types);
        }
        _ => {}
    }
}

fn resource_type(node: Node<'_, '_>) -> ResourceType {
    let mut kind = ResourceType::default();
    for c in element_children(node) {
        let Some(namespace) = c.tag_name().namespace().and_then(Namespace::from_uri) else {
            continue;
        };
        let flag = match (namespace, c.tag_name().name()) {
            (Namespace::Dav, "collection") => &mut kind.collection,
            (Namespace::Dav, "principal") => &mut kind.principal,
            (Namespace::CalDav, "calendar") => &mut kind.calendar,
            (Namespace::CardDav, "addressbook") => &mut kind.addressbook,
            (Namespace::CalDav, "schedule-inbox") => &mut kind.schedule_inbox,
            (Namespace::CalDav, "schedule-outbox") => &mut kind.schedule_outbox,
            (Namespace::CalendarServer, "subscribed") => &mut kind.subscribed,
            (Namespace::CalendarServer, "shared") => &mut kind.shared,
            (Namespace::CalendarServer, "shared-owner") => &mut kind.shared_owner,
            (Namespace::Nextcloud, "deleted-calendar") => &mut kind.deleted_calendar,
            _ => continue,
        };
        *flag = true;
    }
    kind
}

#[cfg(test)]
mod tests;
