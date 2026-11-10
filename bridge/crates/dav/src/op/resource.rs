// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Resources: multiget, GET, PUT, DELETE, MOVE.

use std::collections::HashSet;

use http::{Method, StatusCode};
use nebula_dav_proto::request::XML_CONTENT_TYPE;
use nebula_dav_proto::{Depth, ETag, Href, Property, Status, VcardRequest, request};

use crate::core::client::Call;
use crate::op::report_method;
use crate::{DavClient, DavError, Limits};

/// An iCalendar object or a vCard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// iCalendar (RFC 5545), in a calendar.
    Calendar,
    /// vCard (RFC 6350), in an address book.
    Contact,
}

impl ResourceKind {
    /// The size limit: 1 MiB for iCalendar, 4 MiB for vCard (photos).
    #[must_use]
    pub const fn limit(self, limits: &Limits) -> usize {
        match self {
            Self::Calendar => limits.max_icalendar,
            Self::Contact => limits.max_vcard,
        }
    }

    const fn content_type(self) -> &'static str {
        match self {
            Self::Calendar => "text/calendar; charset=utf-8",
            Self::Contact => "text/vcard; charset=utf-8",
        }
    }

    /// The `Accept` of a GET. vCard 4.0 is asked for with the media type
    /// parameter sabre/dav reads (`text/vcard; version=4.0`), with plain
    /// vCard as a second choice for servers that only have that.
    const fn accept(self, wanted: VcardRequest) -> &'static str {
        match (self, wanted) {
            (Self::Calendar, _) => "text/calendar",
            (Self::Contact, VcardRequest::Version4) => {
                "text/vcard; version=4.0, text/vcard;q=0.9, text/x-vcard;q=0.5"
            }
            (Self::Contact, _) => "text/vcard, text/x-vcard;q=0.5",
        }
    }

    fn accepts(self, media_type: &str) -> bool {
        match self {
            Self::Calendar => media_type == "text/calendar",
            Self::Contact => matches!(media_type, "text/vcard" | "text/x-vcard" | "text/directory"),
        }
    }
}

/// One resource of a multiget.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fetched {
    /// Where it is (one of the hrefs asked for).
    pub href: Href,
    /// Its etag, if the server gave one.
    pub etag: Option<ETag>,
    /// The iCalendar or vCard text (CRLF line ends). `None` when the
    /// server did not give it: with a `status` saying why, or without one
    /// if the server gave nothing usable (an empty or malformed value).
    pub data: Option<String>,
    /// Why the data is missing: the response's status if it is not 2xx,
    /// else the status of the data property (only a real 404 means
    /// deleted meanwhile; 403, 500… are not; 507 is a resource over the
    /// kind's limit, alone or in a batch that cannot be split further).
    pub status: Option<Status>,
}

/// A resource read with GET.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Resource {
    /// Its etag, if the server gave one.
    pub etag: Option<ETag>,
    /// The text.
    pub data: String,
}

/// When a PUT may write (never unconditionally).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutCondition {
    /// Only if nothing is there (`If-None-Match: *`).
    Create,
    /// Only if it still has this etag (`If-Match`).
    Update(ETag),
}

impl DavClient {
    /// `calendar-multiget` / `addressbook-multiget` of these members of
    /// `collection`, in batches of
    /// [`Limits::multiget_batch`](crate::Limits::multiget_batch). Answers
    /// for hrefs that were not asked for are ignored. A batch whose answer
    /// is over [`Limits::max_multistatus`](crate::Limits::max_multistatus)
    /// (or over the XML node or attribute limits,
    /// [`DavError::is_oversized`]) is split in two and asked again (at most two splits per batch on
    /// average); a single resource still too large comes back with
    /// status 507 and no data.
    ///
    /// # Errors
    ///
    /// [`DavError::OutsideCollection`] if an href is not a direct member;
    /// [`DavError::TooLarge`] (or the XML limit) if the answers stay too large after the
    /// splits allowed; [`DavError`] for the requests.
    pub async fn multiget(
        &self,
        collection: &Href,
        kind: ResourceKind,
        hrefs: &[Href],
    ) -> Result<Vec<Fetched>, DavError> {
        self.multiget_as(collection, kind, hrefs, VcardRequest::AsStored)
            .await
    }

    /// [`Self::multiget`] asking for vCards as `version` (ignored for
    /// calendars). Etags and hrefs are the same either way: the server
    /// only converts the text it sends.
    ///
    /// # Errors
    ///
    /// As [`Self::multiget`].
    pub async fn multiget_as(
        &self,
        collection: &Href,
        kind: ResourceKind,
        hrefs: &[Href],
        version: VcardRequest,
    ) -> Result<Vec<Fetched>, DavError> {
        let collection = collection.as_collection();
        self.check_origin(&collection)?;
        if hrefs.iter().any(|href| !href.is_member_of(&collection)) {
            return Err(DavError::OutsideCollection);
        }
        let limits = self.server().limits;
        let mut fetched = Vec::with_capacity(hrefs.len());
        let mut pending: Vec<&[Href]> = hrefs.chunks(limits.multiget_batch.max(1)).rev().collect();
        // Against a server that is always too large, splitting every batch
        // down to single hrefs would cost two requests per href.
        let mut splits_left = pending.len().saturating_mul(2);
        while let Some(batch) = pending.pop() {
            match self.multiget_batch(&collection, kind, batch, version).await {
                Ok(mut found) => fetched.append(&mut found),
                Err(error) if error.is_oversized() => match batch {
                    [] => {}
                    [href] => fetched.push(Fetched {
                        href: href.clone(),
                        etag: None,
                        data: None,
                        status: Some(Status::INSUFFICIENT_STORAGE),
                    }),
                    _ if splits_left == 0 => return Err(error),
                    _ => {
                        splits_left -= 1;
                        let (first, second) = batch.split_at(batch.len() / 2);
                        pending.push(second);
                        pending.push(first);
                    }
                },
                Err(other) => return Err(other),
            }
        }
        Ok(fetched)
    }

    /// One REPORT of a multiget.
    async fn multiget_batch(
        &self,
        collection: &Href,
        kind: ResourceKind,
        batch: &[Href],
        version: VcardRequest,
    ) -> Result<Vec<Fetched>, DavError> {
        let limits = self.server().limits;
        let wanted: HashSet<&Href> = batch.iter().collect();
        let body = match kind {
            ResourceKind::Calendar => request::calendar_multiget(batch)?,
            ResourceKind::Contact => request::addressbook_multiget_as(batch, version)?,
        };
        let call = Call::new(report_method(), collection, limits.max_multistatus)
            .depth(Depth::One)
            .body(body, XML_CONTENT_TYPE);
        let (multistatus, _final_url) = self.multistatus(&call, kind.limit(&limits)).await?;
        let property = match kind {
            ResourceKind::Calendar => Property::CalendarData,
            ResourceKind::Contact => Property::AddressData,
        };
        let mut fetched = Vec::with_capacity(batch.len());
        for response in multistatus.responses {
            let properties = &response.properties;
            let data = match kind {
                ResourceKind::Calendar => properties.calendar_data.clone(),
                ResourceKind::Contact => properties.address_data.clone(),
            };
            // The response's own status, else the data's: only a real 404
            // says the resource is gone.
            let status = response
                .status
                .filter(|status| !status.is_success())
                .or_else(|| {
                    if data.is_some() {
                        return None;
                    }
                    response
                        .status_of(property)
                        .filter(|status| !status.is_success())
                });
            for href in response.hrefs().filter(|href| wanted.contains(href)) {
                fetched.push(Fetched {
                    href: href.clone(),
                    etag: properties.etag.clone(),
                    data: data.clone().filter(|_| status.is_none()),
                    status,
                });
            }
        }
        Ok(fetched)
    }

    /// GET one resource. Its type must be the kind's (`text/calendar`,
    /// `text/vcard`), its body UTF-8.
    ///
    /// # Errors
    ///
    /// [`DavError::NotFound`], [`DavError::ContentType`],
    /// [`DavError::NotUtf8`], [`DavError::TooLarge`], or the request's.
    pub async fn get(&self, href: &Href, kind: ResourceKind) -> Result<Resource, DavError> {
        self.get_as(href, kind, VcardRequest::AsStored).await
    }

    /// [`Self::get`] asking for a vCard as `wanted` (ignored for
    /// calendars).
    ///
    /// # Errors
    ///
    /// As [`Self::get`].
    pub async fn get_as(
        &self,
        href: &Href,
        kind: ResourceKind,
        wanted: VcardRequest,
    ) -> Result<Resource, DavError> {
        self.check_origin(href)?;
        let limits = self.server().limits;
        let mut call = Call::new(Method::GET, href, kind.limit(&limits));
        call.accept = Some(kind.accept(wanted));
        let reply = self.send(&call).await?.success()?;
        if reply.status != StatusCode::OK {
            return Err(DavError::Protocol("expected 200 OK"));
        }
        if let Some(media) = reply.media_type()
            && !kind.accepts(&media)
        {
            return Err(DavError::ContentType);
        }
        let etag = reply.etag();
        let data = String::from_utf8(reply.body.to_vec()).map_err(|_| DavError::NotUtf8)?;
        Ok(Resource { etag, data })
    }

    /// PUT a resource, only if `condition` holds. Returns the new etag if
    /// the server gave one (some do not: GET it).
    ///
    /// # Errors
    ///
    /// [`DavError::PreconditionFailed`] if someone else changed or created
    /// it; [`DavError::TooLarge`] for data over the kind's limit (not
    /// sent); [`DavError::Forbidden`] or [`DavError::Conflict`] with the
    /// server's reason (`no-uid-conflict`, `supported-calendar-component`…);
    /// [`DavError::Redirect`] for a 303 (it would become a GET).
    pub async fn put(
        &self,
        href: &Href,
        kind: ResourceKind,
        data: &str,
        condition: &PutCondition,
    ) -> Result<Option<ETag>, DavError> {
        self.check_origin(href)?;
        let limits = self.server().limits;
        if data.len() > kind.limit(&limits) {
            return Err(DavError::TooLarge);
        }
        let mut call = Call::new(Method::PUT, href, limits.max_error_body)
            .body(data.to_owned(), kind.content_type());
        match condition {
            PutCondition::Create => call.if_none_match_any = true,
            PutCondition::Update(etag) => call.if_match = Some(etag),
        }
        let reply = self.send(&call).await?.success()?;
        Ok(reply.etag())
    }

    /// DELETE a resource (or a collection), only if it still has `etag`
    /// when one is given.
    ///
    /// # Errors
    ///
    /// [`DavError::NotFound`], [`DavError::PreconditionFailed`],
    /// [`DavError::Redirect`] for a 303 (it would become a GET), or the
    /// request's. A 207 (some members could not be deleted) is
    /// [`DavError::Status`].
    pub async fn delete(&self, href: &Href, etag: Option<&ETag>) -> Result<(), DavError> {
        self.check_origin(href)?;
        let mut call = Call::new(Method::DELETE, href, self.server().limits.max_error_body);
        call.if_match = etag;
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(reply.into_error());
        }
        Ok(())
    }

    /// MOVE a resource to `to` (another collection on the same origin),
    /// only if it still has `etag` when one is given. Never overwrites
    /// unless `overwrite`.
    ///
    /// # Errors
    ///
    /// [`DavError::OriginNotAllowed`] if `to` is on another origin than
    /// `from`; [`DavError::PreconditionFailed`] if `to` exists and
    /// `overwrite` is false, or the etag changed; [`DavError::Redirect`]
    /// for a 303 (it would become a GET).
    pub async fn move_to(
        &self,
        from: &Href,
        to: &Href,
        etag: Option<&ETag>,
        overwrite: bool,
    ) -> Result<Option<ETag>, DavError> {
        self.check_origin(from)?;
        if !to.same_origin(from) {
            return Err(DavError::OriginNotAllowed);
        }
        let move_method = Method::from_bytes(b"MOVE").map_err(|_| DavError::Protocol("method"))?;
        let mut call = Call::new(move_method, from, self.server().limits.max_error_body);
        call.destination = Some(to);
        call.overwrite = Some(overwrite);
        call.if_match = etag;
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(reply.into_error());
        }
        Ok(reply.etag())
    }
}
