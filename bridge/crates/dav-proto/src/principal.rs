// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Principals (RFC 3744 §§5, 9.4–9.5; RFC 6638 §2.4.2): what kind of
//! calendar user a principal is, and which properties a server lets
//! clients search (`principal-search-property-set`).

use crate::xml::{child, element_children, is, parse_document};
use crate::{Namespace, ParseError, Property, XmlLimits};

/// Most searchable properties kept from one answer.
const MAX_SEARCH_PROPERTIES: usize = 64;

/// `C:calendar-user-type` (RFC 6638 §2.4.2, the `CUTYPE` values of
/// RFC 5545 §3.2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CalendarUserType {
    /// `INDIVIDUAL` (the default when a server does not say).
    Individual,
    /// `GROUP`.
    Group,
    /// `RESOURCE`.
    Resource,
    /// `ROOM`.
    Room,
    /// `UNKNOWN`, or an extension value.
    Unknown,
}

impl CalendarUserType {
    /// The value of the property (any case, surrounding spaces ignored).
    /// Anything that is not a known value is [`Self::Unknown`].
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if text.eq_ignore_ascii_case("INDIVIDUAL") {
            Self::Individual
        } else if text.eq_ignore_ascii_case("GROUP") {
            Self::Group
        } else if text.eq_ignore_ascii_case("RESOURCE") {
            Self::Resource
        } else if text.eq_ignore_ascii_case("ROOM") {
            Self::Room
        } else {
            Self::Unknown
        }
    }
}

/// Parses the answer to `REPORT principal-search-property-set` (RFC 3744
/// §9.5): the properties this crate knows that the server lets clients
/// search with `principal-property-search`, in order, without
/// duplicates. Descriptions are server text and are not kept.
///
/// # Errors
///
/// [`ParseError`] for the XML limits, malformed XML, a DTD, or a root
/// that is not `DAV:principal-search-property-set` (Radicale 3.8 answers
/// an empty `multistatus`: for a caller, the same as "nothing").
pub fn parse_principal_search_property_set(
    body: &[u8],
    limits: &XmlLimits,
) -> Result<Vec<Property>, ParseError> {
    let document = parse_document(body, limits)?;
    let root = document.root_element();
    if !is(root, Namespace::Dav, "principal-search-property-set") {
        return Err(ParseError::Invalid(
            "root is not DAV:principal-search-property-set",
        ));
    }
    let mut found = Vec::new();
    let props = element_children(root)
        .filter(|node| is(*node, Namespace::Dav, "principal-search-property"))
        .filter_map(|node| child(node, Namespace::Dav, "prop"));
    for prop in props {
        for name in element_children(prop) {
            let tag = name.tag_name();
            if let Some(property) = Property::from_name(tag.namespace(), tag.name())
                && !found.contains(&property)
                && found.len() < MAX_SEARCH_PROPERTIES
            {
                found.push(property);
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_user_types() {
        assert_eq!(
            CalendarUserType::parse("INDIVIDUAL"),
            CalendarUserType::Individual
        );
        assert_eq!(CalendarUserType::parse(" room "), CalendarUserType::Room);
        assert_eq!(CalendarUserType::parse("Group"), CalendarUserType::Group);
        assert_eq!(
            CalendarUserType::parse("RESOURCE"),
            CalendarUserType::Resource
        );
        assert_eq!(
            CalendarUserType::parse("X-ROBOT"),
            CalendarUserType::Unknown
        );
        assert_eq!(CalendarUserType::parse(""), CalendarUserType::Unknown);
    }

    /// Stalwart 0.16.27 searches the display name only.
    #[test]
    fn search_property_set_of_stalwart() {
        let body = br#"<?xml version="1.0" encoding="UTF-8"?><D:principal-search-property-set xmlns:D="DAV:"><D:principal-search-property><D:prop><D:displayname/></D:prop><D:description>Account or Group name</D:description></D:principal-search-property></D:principal-search-property-set>"#;
        assert_eq!(
            parse_principal_search_property_set(body, &XmlLimits::small()),
            Ok(vec![Property::DisplayName])
        );
    }

    #[test]
    fn search_property_set_with_addresses_and_unknown_names() {
        let body = br#"<principal-search-property-set xmlns="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:x="urn:x">
            <principal-search-property><prop><displayname/></prop></principal-search-property>
            <principal-search-property><prop><c:calendar-user-address-set/><x:nickname/></prop></principal-search-property>
            <principal-search-property><prop><displayname/></prop></principal-search-property>
            <other><prop><c:calendar-user-type/></prop></other>
        </principal-search-property-set>"#;
        assert_eq!(
            parse_principal_search_property_set(body, &XmlLimits::small()),
            Ok(vec![
                Property::DisplayName,
                Property::CalendarUserAddressSet
            ])
        );
    }

    /// Radicale 3.8 answers with an empty multistatus.
    #[test]
    fn other_roots_and_dtds_are_errors() {
        assert!(matches!(
            parse_principal_search_property_set(
                br#"<multistatus xmlns="DAV:" />"#,
                &XmlLimits::small()
            ),
            Err(ParseError::Invalid(_))
        ));
        assert_eq!(
            parse_principal_search_property_set(
                br#"<!DOCTYPE x [<!ENTITY a "b">]><principal-search-property-set xmlns="DAV:"/>"#,
                &XmlLimits::small()
            ),
            Err(ParseError::Dtd)
        );
    }
}
