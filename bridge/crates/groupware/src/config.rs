// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The DAV server of every account and the tuning of the groupware layer.
//! Built by the binary from `[upstream.dav]`.
//!
//! ```
//! use nebula_dav::DavServer;
//! use nebula_groupware::{GroupwareConfig, Scheduling};
//!
//! let server = DavServer::new("https://dav.example.com/").unwrap();
//! let config = GroupwareConfig::new(server);
//! assert_eq!(config.scheduling, Scheduling::Auto);
//! assert_eq!(config.poll_with_push.as_secs(), 60);
//!
//! // Address books on another host (Fastmail):
//! let calendars = DavServer::new("https://caldav.fastmail.com/").unwrap();
//! let contacts = DavServer::new("https://carddav.fastmail.com/").unwrap();
//! let split = GroupwareConfig::new(calendars).with_contacts_server(contacts);
//! assert!(split.contacts_server.is_some());
//! ```

use std::time::Duration;

use nebula_dav::DavServer;

use crate::alerts::AlertConfig;

/// Who sends invitations (iTIP over iMIP) when an event with attendees
/// changes, read by the JMAP writes
/// ([`GroupwareAccount::server_schedules`](crate::GroupwareAccount::server_schedules)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Scheduling {
    /// The server if it announces `calendar-auto-schedule` (RFC 6638),
    /// else the bridge.
    #[default]
    Auto,
    /// Always the bridge, by the account's SMTP.
    Bridge,
    /// Always the server.
    Server,
}

impl Scheduling {
    /// Whether the server sends them, given what it announced.
    #[must_use]
    pub const fn server_sends(self, auto_schedule: bool) -> bool {
        match self {
            Self::Auto => auto_schedule,
            Self::Bridge => false,
            Self::Server => true,
        }
    }
}

/// Settings of the groupware layer.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GroupwareConfig {
    /// The CalDAV/CardDAV server (base URL, networks, certificates,
    /// allowed origins, limits): calendars, principals, free-busy, and
    /// address books too unless `contacts_server` is set.
    pub server: DavServer,
    /// A second server for address books only (`contacts_url`, e.g.
    /// Fastmail's `carddav.fastmail.com` next to `caldav.fastmail.com`).
    /// It gets the same login. With it, address books are only taken
    /// from it, calendars only from `server`, and each client sends the
    /// login only to its own origin.
    pub contacts_server: Option<DavServer>,
    /// Sync calendars (`calendars = true`).
    pub calendars: bool,
    /// Sync address books (`contacts = true`).
    pub contacts: bool,
    /// Who sends invitations.
    pub scheduling: Scheduling,
    /// How often an account with an open push connection is synced: 60 s.
    pub poll_with_push: Duration,
    /// How often an account with a session but no push connection is
    /// synced: 5 min.
    pub poll_without_push: Duration,
    /// How often discovery runs again (and every collection is compared
    /// whole with the server): once a day.
    pub discovery_interval: Duration,
    /// How long a resource or collection gone from the server is kept
    /// hidden before it is deleted (a move by another client keeps its
    /// id): a day.
    pub orphan_ttl: Duration,
    /// Occurrences expanded to compute `utc_end`; past it the event counts
    /// as open-ended.
    pub max_occurrences: usize,
    /// First wait after an error; doubles up to `max_retry`.
    pub first_retry: Duration,
    /// Longest wait after errors.
    pub max_retry: Duration,
    /// Calendar alerts.
    pub alerts: AlertConfig,
    /// Most sharees kept per account, over all its collections (its own
    /// first): 50 000, about what 200 collections of 256 gave before.
    /// Past it, a collection that does not fit is left
    /// without a sharing mode.
    pub max_account_shares: usize,
    /// Most bytes of sharee names kept per account: 16 MiB (one listing's
    /// limit). A sharee may be 8.7 KB long, so this, not the count, bounds
    /// the memory and the store against long names.
    pub max_account_share_bytes: usize,
    /// Longest a sharing change sent in several `POST`s may take before
    /// it stops between two of them (the store is read again and the
    /// change is reported unfinished): 180 s, so that one more `POST`
    /// (60 s at most) and the sync after it end before a JMAP request's
    /// 300 s.
    pub share_deadline: Duration,
}

impl GroupwareConfig {
    /// The defaults for `server`: calendars and contacts, scheduling
    /// `auto`, sync every 60 s with push and every 5 min without,
    /// discovery once a day.
    #[must_use]
    pub fn new(server: DavServer) -> Self {
        Self {
            server,
            contacts_server: None,
            calendars: true,
            contacts: true,
            scheduling: Scheduling::Auto,
            poll_with_push: Duration::from_mins(1),
            poll_without_push: Duration::from_mins(5),
            discovery_interval: Duration::from_hours(24),
            orphan_ttl: Duration::from_hours(24),
            max_occurrences: 1000,
            first_retry: Duration::from_secs(5),
            max_retry: Duration::from_mins(5),
            alerts: AlertConfig::default(),
            max_account_shares: 50_000,
            max_account_share_bytes: 16 * 1024 * 1024,
            share_deadline: Duration::from_mins(3),
        }
    }

    /// Takes address books from `contacts` instead of `server`.
    #[must_use]
    pub fn with_contacts_server(mut self, contacts: DavServer) -> Self {
        self.contacts_server = Some(contacts);
        self
    }
}
