// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What changed in a collection.
//!
//! With `sync-collection` (RFC 6578): changed members with their etags,
//! removed ones (404), and a new token. A truncated answer (507 on the
//! collection, §3.6) is continued with the token it gave. A token the
//! server no longer knows (`DAV:valid-sync-token`, §3.2) starts over
//! from scratch, once, and says so ([`SyncResult::full`]).
//!
//! Without it: [`DavClient::collection_state`] (ctag and token) to see
//! whether anything changed, and [`DavClient::list_members`] (every href
//! and etag) to see what.

use std::collections::HashMap;

use nebula_dav_proto::request::XML_CONTENT_TYPE;
use nebula_dav_proto::{
    CTag, Depth, ETag, Href, Multistatus, Precondition, Property, Status, SyncToken, request,
};

use crate::core::client::Call;
use crate::op::{report_method, response_for};
use crate::{DavClient, DavError};

/// A member of a collection and its entity tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Where it is (a direct member of the collection).
    pub href: Href,
    /// Its etag. `None` when the server left it out (some do in sync
    /// answers): fetch the resource to learn it.
    pub etag: Option<ETag>,
}

/// The result of [`DavClient::sync`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SyncResult {
    /// The token for next time.
    pub token: SyncToken,
    /// Members new or changed since the token.
    pub changed: Vec<Member>,
    /// Members gone since the token.
    pub removed: Vec<Href>,
    /// This is everything in the collection (no token, or the old one was
    /// rejected): whatever the caller has that is not in `changed` is gone.
    pub full: bool,
}

impl SyncResult {
    /// The hrefs of `changed`, to fetch with a multiget.
    #[must_use]
    pub fn changed_hrefs(&self) -> Vec<Href> {
        self.changed
            .iter()
            .map(|member| member.href.clone())
            .collect()
    }
}

/// The version markers of a collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CollectionState {
    /// `getctag`.
    pub ctag: Option<CTag>,
    /// `sync-token`.
    pub sync_token: Option<SyncToken>,
}

enum Change {
    Changed(Option<ETag>),
    Removed,
}

struct Page {
    token: SyncToken,
    truncated: bool,
}

const fn is_invalid_token(error: &DavError) -> bool {
    match error {
        DavError::Forbidden { precondition }
        | DavError::Conflict { precondition }
        | DavError::Status { precondition, .. } => {
            matches!(precondition, Some(Precondition::ValidSyncToken))
        }
        // Some servers answer an unknown token with a bare 412.
        DavError::PreconditionFailed => true,
        _ => false,
    }
}

impl DavClient {
    /// `REPORT sync-collection` from `token` (`None`: everything).
    ///
    /// # Errors
    ///
    /// [`DavError`] for the requests; [`DavError::TooMany`] past
    /// [`Limits::max_resources`](crate::Limits::max_resources);
    /// [`DavError::Protocol`] if an answer has no token or the server
    /// keeps truncating past [`Limits::max_sync_rounds`](crate::Limits::max_sync_rounds).
    pub async fn sync(
        &self,
        collection: &Href,
        token: Option<&SyncToken>,
    ) -> Result<SyncResult, DavError> {
        let collection = collection.as_collection();
        let mut current = token.cloned();
        let mut full = token.is_none();
        let mut restarted = false;
        let mut changes: HashMap<Href, Change> = HashMap::new();
        for _ in 0..self.server().limits.max_sync_rounds {
            match self
                .sync_page(&collection, current.as_ref(), &mut changes)
                .await
            {
                Ok(page) if page.truncated => current = Some(page.token),
                Ok(page) => return Ok(finish(page.token, changes, full)),
                Err(error) if current.is_some() && !restarted && is_invalid_token(&error) => {
                    tracing::debug!("dav: sync token rejected, syncing everything again");
                    restarted = true;
                    full = true;
                    current = None;
                    changes.clear();
                }
                Err(error) => return Err(error),
            }
        }
        Err(DavError::Protocol("sync-collection kept truncating"))
    }

    async fn sync_page(
        &self,
        collection: &Href,
        token: Option<&SyncToken>,
        changes: &mut HashMap<Href, Change>,
    ) -> Result<Page, DavError> {
        self.check_origin(collection)?;
        let limits = self.server().limits;
        let body = request::sync_collection(token, &[Property::GetETag], None)?;
        let call = Call::new(report_method(), collection, limits.max_multistatus)
            .depth(Depth::Zero)
            .body(body, XML_CONTENT_TYPE);
        let (multistatus, final_url) = self.multistatus(&call, limits.max_error_body).await?;
        same_collection(collection, &final_url)?;
        if multistatus.refused > 0 {
            tracing::warn!(
                refused = multistatus.refused,
                "dav: sync answer had hrefs that do not resolve safely"
            );
        }
        let mut truncated = false;
        let mut outside = 0_usize;
        for response in &multistatus.responses {
            if response.href().same_location(collection) {
                truncated |= response.status == Some(Status::INSUFFICIENT_STORAGE);
                continue;
            }
            for href in response.hrefs() {
                if !href.is_member_of(collection) {
                    outside += 1;
                    continue;
                }
                if response
                    .properties
                    .resource_type
                    .is_some_and(|kind| kind.collection)
                {
                    continue;
                }
                // Another error (403, 500…) is not "gone": present, with
                // no etag, so that it is fetched (and kept if that fails).
                let change = match response.status {
                    Some(Status::NOT_FOUND) => Change::Removed,
                    Some(status) if !status.is_success() => Change::Changed(None),
                    _ => Change::Changed(response.properties.etag.clone()),
                };
                changes.insert(href.clone(), change);
            }
        }
        if outside > 0 {
            tracing::warn!(outside, "dav: sync answer had hrefs outside the collection");
        }
        if changes.len() > limits.max_resources {
            return Err(DavError::TooMany);
        }
        let token = multistatus
            .sync_token
            .ok_or(DavError::Protocol("sync answer without sync-token"))?;
        Ok(Page { token, truncated })
    }

    /// The `getctag` and `sync-token` of a collection (Depth 0).
    ///
    /// # Errors
    ///
    /// [`DavError`] for the request.
    pub async fn collection_state(&self, collection: &Href) -> Result<CollectionState, DavError> {
        let collection = collection.as_collection();
        let (multistatus, final_url) = self
            .propfind(
                &collection,
                Depth::Zero,
                &[Property::GetCTag, Property::SyncToken],
            )
            .await?;
        Ok(response_for(&multistatus, &collection, &final_url)
            .map(|response| CollectionState {
                ctag: response.properties.ctag.clone(),
                sync_token: response.properties.sync_token.clone(),
            })
            .unwrap_or_default())
    }

    /// Every resource of a collection with its etag (PROPFIND Depth 1),
    /// for servers without sync-collection. Sub-collections, hrefs
    /// outside the collection and members with 404 are left out; a member
    /// with another error status is listed without etag (it is not gone).
    ///
    /// # Errors
    ///
    /// [`DavError`] for the request; [`DavError::TooMany`] past
    /// [`Limits::max_resources`](crate::Limits::max_resources);
    /// [`DavError::Protocol`] if the collection answered from another
    /// location (a redirect): its hrefs would all look outside it.
    pub async fn list_members(&self, collection: &Href) -> Result<Vec<Member>, DavError> {
        let collection = collection.as_collection();
        let (multistatus, final_url) = self
            .propfind(
                &collection,
                Depth::One,
                &[
                    Property::ResourceType,
                    Property::GetETag,
                    Property::GetContentType,
                ],
            )
            .await?;
        same_collection(&collection, &final_url)?;
        members(
            &multistatus,
            &collection,
            self.server().limits.max_resources,
        )
    }
}

/// A full listing must come from the collection itself: after a redirect
/// every href is relative to the new place and would look outside, so
/// the listing would be empty and everything taken as deleted.
fn same_collection(collection: &Href, final_url: &Href) -> Result<(), DavError> {
    if final_url.as_collection().same_location(collection) {
        Ok(())
    } else {
        Err(DavError::Protocol(
            "collection answered from another location",
        ))
    }
}

fn members(
    multistatus: &Multistatus,
    collection: &Href,
    max: usize,
) -> Result<Vec<Member>, DavError> {
    if multistatus.refused > 0 {
        tracing::warn!(
            refused = multistatus.refused,
            "dav: listing had hrefs that do not resolve safely"
        );
    }
    let mut members = Vec::new();
    for response in &multistatus.responses {
        let href = response.href();
        if !href.is_member_of(collection)
            || response
                .properties
                .resource_type
                .is_some_and(|kind| kind.collection)
            || response.status == Some(Status::NOT_FOUND)
        {
            continue;
        }
        if members.len() >= max {
            return Err(DavError::TooMany);
        }
        // Another error (403, 500…) is not "gone": listed without etag.
        let failed = response.status.is_some_and(|status| !status.is_success());
        members.push(Member {
            href: href.clone(),
            etag: response.properties.etag.clone().filter(|_| !failed),
        });
    }
    Ok(members)
}

fn finish(token: SyncToken, found: HashMap<Href, Change>, full: bool) -> SyncResult {
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    for (href, change) in found {
        match change {
            Change::Changed(etag) => changed.push(Member { href, etag }),
            Change::Removed => removed.push(href),
        }
    }
    let by_path = |a: &Href, b: &Href| a.path().cmp(b.path());
    changed.sort_by(|a, b| by_path(&a.href, &b.href));
    removed.sort_by(by_path);
    SyncResult {
        token,
        changed,
        removed,
        full,
    }
}
