// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! CalDAV/CardDAV client, on top of `nebula-dav-proto`.
//!
//! - **Connections**: hyper's HTTP/1.1 client connection over
//!   `net_guard::connect` (every resolved address checked, the connection
//!   pinned to the validated IP) and `nebula-tls` (rustls, platform
//!   verifier; no verification only for loopback). Plain `http` only to a
//!   loopback peer (Docker). No proxies from the environment.
//! - **Where requests go**: the configured server's origin, the account's
//!   own iCloud host (only for the two iCloud origins, [`AccountHosts`]),
//!   or an origin the admin allowed ([`AllowedOrigin`]), all decided by
//!   [`DavServer::trust`]. Everything the server says
//!   (redirects, principal, homes, hrefs) is checked against that, and
//!   resources against their collection.
//! - **Redirects** by hand: at most 5, each hop checked again; `303` only
//!   for GET and HEAD (any other method would become a GET), a POST only
//!   with 307/308. Basic credentials only to the server's own origin (and
//!   its iCloud account host), never after a redirect elsewhere.
//! - **Limits** ([`Limits`]): multistatus 16 MiB, iCalendar 1 MiB, vCard
//!   4 MiB (also after decompressing: we ask for `identity` and decode
//!   gzip/deflate with the same cap), connection 5 s, request 60 s, 15 s
//!   without data while a body comes, and a connection unused for 15 s is
//!   not reused. The wait for the response head has only the 60 s of the
//!   request: servers build big reports before they answer.
//! - **Logs**: methods and statuses only, at debug level; never
//!   credentials, hrefs, names or bodies.
//!
//! Operations (in `op/`, quirks from libdav 0.11): [`DavClient::discover`]
//! (RFC 6764 `.well-known`, `current-user-principal`, homes, addresses,
//! outbox, `DAV:` capabilities), [`DavClient::list_collections`],
//! [`DavClient::sync`] (RFC 6578 with 507 continuation; an expired token
//! starts over), [`DavClient::collection_state`] and
//! [`DavClient::list_members`] (ctag + etags when there is no
//! sync-collection), [`DavClient::multiget`], [`DavClient::get`],
//! [`DavClient::put`], [`DavClient::delete`], [`DavClient::move_to`],
//! [`DavClient::mkcalendar`], [`DavClient::mkcol`],
//! [`DavClient::proppatch`]; for free-busy and the principals of the
//! server: [`DavClient::free_busy`] (a `VFREEBUSY` POST on the
//! scheduling outbox, RFC 6638 §5; only 307/308 redirects are followed
//! for a POST), [`DavClient::list_principals`],
//! [`DavClient::search_principals`] and
//! [`DavClient::principal_search_properties`] (RFC 3744 §9.4–9.5, only
//! inside a principal collection discovery found); for sharing:
//! [`DavClient::read_acl`] and [`DavClient::set_acl`] (RFC 3744 `ACL`)
//! and [`DavClient::share`] (a Calendar Server `CS:share` POST),
//! principals only on the collection's origin.
//!
//! ```no_run
//! # async fn example() -> Result<(), nebula_dav::DavError> {
//! use secrecy::SecretString;
//! use nebula_dav::{Credentials, DavClient, DavServer};
//!
//! let server = DavServer::new("https://dav.example.com/")?;
//! let credentials = Credentials::new("alice", SecretString::from("secret"))?;
//! let client = DavClient::new(server, Some(credentials))?;
//! let found = client.discover().await?;
//! for home in &found.calendar_homes {
//!     for calendar in client.list_collections(home).await? {
//!         let changes = client.sync(&calendar.href, None).await?;
//!         let _events = client.multiget(&calendar.href, calendar.kind.resource(), &changes.changed_hrefs()).await?;
//!     }
//! }
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod config;
mod core;
mod error;
mod op;
mod scope;

// The fake account hosts are for tests only: a release build never has
// them.
#[cfg(all(feature = "test-account-hosts", not(debug_assertions)))]
compile_error!("the `test-account-hosts` feature is for tests only, never for a release build");

pub use crate::config::{AllowedOrigin, Credentials, DavServer, Limits, Origin, Scheme};
pub use crate::core::client::DavClient;
pub use error::{DavError, RedirectError};
pub use op::collections::{
    Collection, CollectionKind, HomeListing, ListOptions, SharingData, Subscription,
};
pub use op::discover::Discovery;
pub use op::freebusy::{FreeBusyAnswer, MAX_FREEBUSY_RESPONSE};
pub use op::nextcloud::{DeletedCalendar, DeletedEvent, MAX_TRASH_EVENTS, TrashBin, TrashContents};
pub use op::principals::{DavPrincipal, MAX_PRINCIPALS};
pub use op::resource::{Fetched, PutCondition, Resource, ResourceKind};
pub use op::sync::{CollectionState, Member, SyncResult};
pub use scope::{AccountHost, AccountHosts, MAX_PARTITION_DIGITS, Trust};

/// The protocol types (hrefs, tags, properties), re-exported.
pub use nebula_dav_proto as proto;
/// Certificate checks, re-exported.
pub use nebula_tls::TlsVerify;
