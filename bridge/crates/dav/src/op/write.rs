// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Creating collections and changing their properties.

use http::{Method, StatusCode};
use nebula_dav_proto::request::{self, CollectionKind as MkcolKind, PropValue, XML_CONTENT_TYPE};
use nebula_dav_proto::{Depth, Href, Property, Status};

use crate::core::client::Call;
use crate::op::response_for;
use crate::{CollectionKind, DavClient, DavError};

impl DavClient {
    /// `MKCALENDAR` (RFC 4791 §5.3.1) with initial properties.
    ///
    /// # Errors
    ///
    /// [`DavError::MethodNotAllowed`] if something is already there,
    /// [`DavError::Forbidden`] with the server's reason,
    /// [`DavError::Redirect`] for a 303 (it would become a GET), or the
    /// request's.
    pub async fn mkcalendar(&self, href: &Href, values: &[PropValue]) -> Result<(), DavError> {
        let href = href.as_collection();
        self.check_origin(&href)?;
        let method = Method::from_bytes(b"MKCALENDAR").map_err(|_| DavError::Protocol("method"))?;
        let mut call = Call::new(method, &href, self.server().limits.max_error_body);
        if let Some(body) = request::mkcalendar(values)? {
            call = call.body(body, XML_CONTENT_TYPE);
        }
        self.created(&call).await
    }

    /// Extended `MKCOL` (RFC 5689) for a calendar or an address book (the
    /// only way to create one in CardDAV).
    ///
    /// # Errors
    ///
    /// As [`DavClient::mkcalendar`].
    pub async fn mkcol(
        &self,
        href: &Href,
        kind: CollectionKind,
        values: &[PropValue],
    ) -> Result<(), DavError> {
        let href = href.as_collection();
        self.check_origin(&href)?;
        let kind = match kind {
            CollectionKind::Calendar => MkcolKind::Calendar,
            CollectionKind::Addressbook => MkcolKind::Addressbook,
        };
        let method = Method::from_bytes(b"MKCOL").map_err(|_| DavError::Protocol("method"))?;
        let call = Call::new(method, &href, self.server().limits.max_multistatus)
            .body(request::mkcol(kind, values)?, XML_CONTENT_TYPE);
        self.created(&call).await
    }

    async fn created(&self, call: &Call<'_>) -> Result<(), DavError> {
        let reply = self.send(call).await?.success()?;
        // RFC 5689 §5.2: a 207 lists properties that could not be set,
        // and the collection was not created.
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(DavError::Status {
                status: 207,
                precondition: None,
            });
        }
        Ok(())
    }

    /// `PROPPATCH`: set and remove properties. Returns the properties the
    /// server refused, with their status (empty: all applied; RFC 4918
    /// says then nothing was applied).
    ///
    /// # Errors
    ///
    /// [`DavError::Redirect`] for a 303 (it would become a GET), or
    /// [`DavError`] for the request.
    pub async fn proppatch(
        &self,
        href: &Href,
        set: &[PropValue],
        remove: &[Property],
    ) -> Result<Vec<(Property, Status)>, DavError> {
        self.check_origin(href)?;
        let method = Method::from_bytes(b"PROPPATCH").map_err(|_| DavError::Protocol("method"))?;
        let limits = self.server().limits;
        let call = Call::new(method, href, limits.max_multistatus)
            .depth(Depth::Zero)
            .body(request::proppatch(set, remove)?, XML_CONTENT_TYPE);
        let (multistatus, final_url) = self.multistatus(&call, limits.max_error_body).await?;
        let failed = response_for(&multistatus, href, &final_url)
            .map(|response| response.failed.clone())
            .unwrap_or_default();
        Ok(failed)
    }
}
