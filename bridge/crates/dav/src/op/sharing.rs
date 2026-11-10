// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sharing a calendar or an address book:
//!
//! - **WebDAV ACL** (RFC 3744): [`DavClient::read_acl`] reads the
//!   collection's `DAV:acl` right before a change, [`DavClient::set_acl`]
//!   replaces the editable ACEs with the `ACL` method (§8.1).
//! - **Calendar Server sharing**: [`DavClient::share`] POSTs a `CS:share`
//!   to the calendar (only 307/308 redirects are followed for a POST).
//! - **Nextcloud sharing**: [`DavClient::oc_share`] POSTs an `O:share` to
//!   the calendar or address book. Its sharees are paths below the
//!   server's DAV root (`principal:principals/users/bob`), so they can only
//!   name principals of the collection's own server.
//!
//! One `CS:share` holds at most
//! [`MAX_SHARE_REQUEST`](nebula_dav_proto::MAX_SHARE_REQUEST) entries and
//! one `O:share` at most
//! [`MAX_OC_SHARE_REQUEST`](nebula_dav_proto::MAX_OC_SHARE_REQUEST): the
//! caller sends a bigger change in several, checked whole first
//! ([`DavClient::check_share`]). An `ACL` replaces the list whole, so it
//! goes in one request (at most
//! [`MAX_ACES`](nebula_dav_proto::MAX_ACES) ACEs, some 2.5 MB; the request
//! body has no limit of its own, only the answer's).
//!
//! Principals are written as paths, so every principal href must be on the
//! collection's origin (a path of another host would name someone else
//! here): anything else is [`DavError::OriginNotAllowed`], never sent.

use http::{Method, StatusCode};
use nebula_dav_proto::request::{self, XML_CONTENT_TYPE};
use nebula_dav_proto::{
    Ace, AcePrincipal, Acl, Depth, Href, OcPrincipal, Property, ShareAccess, Sharee,
};

use crate::core::client::Call;
use crate::op::response_for;
use crate::{DavClient, DavError};

/// Every principal href is on the collection's origin.
fn same_origin_principals<'a>(
    collection: &Href,
    hrefs: impl IntoIterator<Item = &'a Href>,
) -> Result<(), DavError> {
    if hrefs.into_iter().all(|href| href.same_origin(collection)) {
        Ok(())
    } else {
        Err(DavError::OriginNotAllowed)
    }
}

impl DavClient {
    /// What [`Self::share`] checks before sending: the collection is on
    /// the client's origin and every principal sharee on its origin. A
    /// change sent in several `POST`s is checked whole with this first, so
    /// a bad name in a later one stops all of them, not only its own.
    ///
    /// # Errors
    ///
    /// [`DavError::OriginNotAllowed`].
    pub fn check_share(
        &self,
        collection: &Href,
        set: &[(Sharee, ShareAccess)],
        remove: &[Sharee],
    ) -> Result<(), DavError> {
        let collection = collection.as_collection();
        self.check_origin(&collection)?;
        let principals = set
            .iter()
            .map(|(sharee, _)| sharee)
            .chain(remove)
            .filter_map(|sharee| match sharee {
                Sharee::Principal(href) => Some(href),
                Sharee::Address(_) => None,
            });
        same_origin_principals(&collection, principals)
    }

    /// The collection's `DAV:acl` now (a PROPFIND Depth 0).
    ///
    /// # Errors
    ///
    /// [`DavError::Protocol`] if the server does not show it (no
    /// `read-acl`, or not RFC 3744); or the request's.
    pub async fn read_acl(&self, collection: &Href) -> Result<Acl, DavError> {
        let collection = collection.as_collection();
        let (multistatus, final_url) = self
            .propfind(&collection, Depth::Zero, &[Property::Acl])
            .await?;
        response_for(&multistatus, &collection, &final_url)
            .and_then(|response| response.properties.acl.clone())
            .ok_or(DavError::Protocol("no DAV:acl"))
    }

    /// The `ACL` method: `aces` replace every editable ACE of the
    /// collection (protected and inherited ones are not sent).
    ///
    /// # Errors
    ///
    /// [`DavError::OriginNotAllowed`] for a principal on another origin;
    /// [`DavError::Encode`] for an ACE that cannot be written (unknown
    /// principal, no privileges, too many); [`DavError::Forbidden`] with
    /// the server's precondition (`grant-only`, `no-invert`,
    /// `allowed-principal`…); [`DavError::Redirect`] for a 303 (it would
    /// become a GET); or the request's.
    pub async fn set_acl(&self, collection: &Href, aces: &[Ace]) -> Result<(), DavError> {
        let collection = collection.as_collection();
        self.check_origin(&collection)?;
        same_origin_principals(
            &collection,
            aces.iter().filter_map(|ace| match &ace.principal {
                AcePrincipal::Href(href) => Some(href),
                _ => None,
            }),
        )?;
        let body = request::acl(aces)?;
        let method = Method::from_bytes(b"ACL").map_err(|_| DavError::Protocol("method"))?;
        let call = Call::new(method, &collection, self.server().limits.max_error_body)
            .body(body, XML_CONTENT_TYPE);
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(DavError::Status {
                status: 207,
                precondition: None,
            });
        }
        Ok(())
    }

    /// Calendar Server sharing: a `POST` of `CS:share` that adds or
    /// changes the sharees of `set` and removes those of `remove`.
    ///
    /// # Errors
    ///
    /// [`DavError::OriginNotAllowed`] for a principal on another origin;
    /// [`DavError::Encode`] for nothing to do or too many sharees;
    /// [`DavError::Redirect`] for a 301/302/303; or the request's.
    pub async fn share(
        &self,
        collection: &Href,
        set: &[(Sharee, ShareAccess)],
        remove: &[Sharee],
    ) -> Result<(), DavError> {
        self.check_share(collection, set, remove)?;
        let collection = collection.as_collection();
        let body = request::share(set, remove)?;
        let call = Call::new(
            Method::POST,
            &collection,
            self.server().limits.max_error_body,
        )
        .body(body, XML_CONTENT_TYPE);
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(DavError::Status {
                status: 207,
                precondition: None,
            });
        }
        Ok(())
    }

    /// Nextcloud sharing: a `POST` of `O:share` that adds or changes the
    /// sharees of `set` and removes those of `remove`. Nextcloud answers
    /// 200 even when it ignores a sharee (unknown, the user, a group it
    /// does not share with): callers read `O:invite` again to see.
    ///
    /// # Errors
    ///
    /// [`DavError::Encode`] for nothing to do or too many sharees;
    /// [`DavError::Redirect`] for a 301/302/303; or the request's.
    pub async fn oc_share(
        &self,
        collection: &Href,
        set: &[(OcPrincipal, ShareAccess)],
        remove: &[OcPrincipal],
    ) -> Result<(), DavError> {
        let collection = collection.as_collection();
        self.check_origin(&collection)?;
        let body = request::oc_share(set, remove)?;
        let call = Call::new(
            Method::POST,
            &collection,
            self.server().limits.max_error_body,
        )
        .body(body, XML_CONTENT_TYPE);
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(DavError::Status {
                status: 207,
                precondition: None,
            });
        }
        Ok(())
    }
}
