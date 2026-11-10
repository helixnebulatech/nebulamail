// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Operations, each an `impl DavClient` block.

pub(crate) mod collections;
pub(crate) mod discover;
pub(crate) mod freebusy;
pub(crate) mod nextcloud;
pub(crate) mod principals;
pub(crate) mod resource;
pub(crate) mod sharing;
pub(crate) mod sync;
pub(crate) mod write;

use http::{Method, StatusCode};
use nebula_dav_proto::request::XML_CONTENT_TYPE;
use nebula_dav_proto::{
    DavCapabilities, Depth, Href, Multistatus, Property, Response, XmlLimits, parse_multistatus,
    request,
};

use crate::core::client::{Call, Reply};
use crate::{DavClient, DavError};

pub(crate) fn propfind_method() -> Method {
    // A valid token: cannot fail.
    Method::from_bytes(b"PROPFIND").unwrap_or(Method::GET)
}

pub(crate) fn report_method() -> Method {
    Method::from_bytes(b"REPORT").unwrap_or(Method::GET)
}

/// The response about `href` itself in a Depth 0 answer: the one with
/// its location (or the final URL after redirects), else the only one.
pub(crate) fn response_for<'a>(
    multistatus: &'a Multistatus,
    href: &Href,
    final_url: &Href,
) -> Option<&'a Response> {
    multistatus
        .responses
        .iter()
        .find(|r| r.href().same_location(href) || r.href().same_location(final_url))
        .or(match multistatus.responses.as_slice() {
            [only] => Some(only),
            _ => None,
        })
}

impl DavClient {
    pub(crate) fn xml_limits(&self, max_value_bytes: usize) -> XmlLimits {
        let limits = &self.server().limits;
        XmlLimits {
            max_bytes: limits.max_multistatus,
            max_nodes: limits.max_xml_nodes,
            max_value_bytes,
            ..XmlLimits::default()
        }
    }

    /// Sends a call that answers with a multistatus and parses it.
    pub(crate) async fn multistatus(
        &self,
        call: &Call<'_>,
        max_value_bytes: usize,
    ) -> Result<(Multistatus, Href), DavError> {
        let reply = self.send(call).await?.success()?;
        if reply.status != StatusCode::MULTI_STATUS {
            return Err(DavError::Protocol("expected 207 Multi-Status"));
        }
        reply.expect_xml()?;
        let multistatus = parse_multistatus(
            &reply.body,
            reply.url.url(),
            &self.xml_limits(max_value_bytes),
        )?;
        Ok((multistatus, reply.url))
    }

    /// PROPFIND with these properties.
    pub(crate) async fn propfind(
        &self,
        href: &Href,
        depth: Depth,
        properties: &[Property],
    ) -> Result<(Multistatus, Href), DavError> {
        self.propfind_as(href, depth, properties, false).await
    }

    /// PROPFIND, from discovery (`discovering`: any iCloud host of the
    /// server's pattern is the account's) or not.
    pub(crate) async fn propfind_as(
        &self,
        href: &Href,
        depth: Depth,
        properties: &[Property],
        discovering: bool,
    ) -> Result<(Multistatus, Href), DavError> {
        self.check(href, discovering)?;
        let limits = self.server().limits;
        let mut call = Call::new(propfind_method(), href, limits.max_multistatus)
            .depth(depth)
            .body(request::propfind(properties), XML_CONTENT_TYPE);
        call.discovering = discovering;
        // Values of these properties are small; data comes by multiget.
        self.multistatus(&call, limits.max_vcard).await
    }

    /// PROPFIND whose answer may be at most `max_body` bytes, and how many
    /// bytes it cost: the body read, or `max_body` for one too big to read
    /// whole (what was read before it stopped).
    pub(crate) async fn propfind_sized(
        &self,
        href: &Href,
        depth: Depth,
        properties: &[Property],
        max_body: usize,
    ) -> (Result<(Multistatus, Href), DavError>, usize) {
        if let Err(error) = self.check(href, false) {
            return (Err(error), 0);
        }
        let limits = self.server().limits;
        let call = Call::new(propfind_method(), href, max_body)
            .depth(depth)
            .body(request::propfind(properties), XML_CONTENT_TYPE);
        let reply = match self.send(&call).await.and_then(Reply::success) {
            Ok(reply) => reply,
            Err(error) if error.is_oversized() => return (Err(error), max_body),
            Err(error) => return (Err(error), limits.max_error_body.min(max_body)),
        };
        let cost = reply.body.len();
        if reply.status != StatusCode::MULTI_STATUS {
            return (Err(DavError::Protocol("expected 207 Multi-Status")), cost);
        }
        if let Err(error) = reply.expect_xml() {
            return (Err(error), cost);
        }
        let parsed = parse_multistatus(
            &reply.body,
            reply.url.url(),
            &self.xml_limits(limits.max_vcard),
        )
        .map_err(DavError::from)
        .map(|multistatus| (multistatus, reply.url.clone()));
        (parsed, cost)
    }

    /// PROPFIND without Nextcloud's webcal caching header: subscriptions
    /// show as `CS:subscribed` with their `CS:source`.
    pub(crate) async fn propfind_plain(
        &self,
        href: &Href,
        depth: Depth,
        properties: &[Property],
    ) -> Result<(Multistatus, Href), DavError> {
        self.check(href, false)?;
        let limits = self.server().limits;
        let mut call = Call::new(propfind_method(), href, limits.max_multistatus)
            .depth(depth)
            .body(request::propfind(properties), XML_CONTENT_TYPE);
        call.plain_subscriptions = true;
        self.multistatus(&call, limits.max_vcard).await
    }

    /// `OPTIONS`: what the server announces in `DAV:`.
    ///
    /// # Errors
    ///
    /// [`DavError`] for the request; a non-2xx answer is its status.
    pub async fn options(&self, href: &Href) -> Result<DavCapabilities, DavError> {
        self.check_origin(href)?;
        let call = Call::new(Method::OPTIONS, href, self.server().limits.max_error_body);
        let reply: Reply = self.send(&call).await?.success()?;
        let values = reply
            .headers
            .get_all("dav")
            .iter()
            .filter_map(|value| value.to_str().ok());
        Ok(DavCapabilities::from_headers(values))
    }
}
