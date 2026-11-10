// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! One account on the DAV server: its client, data key, what discovery
//! found, its status and the live-sync task.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nebula_crypto::AccountKey;
use nebula_dav::proto::{CalendarUserType, Href, LocationKey};
use nebula_dav::{Credentials, DavClient};
use nebula_store::{AccountId, CollectionId, CollectionKind, DavHref, Recorded, Store, UnixTime};
use tokio::sync::{Notify, broadcast};

use super::live::Live;
use crate::principals::Directory;
use crate::{GroupwareConfig, GroupwareEvent};

/// What all accounts share.
pub(crate) struct Shared {
    pub(crate) store: Store,
    pub(crate) config: GroupwareConfig,
    pub(crate) events: broadcast::Sender<GroupwareEvent>,
    /// Open push connections per account (they make polling faster).
    pub(crate) watchers: Mutex<HashMap<AccountId, usize>>,
    /// Listeners of calendar alerts per account: push connections that
    /// asked for `CalendarAlert` and Web Push subscriptions. A push
    /// connection that did not ask for them is only a watcher: an alert
    /// sent for it would be recorded as sent and dropped by its filter.
    pub(crate) alert_listeners: Mutex<HashMap<AccountId, usize>>,
}

impl Shared {
    /// Whether anyone would see an alert of the account now.
    pub(crate) fn alerts_heard(&self, account: AccountId) -> bool {
        self.alert_listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&account)
            .is_some_and(|count| *count > 0)
    }

    pub(crate) fn watchers(&self, account: AccountId) -> usize {
        self.watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&account)
            .copied()
            .unwrap_or(0)
    }
}

/// Where an account stands with the DAV server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DavStatus {
    /// Discovering (right after login).
    Starting,
    /// Discovered; the last sync round worked.
    Ready,
    /// The server could not be used (down, no DAV for this user, a broken
    /// answer…): retried later. What was synced before stays.
    Failing,
    /// The server refused the login (401): no calendars or contacts for
    /// this account until it logs in again. Not retried.
    Unauthorized,
}

/// What discovery found, as the sync uses it.
#[derive(Debug, Clone)]
pub(crate) struct Discovered {
    pub(crate) calendar_homes: Vec<Href>,
    pub(crate) addressbook_homes: Vec<Href>,
    pub(crate) auto_schedule: bool,
    /// `calendar-user-address-set`, as the server sent it.
    pub(crate) user_addresses: Vec<String>,
    /// The user's principal, its name and kind, and where the others are
    /// (for `Principal/*` and free-busy).
    pub(crate) principal: Href,
    pub(crate) principal_name: Option<String>,
    pub(crate) principal_type: Option<CalendarUserType>,
    pub(crate) principal_collections: Vec<Href>,
    /// `schedule-outbox-URL` (allowed origin), for free-busy.
    pub(crate) schedule_outbox: Option<Href>,
    /// What the server announced for sharing.
    pub(crate) sharing: crate::sharing::SharingSupport,
    /// The same two for address books: from the contacts server with
    /// `contacts_url`, else a copy of the above. Whose an address book is
    /// is decided against the principal of its own server.
    pub(crate) contacts_principal: Href,
    pub(crate) contacts_sharing: crate::sharing::SharingSupport,
    /// Nextcloud's calendar extras the server announced.
    pub(crate) nextcloud: NextcloudSupport,
    pub(crate) at: UnixTime,
}

/// Nextcloud's calendar extras a server offers
/// from its `DAV:` header; all `false` elsewhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NextcloudSupport {
    /// Calendars can be published at a public link
    /// (`oc-calendar-publishing`).
    pub publishing: bool,
    /// Webcal subscriptions can be read and created
    /// (`calendarserver-subscribed` and `nc-calendar-webcal-cache`).
    pub subscriptions: bool,
    /// Deleted calendars and events wait in a trash bin
    /// (`nc-calendar-trashbin`).
    pub trash_bin: bool,
}

impl Discovered {
    /// The user's principal on the server of `kind`'s collections.
    pub(crate) const fn principal_for(&self, kind: CollectionKind) -> &Href {
        match kind {
            CollectionKind::Calendar => &self.principal,
            CollectionKind::AddressBook => &self.contacts_principal,
        }
    }

    /// What the server of `kind`'s collections announced for sharing.
    pub(crate) const fn sharing_for(&self, kind: CollectionKind) -> crate::sharing::SharingSupport {
        match kind {
            CollectionKind::Calendar => self.sharing,
            CollectionKind::AddressBook => self.contacts_sharing,
        }
    }
}

/// Whether an account has calendars and contacts now (for the JMAP
/// session).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Availability {
    /// Calendars are configured and the account has a calendar home.
    pub calendars: bool,
    /// Contacts are configured and the account has an address book home.
    pub contacts: bool,
}

struct State {
    status: DavStatus,
    discovered: Option<Discovered>,
}

/// An account on the CalDAV/CardDAV server: discovery, sync rounds and
/// what they found.
///
/// Get one from [`Groupware::open`](crate::Groupware::open). The sync
/// writes to the store and broadcasts the new states.
pub struct GroupwareAccount {
    pub(crate) id: AccountId,
    pub(crate) shared: Arc<Shared>,
    /// The client of `url`: calendars, principals, free-busy, and
    /// address books unless there is a contacts client.
    pub(crate) client: DavClient,
    /// The client of `contacts_url`, if set: address books only.
    pub(crate) contacts_client: Option<DavClient>,
    pub(crate) key: Arc<AccountKey>,
    state: Mutex<State>,
    /// One sync round at a time.
    pub(crate) sync_lock: tokio::sync::Mutex<()>,
    /// One change of a resource (read, patch, PUT) at a time: see
    /// [`Self::write_turn`].
    write_lock: tokio::sync::Mutex<()>,
    /// One change of sharing (read the sharees or ACL, write them back) at
    /// a time: see [`Self::share_turn`].
    share_lock: tokio::sync::Mutex<()>,
    /// Wakes the live task: a kick, or the number of watchers changed.
    pub(crate) wake: Notify,
    /// Wakes the alert scheduler (an event source opened).
    pub(crate) alerts_wake: Notify,
    kicked: AtomicBool,
    live: Mutex<Option<Live>>,
    /// The other principals seen (listing and searches), for a while.
    pub(crate) directory: Directory,
    /// Members asked for in a multiget that did not come back (no answer,
    /// or a status that may change): rounds in a row, by collection. Only
    /// this round's are kept, so it is never more than what was asked.
    pub(crate) unresolved: Mutex<HashMap<(CollectionId, DavHref), u8>>,
    /// Homes whose whole listing was too big: listed split (sharing per
    /// collection) until the next discovery.
    pub(crate) split_homes: Mutex<HashSet<LocationKey>>,
}

impl std::fmt::Debug for GroupwareAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupwareAccount")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl GroupwareAccount {
    pub(crate) fn new(
        id: AccountId,
        shared: Arc<Shared>,
        client: DavClient,
        contacts_client: Option<DavClient>,
        key: AccountKey,
    ) -> Self {
        Self {
            id,
            shared,
            client,
            contacts_client,
            key: Arc::new(key),
            state: Mutex::new(State {
                status: DavStatus::Starting,
                discovered: None,
            }),
            sync_lock: tokio::sync::Mutex::new(()),
            write_lock: tokio::sync::Mutex::new(()),
            share_lock: tokio::sync::Mutex::new(()),
            wake: Notify::new(),
            alerts_wake: Notify::new(),
            kicked: AtomicBool::new(false),
            live: Mutex::new(None),
            directory: Directory::default(),
            unresolved: Mutex::new(HashMap::new()),
            split_homes: Mutex::new(HashSet::new()),
        }
    }

    /// The account id.
    #[must_use]
    pub fn id(&self) -> AccountId {
        self.id
    }

    /// Waits for this account's turn to change a resource; the turn lasts
    /// while the guard lives. The bridge's own read-patch-PUT steps run one
    /// at a time, so two requests at once never PUT with the same etag:
    /// some servers don't check `If-Match` atomically and take both, and
    /// one change is lost (Nextcloud 34 on PostgreSQL, seen 2026-10-30: two
    /// PUTs with the same etag both got 204 in 8 of 10 tries).
    pub async fn write_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().await
    }

    /// Waits for this account's turn to change who a collection is
    /// shared with: each change reads the ACL (or the sharees) and writes
    /// it back whole, so two at once would undo each other (a revocation
    /// lost). Separate from [`Self::write_turn`]; a sync round inside it
    /// takes `sync_lock`, never this one.
    pub(crate) async fn share_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.share_lock.lock().await
    }

    /// Where the account stands with the DAV server.
    #[must_use]
    pub fn status(&self) -> DavStatus {
        self.lock_state().status
    }

    /// Whether it has calendars and contacts now: discovered, not refused,
    /// and with a home of each kind that is configured.
    #[must_use]
    pub fn availability(&self) -> Availability {
        let state = self.lock_state();
        let config = &self.shared.config;
        match (&state.discovered, state.status) {
            (_, DavStatus::Unauthorized) | (None, _) => Availability::default(),
            (Some(found), _) => Availability {
                calendars: config.calendars && !found.calendar_homes.is_empty(),
                contacts: config.contacts && !found.addressbook_homes.is_empty(),
            },
        }
    }

    /// Whether the server sends invitations itself (`scheduling` and what
    /// the server announced), once discovered.
    #[must_use]
    pub fn server_schedules(&self) -> Option<bool> {
        let state = self.lock_state();
        let found = state.discovered.as_ref()?;
        Some(
            self.shared
                .config
                .scheduling
                .server_sends(found.auto_schedule),
        )
    }

    /// Asks for a sync round now (after the bridge's own writes, steps
    /// 4.3–4.5), instead of at the next poll.
    pub fn kick(&self) {
        self.kicked.store(true, Ordering::Release);
        self.wake.notify_one();
    }

    pub(crate) fn take_kick(&self) -> bool {
        self.kicked.swap(false, Ordering::AcqRel)
    }

    /// How long to wait between rounds now.
    pub(crate) fn poll_interval(&self) -> Duration {
        let config = &self.shared.config;
        if self.shared.watchers(self.id) > 0 {
            config.poll_with_push
        } else {
            config.poll_without_push
        }
    }

    /// The client for the collections of `kind`: each one sends the
    /// login only to its own configured origin.
    pub(crate) fn client_for(&self, kind: CollectionKind) -> &DavClient {
        match (kind, &self.contacts_client) {
            (CollectionKind::AddressBook, Some(contacts)) => contacts,
            _ => &self.client,
        }
    }

    pub(crate) fn store(&self) -> &Store {
        &self.shared.store
    }

    pub(crate) fn config(&self) -> &GroupwareConfig {
        &self.shared.config
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_status(&self, status: DavStatus) {
        self.lock_state().status = status;
    }

    pub(crate) fn discovered(&self) -> Option<Discovered> {
        self.lock_state().discovered.clone()
    }

    /// Nextcloud's calendar extras of the account's server (none before
    /// discovery).
    #[must_use]
    pub fn nextcloud_support(&self) -> NextcloudSupport {
        self.lock_state()
            .discovered
            .as_ref()
            .map(|found| found.nextcloud)
            .unwrap_or_default()
    }

    pub(crate) fn set_discovered(&self, found: Discovered) {
        self.lock_state().discovered = Some(found);
    }

    /// Broadcasts new states, if anything changed. Nobody listening is fine.
    pub(crate) fn emit(&self, recorded: &Recorded) {
        if let Some(event) = GroupwareEvent::changed(self.id, recorded) {
            let _ = self.shared.events.send(event);
        }
    }

    pub(crate) fn emit_unauthorized(&self) {
        let _ = self
            .shared
            .events
            .send(GroupwareEvent::Unauthorized(self.id));
    }

    /// Sends this login from the next request on, to both servers (a new
    /// password after logging in again), or none (`None`: closing).
    pub(crate) fn set_credentials(&self, credentials: Option<&Credentials>) {
        self.client.set_credentials(credentials.cloned());
        if let Some(contacts) = &self.contacts_client {
            contacts.set_credentials(credentials.cloned());
        }
    }

    pub(crate) fn set_live(&self, live: Option<Live>) -> Option<Live> {
        let mut slot = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::replace(&mut *slot, live)
    }
}
