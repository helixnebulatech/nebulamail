// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Other principals of the server (RFC 3744 §§5.8, 9.4, 9.5; RFC 6638
//! §2.4): who can be invited or asked for free-busy. Only under a
//! principal collection that discovery found (allowed origin), and every
//! principal found must be inside it: a hostile server cannot point us at
//! another host or another place.

use std::collections::HashSet;

use nebula_dav_proto::request::{self, XML_CONTENT_TYPE};
use nebula_dav_proto::{
    CalendarUserType, Depth, Href, Multistatus, Property, parse_principal_search_property_set,
};

use crate::core::client::Call;
use crate::op::report_method;
use crate::{DavClient, DavError};

/// Most principals kept from one listing or search.
pub const MAX_PRINCIPALS: usize = 10_000;

/// What we ask about each principal.
const PRINCIPAL_PROPERTIES: [Property; 4] = [
    Property::ResourceType,
    Property::DisplayName,
    Property::CalendarUserAddressSet,
    Property::CalendarUserType,
];

/// A principal on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DavPrincipal {
    /// Where it is (inside a principal collection).
    pub href: Href,
    /// `displayname`.
    pub display_name: Option<String>,
    /// `calendar-user-address-set`, as the server wrote it (`mailto:…`,
    /// sometimes paths).
    pub addresses: Vec<String>,
    /// `calendar-user-type`.
    pub kind: Option<CalendarUserType>,
}

/// Where the principals of an answer must be.
#[derive(Clone, Copy)]
enum Placement {
    /// A direct member (a Depth 1 listing).
    Member,
    /// Anywhere below (a search: `SabreDAV` keeps users in
    /// `principals/users/` under `principals/`).
    Inside,
}

impl DavClient {
    /// The properties `collection` lets clients search
    /// (`REPORT principal-search-property-set`), among those this crate
    /// knows.
    ///
    /// # Errors
    ///
    /// [`DavError`] for the request; [`DavError::Xml`] for an answer that
    /// is not a `principal-search-property-set` (Radicale 3.8 answers an
    /// empty multistatus).
    pub async fn principal_search_properties(
        &self,
        collection: &Href,
    ) -> Result<Vec<Property>, DavError> {
        let collection = collection.as_collection();
        self.check_origin(&collection)?;
        let limits = self.server().limits;
        let call = Call::new(report_method(), &collection, limits.max_error_body)
            .depth(Depth::Zero)
            .body(request::principal_search_property_set(), XML_CONTENT_TYPE);
        let reply = self.send(&call).await?.success()?;
        reply.expect_xml()?;
        Ok(parse_principal_search_property_set(
            &reply.body,
            &self.xml_limits(limits.max_error_body),
        )?)
    }

    /// `REPORT principal-property-search` (RFC 3744 §9.4) on `collection`:
    /// principals whose `property` contains `text`, as the server matches
    /// it (callers check the results themselves).
    ///
    /// # Errors
    ///
    /// [`DavError`] for the request or an unusable `text`.
    pub async fn search_principals(
        &self,
        collection: &Href,
        property: Property,
        text: &str,
    ) -> Result<Vec<DavPrincipal>, DavError> {
        let collection = collection.as_collection();
        self.check_origin(&collection)?;
        let limits = self.server().limits;
        let body = request::principal_property_search(property, text, &PRINCIPAL_PROPERTIES)?;
        let call = Call::new(report_method(), &collection, limits.max_multistatus)
            .depth(Depth::Zero)
            .body(body, XML_CONTENT_TYPE);
        let (multistatus, _final_url) = self.multistatus(&call, limits.max_error_body).await?;
        Ok(principals_in(multistatus, &collection, Placement::Inside))
    }

    /// The principals directly inside `collection` (PROPFIND Depth 1).
    ///
    /// # Errors
    ///
    /// [`DavError`] for the request (servers that do not list their
    /// principals answer 403 or 405).
    pub async fn list_principals(&self, collection: &Href) -> Result<Vec<DavPrincipal>, DavError> {
        let collection = collection.as_collection();
        let (multistatus, _final_url) = self
            .propfind(&collection, Depth::One, &PRINCIPAL_PROPERTIES)
            .await?;
        Ok(principals_in(multistatus, &collection, Placement::Member))
    }
}

fn principals_in(
    multistatus: Multistatus,
    collection: &Href,
    placement: Placement,
) -> Vec<DavPrincipal> {
    if multistatus.refused > 0 {
        tracing::warn!(
            refused = multistatus.refused,
            "dav: principal listing had hrefs that do not resolve safely"
        );
    }
    let mut found: Vec<DavPrincipal> = Vec::new();
    // Every href is a collection here, so equal hrefs are the same
    // location (`Href` compares the canonical URL).
    let mut seen: HashSet<Href> = HashSet::new();
    let mut outside = 0_usize;
    for response in multistatus.responses {
        let href = response.href().as_collection();
        if href.same_location(collection)
            || response.status.is_some_and(|status| !status.is_success())
        {
            continue;
        }
        let placed = match placement {
            Placement::Member => href.is_member_of(collection),
            Placement::Inside => href.is_inside(collection),
        };
        if !placed {
            outside += 1;
            continue;
        }
        let p = response.properties;
        // Not a principal (a calendar home listed next to them…). Servers
        // that do not send `resourcetype` in a search are taken at their
        // word.
        if p.resource_type.is_some_and(|kind| !kind.principal) {
            continue;
        }
        if seen.contains(&href) {
            continue;
        }
        if found.len() >= MAX_PRINCIPALS {
            tracing::warn!(
                limit = MAX_PRINCIPALS,
                "dav: more principals than the limit, the rest ignored"
            );
            break;
        }
        seen.insert(href.clone());
        found.push(DavPrincipal {
            href,
            display_name: p.display_name,
            addresses: p.calendar_user_address_set,
            kind: p.calendar_user_type,
        });
    }
    if outside > 0 {
        tracing::warn!(
            outside,
            "dav: principals outside their collection were ignored"
        );
    }
    found
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use nebula_dav_proto::{Href, XmlLimits, parse_multistatus};
    use url::Url;

    use super::{MAX_PRINCIPALS, Placement, principals_in};

    #[test]
    fn principals_are_deduplicated_in_linear_time() {
        let mut body = String::from(r#"<multistatus xmlns="DAV:">"#);
        for i in 0..MAX_PRINCIPALS {
            // Each one twice, with and without the trailing slash.
            for slash in ["/", ""] {
                let _ = write!(
                    body,
                    "<response><href>/dav/pal/u{i}{slash}</href><propstat><prop><resourcetype><principal/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>"
                );
            }
        }
        body.push_str("</multistatus>");
        let url = Url::parse("https://dav.example.com/dav/pal/").unwrap();
        let multistatus = parse_multistatus(body.as_bytes(), &url, &XmlLimits::default()).unwrap();
        let collection = Href::from_url(&url).unwrap();
        let found = principals_in(multistatus, &collection, Placement::Member);
        assert_eq!(found.len(), MAX_PRINCIPALS);
        assert!(found.iter().all(|p| p.href.is_collection()));
        assert_eq!(found[0].href.path(), "/dav/pal/u0/");
        assert_eq!(found[1].href.path(), "/dav/pal/u1/");
    }
}
