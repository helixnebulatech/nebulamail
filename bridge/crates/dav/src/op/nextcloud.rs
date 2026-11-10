// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Nextcloud's calendar extras, each used
//! only where the server announced it:
//!
//! - **Publishing** (`oc-calendar-publishing`): [`DavClient::publish`]
//!   POSTs `CS:publish-calendar` or `CS:unpublish-calendar`; the public
//!   link comes back as `CS:publish-url` in the home listing.
//! - **Subscriptions** (`calendarserver-subscribed`):
//!   [`DavClient::subscribe`] creates a `CS:subscribed` collection with
//!   its `CS:source`. Nextcloud fetches the source, with its own rules;
//!   the bridge never does.
//! - **Trash bin** (`nc-calendar-trashbin`): [`DavClient::trash_bin`]
//!   lists the deleted calendars of a home and the deleted events of its
//!   `trashbin/objects/`; [`DavClient::restore`] MOVEs one to
//!   `trashbin/restore/`, [`DavClient::delete_forever`] deletes it with
//!   `X-NC-CalDAV-No-Trashbin: 1`.

use http::{Method, StatusCode};
use nebula_dav_proto::request::{self, PropValue, XML_CONTENT_TYPE};
use nebula_dav_proto::{CalendarColor, Depth, ETag, ExternalUrl, Href, Property, ResourceName};

use crate::core::client::Call;
use crate::op::{report_method, response_for};
use crate::{DavClient, DavError};

/// Most deleted events read from a trash bin (the newest are not chosen:
/// the server's order is kept).
pub const MAX_TRASH_EVENTS: usize = 2000;

/// The trash bin's name in a Nextcloud calendar home.
const TRASH_BIN: &str = "trashbin";

/// A calendar in the trash bin (`NC:deleted-calendar`, still a member of
/// its home).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedCalendar {
    /// Where it is.
    pub href: Href,
    /// `displayname`.
    pub name: Option<String>,
    /// `calendar-color`.
    pub color: Option<CalendarColor>,
    /// `NC:deleted-at` (RFC 3339 text).
    pub deleted_at: Option<String>,
}

/// An event in the trash bin (`trashbin/objects/…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedEvent {
    /// Where it is.
    pub href: Href,
    /// `getetag`.
    pub etag: Option<ETag>,
    /// `NC:calendar-uri`: the last segment of the calendar it was in.
    pub calendar_uri: Option<String>,
    /// `NC:deleted-at` (RFC 3339 text).
    pub deleted_at: Option<String>,
    /// `calendar-data` (CRLF line ends).
    pub data: Option<String>,
}

/// What a home's trash bin holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrashBin {
    /// `NC:trash-bin-retention-duration`, in seconds.
    pub retention: Option<u64>,
    /// Deleted calendars.
    pub calendars: Vec<DeletedCalendar>,
    /// Deleted events (at most [`MAX_TRASH_EVENTS`]).
    pub events: Vec<DeletedEvent>,
}

/// What [`DavClient::trash_bin`] asks for about deleted events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrashContents {
    /// Their `calendar-data` too, to show them. If the answer is over
    /// [`Limits::max_multistatus`](crate::Limits::max_multistatus), it is
    /// asked again without it.
    WithData,
    /// Only hrefs, etags, calendars and dates: enough to restore or delete
    /// one, whatever the size of the events.
    HrefsOnly,
}

fn segment(name: &str) -> Result<ResourceName, DavError> {
    ResourceName::try_from(name).map_err(DavError::Href)
}

impl DavClient {
    /// Publishes a calendar at a public link, or stops publishing it
    /// (Nextcloud's `oc-calendar-publishing`).
    ///
    /// # Errors
    ///
    /// [`DavError::Redirect`] for a 301/302/303; a refusal (403: the
    /// admin turned publishing off, not the owner) with its status; or
    /// the request's.
    pub async fn publish(&self, calendar: &Href, publish: bool) -> Result<(), DavError> {
        let calendar = calendar.as_collection();
        self.check_origin(&calendar)?;
        let call = Call::new(Method::POST, &calendar, self.server().limits.max_error_body)
            .body(request::publish_calendar(publish), XML_CONTENT_TYPE);
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(reply.into_error());
        }
        Ok(())
    }

    /// Creates a webcal subscription at `href` (a new member of a calendar
    /// home) from `source`, with `values` (name, colour). The server
    /// fetches the source; the bridge never connects to it.
    ///
    /// # Errors
    ///
    /// As [`DavClient::mkcol`](crate::DavClient::mkcol).
    pub async fn subscribe(
        &self,
        href: &Href,
        source: &ExternalUrl,
        values: &[PropValue],
    ) -> Result<(), DavError> {
        let href = href.as_collection();
        self.check_origin(&href)?;
        let method = Method::from_bytes(b"MKCOL").map_err(|_| DavError::Protocol("method"))?;
        let call = Call::new(method, &href, self.server().limits.max_multistatus)
            .body(request::subscribe(source, values)?, XML_CONTENT_TYPE);
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(DavError::Status {
                status: 207,
                precondition: None,
            });
        }
        Ok(())
    }

    /// The trash bin of a calendar home: its retention, the deleted
    /// calendars (members of the home, at most
    /// [`Limits::max_collections`](crate::Limits::max_collections)) and
    /// the deleted events (with their data or not, `contents`). An event
    /// over [`Limits::max_icalendar`](crate::Limits::max_icalendar) comes
    /// without data.
    ///
    /// # Errors
    ///
    /// [`DavError`] for any of the requests.
    pub async fn trash_bin(
        &self,
        home: &Href,
        contents: TrashContents,
    ) -> Result<TrashBin, DavError> {
        let home = home.as_collection();
        let bin = home.member(&segment(TRASH_BIN)?, true)?;
        let (multistatus, final_url) = self
            .propfind(&bin, Depth::Zero, &[Property::TrashBinRetentionDuration])
            .await?;
        let retention = response_for(&multistatus, &bin, &final_url)
            .and_then(|r| r.properties.trash_bin_retention);

        let (listing, _final_url) = self
            .propfind(
                &home,
                Depth::One,
                &[
                    Property::ResourceType,
                    Property::DisplayName,
                    Property::CalendarColor,
                    Property::DeletedAt,
                ],
            )
            .await?;
        let mut calendars = Vec::new();
        for response in listing.responses {
            let deleted = response
                .properties
                .resource_type
                .is_some_and(|kind| kind.deleted_calendar);
            if !deleted || !response.href().is_member_of(&home) {
                continue;
            }
            if calendars.len() >= self.server().limits.max_collections {
                tracing::warn!("dav: the trash bin has more calendars than are shown");
                break;
            }
            let href = response.href().as_collection();
            let p = response.properties;
            calendars.push(DeletedCalendar {
                href,
                name: p.display_name,
                color: p.calendar_color,
                deleted_at: p.deleted_at,
            });
        }

        let objects = bin.member(&segment("objects")?, true)?;
        self.check_origin(&objects)?;
        let found = match contents {
            TrashContents::WithData => match self.deleted_events(&objects, true).await {
                Err(error) if error.is_oversized() => {
                    tracing::warn!("dav: the trash bin is too large to show with its events' data");
                    self.deleted_events(&objects, false).await?
                }
                other => other?,
            },
            TrashContents::HrefsOnly => self.deleted_events(&objects, false).await?,
        };
        let mut events = Vec::new();
        for response in found.responses {
            if !response.href().is_member_of(&objects) || response.href().is_collection() {
                continue;
            }
            if events.len() >= MAX_TRASH_EVENTS {
                tracing::warn!("dav: the trash bin has more events than are shown");
                break;
            }
            let href = response.href().clone();
            let p = response.properties;
            events.push(DeletedEvent {
                href,
                etag: p.etag,
                calendar_uri: p.calendar_uri,
                deleted_at: p.deleted_at,
                data: p.calendar_data,
            });
        }
        Ok(TrashBin {
            retention,
            calendars,
            events,
        })
    }

    /// The `calendar-query` of `trashbin/objects/`.
    async fn deleted_events(
        &self,
        objects: &Href,
        with_data: bool,
    ) -> Result<nebula_dav_proto::Multistatus, DavError> {
        let limits = self.server().limits;
        let call = Call::new(report_method(), objects, limits.max_multistatus)
            .depth(Depth::One)
            .body(request::trash_bin_query(with_data), XML_CONTENT_TYPE);
        let (found, _final_url) = self.multistatus(&call, limits.max_icalendar).await?;
        Ok(found)
    }

    /// Restores a deleted calendar or event of `home`'s trash bin: a MOVE
    /// to `trashbin/restore/`.
    ///
    /// # Errors
    ///
    /// [`DavError::OriginNotAllowed`] if `item` is not on the home's
    /// origin; [`DavError::NotFound`] if it is gone; or the request's.
    pub async fn restore(&self, home: &Href, item: &Href) -> Result<(), DavError> {
        let home = home.as_collection();
        let name = item
            .last_segment()
            .ok_or(DavError::Protocol("nothing to restore"))?;
        let target = home
            .member(&segment(TRASH_BIN)?, true)?
            .member(&segment("restore")?, true)?
            .member(&segment(name)?, false)?;
        if !item.is_inside(&home) {
            return Err(DavError::OriginNotAllowed);
        }
        self.move_to(item, &target, None, false).await.map(|_| ())
    }

    /// Deletes a calendar or event of the trash bin for good.
    ///
    /// # Errors
    ///
    /// [`DavError::NotFound`] if it is gone, or the request's.
    pub async fn delete_forever(&self, item: &Href) -> Result<(), DavError> {
        self.check_origin(item)?;
        let mut call = Call::new(Method::DELETE, item, self.server().limits.max_error_body);
        call.no_trashbin = true;
        let reply = self.send(&call).await?.success()?;
        if reply.status == StatusCode::MULTI_STATUS {
            return Err(reply.into_error());
        }
        Ok(())
    }
}
