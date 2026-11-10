// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendars and contacts over CalDAV/CardDAV: the sibling
//! of `nebula-mail` for groupware.
//!
//! - [`Groupware`]: the service. [`Groupware::open`] starts an account
//!   (discovery and sync in the background, with the IMAP credentials);
//!   [`Groupware::close`] stops it; [`Groupware::watch`] counts open push
//!   connections (sync every 60 s with one, 5 min without);
//!   with [`GroupwareConfig::contacts_server`] (`contacts_url`, as
//!   Fastmail), a second DAV client per account for address books only,
//!   each client sending the login only to its own configured origin;
//!   [`Groupware::kick`] asks for a sync now. [`Groupware::subscribe`]
//!   gives [`GroupwareEvent`]s: new JMAP states to push, or a refused
//!   login (401: calendars and contacts off for that account, mail goes
//!   on, nothing retried until the next login).
//! - [`GroupwareAccount`]: one account: [`GroupwareAccount::discover`],
//!   [`GroupwareAccount::sync_round`], its [`DavStatus`] and
//!   [`Availability`].
//! - Sync (`sync/`): one PROPFIND Depth 1 per home and round, then
//!   `sync-collection` (or ctag + etags) only for collections that
//!   changed, multiget in batches of 100. Each resource is sealed with the
//!   account's data key and indexed with
//!   calcard (`UID`, UTC start and end with bounded expansion, sort name).
//!   A resource that does not parse is kept but not shown; it never breaks
//!   the sync.
//! - [`model::convert`]: iCalendar ↔ JSCalendar and vCard ↔ JSContact with
//!   calcard (Stalwart's library, vendored in `vendor/calcard`), plus
//!   recurrence expansion. Every call has an input size limit and runs
//!   inside `catch_unwind`, so a hostile calendar or card fails that one
//!   resource instead of the request (`panic = "unwind"` in release).
//!
//! - Writes (`write/`, for the JMAP `/set` methods): PUT with
//!   `If-None-Match: *` or `If-Match`, DELETE and MOVE with `If-Match`,
//!   extended MKCOL, PROPPATCH; each one stored at once (sealed, with the
//!   new etag) and followed by a [kick](GroupwareAccount::kick). A 412 is
//!   [`GroupwareError::is_conflict`]: read again with
//!   [`GroupwareAccount::refetch`] and retry once.
//!
//! - [`model::edit`] and [`model::itip`]: a stored event
//!   opened as JSCalendar for a patch and written back as iCalendar, and
//!   the invitations of a write (`SCHEDULE-AGENT=CLIENT` where a
//!   scheduling server must not send; the organizer's REQUEST/CANCEL when
//!   the bridge sends them).
//! - Alerts ([`alerts`]): a scheduler per account sends
//!   [`GroupwareEvent::CalendarAlert`] when a `display` alert is due, at
//!   most once per event, alert and occurrence (also across restarts).
//!
//! - Principals and free-busy ([`principals`]): the user's principal and
//!   the others the server shows (listing or address search under its
//!   `principal-collection-set`), and busy times through the scheduling
//!   outbox (RFC 6638 §5) for `Principal/getAvailability`.
//!
//! - Sharing ([`sharing`]): whose each collection is and who it is
//!   shared with (WebDAV ACL or Calendar Server sharing), and
//!   [`GroupwareAccount::share_collection`] to change it.
//!
//! The JMAP methods are in `nebula-jmap`.

#![forbid(unsafe_code)]

pub mod alerts;
mod availability;
mod config;
mod core;
mod error;
mod event;
pub mod model;
mod nextcloud;
pub mod principals;
mod service;
pub mod sharing;
mod sync;
mod write;

pub use availability::{MAX_DETAILED, OwnEvent, OwnPeriod};
pub use config::{GroupwareConfig, Scheduling};
pub use core::account::{Availability, DavStatus, GroupwareAccount, NextcloudSupport};
pub use error::{ConvertError, GroupwareError};
pub use event::{GroupwareEvent, StateChanged};
pub use nextcloud::{Trash, TrashItem, TrashKind, TrashedCalendar, TrashedEvent};
pub use service::{AlertListener, Groupware, Watch};
pub use write::{Contents, ResourceText};
