// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! WebDAV, CalDAV and CardDAV without I/O.
//!
//! The request shapes and server quirks follow `libdav` 0.11 (pimsync,
//! ISC), rewritten here with stricter parsing: everything a server sends is
//! hostile.
//!
//! - [`Href`]: a server path, percent-decoded per segment, never with dot
//!   segments, encoded slashes, queries or control characters, resolved
//!   against the request URL. [`Href::is_member_of`] says whether a
//!   resource is inside its collection (same origin too).
//! - [`ETag`], [`SyncToken`], [`CTag`]: opaque values, checked so they can
//!   go back in a header or in XML without injection.
//! - [`DavCapabilities`]: the `DAV:` header (`calendar-access`,
//!   `addressbook`, `calendar-auto-schedule`…).
//! - [`request`]: XML request bodies (PROPFIND, PROPPATCH, REPORT
//!   sync-collection and multiget, MKCALENDAR, extended MKCOL) written with
//!   our own escaping.
//! - [`parse_multistatus`]: a strict `multistatus` parser on roxmltree:
//!   size limit before parsing, UTF-8 only, any DTD is an error (no XXE,
//!   no billion laughs), node and depth limits, namespaces compared by URI
//!   (never by prefix). The same parser reads the other answers:
//!   [`parse_schedule_response`] (a free-busy POST on the scheduling
//!   outbox, RFC 6638) and [`parse_principal_search_property_set`]
//!   (RFC 3744 §9.5).
//! - Sharing: `DAV:owner`, `DAV:acl` ([`Acl`], RFC 3744) and the
//!   Calendar Server `CS:invite` ([`Invite`]) and Nextcloud's `O:invite`
//!   ([`OcInvite`]), with the bodies of the `ACL`
//!   method and the `CS:share` POST ([`request::acl`], [`request::share`]).
//!
//! ```
//! use url::Url;
//! use nebula_dav_proto::{parse_multistatus, Property, XmlLimits, request};
//!
//! let body = request::propfind(&[Property::DisplayName, Property::GetETag]);
//! assert!(body.contains("<D:displayname/>"));
//!
//! let base = Url::parse("https://dav.example.com/cal/alice/").unwrap();
//! let xml = br#"<?xml version="1.0" encoding="utf-8"?>
//! <d:multistatus xmlns:d="DAV:">
//!   <d:response>
//!     <d:href>/cal/alice/work/</d:href>
//!     <d:propstat>
//!       <d:prop><d:displayname>Work</d:displayname></d:prop>
//!       <d:status>HTTP/1.1 200 OK</d:status>
//!     </d:propstat>
//!   </d:response>
//! </d:multistatus>"#;
//! let multistatus = parse_multistatus(xml, &base, &XmlLimits::default()).unwrap();
//! let response = &multistatus.responses[0];
//! assert_eq!(response.href().path(), "/cal/alice/work/");
//! assert_eq!(response.properties.display_name.as_deref(), Some("Work"));
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod capabilities;
mod error;
mod error_body;
mod external;
mod href;
mod multistatus;
mod names;
mod percent;
mod principal;
pub mod request;
mod schedule;
mod sharing;
mod status;
mod values;
mod xml;

pub use capabilities::DavCapabilities;
pub use error::{EncodeError, HrefError, ParseError};
pub use error_body::{Precondition, parse_error_body};
pub use external::ExternalUrl;
pub use href::{Href, LocationKey, ResourceName};
pub use multistatus::{
    MAX_DEFAULT_ALARMS, MAX_TIMEZONE_ID, Multistatus, Properties, Response, parse_multistatus,
};
pub use names::{Namespace, Property};
pub use principal::{CalendarUserType, parse_principal_search_property_set};
pub use schedule::{
    CalendarAddress, FreeBusyRequest, MAX_FREEBUSY_ATTENDEES, MAX_SCHEDULE_RESPONSES,
    RequestStatus, ScheduleResponse, parse_schedule_response,
};
pub use sharing::{
    Ace, AcePrincipal, Acl, Invite, InviteStatus, InviteUser, MAX_ACES, MAX_OC_SHARE_REQUEST,
    MAX_PRIVILEGES, MAX_SHARE_REQUEST, MAX_SHAREES, OcInvite, OcPrincipal, OcShare, QualifiedName,
    ShareAccess, Sharee, SharingModes,
};
pub use status::Status;
pub use values::{
    AddressDataType, CTag, CalendarColor, Component, ComponentSet, ETag, Privilege, PrivilegeSet,
    Report, ReportSet, ResourceType, SyncToken, VcardRequest,
};
pub use xml::{MAX_ATTRIBUTES_PER_ELEMENT, MAX_NAMESPACES_IN_SCOPE, XmlLimits};

/// The `Depth` header of PROPFIND and REPORT. `infinity` is never sent:
/// servers may refuse it and it can be very expensive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Depth {
    /// The resource itself.
    Zero,
    /// The resource and its direct members.
    One,
}

impl Depth {
    /// The header value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "0",
            Self::One => "1",
        }
    }
}

#[cfg(test)]
mod proptests;
