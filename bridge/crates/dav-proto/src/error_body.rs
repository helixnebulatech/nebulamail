// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `DAV:error` bodies: which precondition failed (RFC 4918 §16).

use roxmltree::Node;

use crate::xml::{element_children, is, parse_document};
use crate::{Namespace, XmlLimits};

/// A failed precondition or postcondition named by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Precondition {
    /// `DAV:valid-sync-token`: the sync token expired or is unknown
    /// (RFC 6578 §3.2): sync again from scratch.
    ValidSyncToken,
    /// `DAV:number-of-matches-within-limits`.
    NumberOfMatchesWithinLimits,
    /// `DAV:need-privileges`.
    NeedPrivileges,
    /// `DAV:resource-must-be-null`: something already exists there.
    ResourceMustBeNull,
    /// `C:no-uid-conflict` / `CR:no-uid-conflict`: another resource has
    /// this UID.
    NoUidConflict,
    /// `C:supported-calendar-data` / `CR:supported-address-data`.
    SupportedData,
    /// `C:valid-calendar-data` / `CR:valid-address-data`.
    ValidData,
    /// `C:valid-calendar-object-resource`.
    ValidCalendarObjectResource,
    /// `C:supported-calendar-component`: the calendar does not take this
    /// kind of component.
    SupportedCalendarComponent,
    /// `C:calendar-collection-location-ok`.
    CalendarCollectionLocationOk,
    /// `C:max-resource-size` / `CR:max-resource-size`.
    MaxResourceSize,
    /// Something else (not kept: the name is server data).
    Other,
}

impl Precondition {
    pub(crate) fn from_element(node: Node<'_, '_>) -> Self {
        let local = node.tag_name().name();
        let Some(namespace) = node.tag_name().namespace().and_then(Namespace::from_uri) else {
            return Self::Other;
        };
        match (namespace, local) {
            (Namespace::Dav, "valid-sync-token") => Self::ValidSyncToken,
            (Namespace::Dav, "number-of-matches-within-limits") => {
                Self::NumberOfMatchesWithinLimits
            }
            (Namespace::Dav, "need-privileges") => Self::NeedPrivileges,
            (Namespace::Dav, "resource-must-be-null") => Self::ResourceMustBeNull,
            (Namespace::CalDav | Namespace::CardDav, "no-uid-conflict") => Self::NoUidConflict,
            (Namespace::CalDav, "supported-calendar-data")
            | (Namespace::CardDav, "supported-address-data") => Self::SupportedData,
            (Namespace::CalDav, "valid-calendar-data")
            | (Namespace::CardDav, "valid-address-data") => Self::ValidData,
            (Namespace::CalDav, "valid-calendar-object-resource") => {
                Self::ValidCalendarObjectResource
            }
            (Namespace::CalDav, "supported-calendar-component") => Self::SupportedCalendarComponent,
            (Namespace::CalDav, "calendar-collection-location-ok") => {
                Self::CalendarCollectionLocationOk
            }
            (Namespace::CalDav | Namespace::CardDav, "max-resource-size") => Self::MaxResourceSize,
            _ => Self::Other,
        }
    }

    /// The first known condition inside a `DAV:error` element.
    pub(crate) fn from_error_element(error: Node<'_, '_>) -> Option<Self> {
        let mut found = None;
        for condition in element_children(error) {
            match Self::from_element(condition) {
                Self::Other => found = found.or(Some(Self::Other)),
                known => return Some(known),
            }
        }
        found
    }
}

/// The precondition of an error body (`<D:error>…</D:error>`), if the body
/// is a small, valid `DAV:error` document. Anything else (HTML error
/// pages, text, a DTD) is `None`.
///
/// ```
/// use nebula_dav_proto::{parse_error_body, Precondition};
///
/// let body = br#"<?xml version="1.0"?><error xmlns="DAV:"><valid-sync-token/></error>"#;
/// assert_eq!(parse_error_body(body), Some(Precondition::ValidSyncToken));
/// assert_eq!(parse_error_body(b"<html>Forbidden</html>"), None);
/// ```
#[must_use]
pub fn parse_error_body(body: &[u8]) -> Option<Precondition> {
    let document = parse_document(body, &XmlLimits::small()).ok()?;
    let root = document.root_element();
    if !is(root, Namespace::Dav, "error") {
        return None;
    }
    Precondition::from_error_element(root)
}

#[cfg(test)]
mod tests {
    use super::{Precondition, parse_error_body};

    #[test]
    fn conditions() {
        let cases = [
            (
                r#"<d:error xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><c:no-uid-conflict><d:href>/x</d:href></c:no-uid-conflict></d:error>"#,
                Some(Precondition::NoUidConflict),
            ),
            (
                r#"<error xmlns="DAV:"><x xmlns="urn:other"/><number-of-matches-within-limits/></error>"#,
                Some(Precondition::NumberOfMatchesWithinLimits),
            ),
            (
                r#"<error xmlns="DAV:"><x xmlns="urn:other"/></error>"#,
                Some(Precondition::Other),
            ),
            (r#"<error xmlns="DAV:"/>"#, None),
            (r"<error><valid-sync-token/></error>", None),
            (
                r#"<!DOCTYPE x><error xmlns="DAV:"><valid-sync-token/></error>"#,
                None,
            ),
        ];
        for (body, want) in cases {
            assert_eq!(parse_error_body(body.as_bytes()), want, "{body}");
        }
        let huge = format!(
            r#"<error xmlns="DAV:"><valid-sync-token/>{}</error>"#,
            " ".repeat(70_000)
        );
        assert_eq!(parse_error_body(huge.as_bytes()), None);
    }
}
