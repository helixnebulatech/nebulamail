// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sieve scripts (RFC 9661) and the vacation response (RFC 8621 §8) over
//! ManageSieve (RFC 5804)
//!
//! - Each method call opens one ManageSieve session with the account's
//!   login and password (the ones the IMAP layer already holds), lists the
//!   scripts, reads them, and compares the listing with the store
//!   ([`nebula_store::Store::sync_sieve_scripts`]): stable ids (`z{n}`),
//!   state and `/changes` come from there. Up to 3 sessions per account.
//! - Script content stays on the server and is opaque bytes here. Its
//!   blob id is `bz{n}_{hash}` (the first 64 bits of its SHA-256), so a
//!   blob id always means the same bytes: once the script changes, the
//!   old id is not found.
//! - The capability is announced only after a first successful login to
//!   the ManageSieve server (probed when the account starts, retried
//!   while it is down). `mailboxid` is never announced: the mailbox ids
//!   clients know are the bridge's, not the server's.
//! - The vacation response is the script `vacation`, written by the
//!   bridge ([`vacation`]); behaviour as in Stalwart.
//! - ManageSieve has no way to announce changes, so while an account has
//!   a push stream open its scripts are read every minute; what changed
//!   (there or through the bridge) comes out as a `SieveScript` state.

pub(crate) mod script;
pub(crate) mod vacation;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use nebula_http::{AccountContext, BackendError, Blob, BlobBody, MediaType};
use nebula_jmap_proto::session::SieveAccountCapability;
use nebula_jmap_proto::{
    BlobId, MethodError, MethodErrorType, SieveScriptId as JmapScriptId, State, UnsignedInt,
};
use nebula_managesieve::proto::{Capabilities, ResponseCode, ScriptName};
use nebula_managesieve::{Authenticated, Client, Connector, Credentials};
use nebula_store::{AccountId, DataType, ListedSieveScript, SieveScriptId, SieveScriptRow};
use sha2::{Digest as _, Sha256};
use tokio::sync::{
    OwnedRwLockReadGuard, OwnedRwLockWriteGuard, OwnedSemaphorePermit, RwLock, Semaphore, broadcast,
};
use tokio_util::sync::CancellationToken;

use crate::core::backend::Inner;
use crate::core::context::Ctx;
use crate::core::ids::{self, parse_number};
use crate::error::{Bug, CallError};

/// The name of the vacation response's script (as in Stalwart).
pub(crate) const VACATION_NAME: &str = "vacation";

/// Extensions never announced: `mailboxid` (RFC 9042) would take the
/// server's mailbox ids, and JMAP clients only know the bridge's.
const HIDDEN_EXTENSIONS: &[&str] = &["mailboxid"];

/// How often scripts are read while someone is listening.
const DEFAULT_POLL: Duration = Duration::from_mins(1);

/// Events waiting for a slow push stream, which then assumes a change.
const EVENT_BACKLOG: usize = 64;

/// ManageSieve settings of the backend.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SieveSettings {
    /// How to reach and log in to the server.
    pub connector: Connector,
    /// Largest script in bytes (`maxSizeScript`). Bigger ones are refused
    /// with `tooLarge` before reaching the server.
    pub max_script_size: usize,
    /// Most scripts per account (`maxNumberScripts`), if limited.
    pub max_scripts: Option<usize>,
    /// How often the scripts of an account with an open push stream are
    /// read, to notice changes made by other clients. `None`: never.
    pub poll_interval: Option<Duration>,
}

impl SieveSettings {
    /// Settings with the defaults: scripts up to 1 MiB, at most
    /// [`DEFAULT_MAX_SCRIPTS`] scripts.
    #[must_use]
    pub fn new(connector: Connector) -> Self {
        Self {
            connector,
            max_script_size: 1024 * 1024,
            max_scripts: Some(DEFAULT_MAX_SCRIPTS),
            poll_interval: Some(DEFAULT_POLL),
        }
    }

    /// Most scripts a sync reads: [`Self::max_scripts`], or
    /// [`MAX_LISTED_SCRIPTS`] without one. Each one is a GETSCRIPT on
    /// every Sieve method call.
    fn listing_cap(&self) -> usize {
        self.max_scripts.unwrap_or(MAX_LISTED_SCRIPTS)
    }
}

/// Default `maxNumberScripts`.
pub(crate) const DEFAULT_MAX_SCRIPTS: usize = 100;

/// Most scripts read in a sync when there is no `max_scripts`.
const MAX_LISTED_SCRIPTS: usize = 1000;

struct AccountState {
    /// What the server announced at the last login (`None` until the
    /// first one works).
    capabilities: Option<Capabilities>,
    /// The server refused the login: no new login until the next sign-in
    /// to the bridge (guessing with a password that stopped working could
    /// get the account locked).
    refused: bool,
    /// Session slots and the order of syncs and writes.
    locks: Arc<AccountLocks>,
    stop: CancellationToken,
}

/// The locks of one account's ManageSieve work, always taken in this
/// order: a session slot ([`AccountLocks::slot`]), then the order lock
/// through that slot ([`Slot::read`] for a sync, [`Slot::write`] for a
/// write). The types allow no other order: the order lock is only
/// reachable from a slot. A call takes the order lock once (no nested
/// reads: `tokio`'s `RwLock` lets a waiting writer go first, so a second
/// read behind it would wait forever).
pub(crate) struct AccountLocks {
    /// ManageSieve sessions open at once for the account.
    sessions: Arc<Semaphore>,
    /// Syncs read, writes (`SieveScript/set`, `VacationResponse/set`)
    /// write: no listing taken before a write is stored after it.
    order: Arc<RwLock<()>>,
    /// Which listing may be stored (two syncs in parallel).
    listings: Listings,
}

impl AccountLocks {
    fn new() -> Self {
        Self {
            sessions: Arc::new(Semaphore::new(SESSIONS_PER_ACCOUNT)),
            order: Arc::new(RwLock::new(())),
            listings: Listings::default(),
        }
    }

    /// A session slot (waits while all are taken).
    pub(crate) async fn slot(self: &Arc<Self>) -> Option<Slot> {
        let permit = Arc::clone(&self.sessions).acquire_owned().await.ok()?;
        Some(Slot {
            _permit: permit,
            locks: Arc::clone(self),
        })
    }
}

/// One of an account's session slots.
pub(crate) struct Slot {
    _permit: OwnedSemaphorePermit,
    locks: Arc<AccountLocks>,
}

impl Slot {
    /// The order lock for a sync (reads run in parallel).
    pub(crate) async fn read(&self) -> OwnedRwLockReadGuard<()> {
        Arc::clone(&self.locks.order).read_owned().await
    }

    /// The order lock for a write, held until the [`WriteTurn`] drops.
    pub(crate) async fn write(&self) -> WriteTurn {
        WriteTurn {
            _order: Arc::clone(&self.locks.order).write_owned().await,
        }
    }
}

/// The order lock of an account held for a write: syncs inside it go
/// through [`Session::sync_turn`], which doesn't take the lock again.
pub(crate) struct WriteTurn {
    _order: OwnedRwLockWriteGuard<()>,
}

/// Numbers the listings of an account in the order they start, and keeps
/// a listing from being stored after one that started later (two syncs
/// in parallel, with another client changing scripts between them).
#[derive(Default)]
struct Listings {
    started: AtomicU64,
    /// The number of the last listing stored.
    stored: tokio::sync::Mutex<u64>,
}

impl Listings {
    /// The number of a listing about to start (taken before LISTSCRIPTS).
    fn start(&self) -> u64 {
        self.started
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }
}

/// Whether listing number `started` may be stored when the last stored
/// one is `stored`: not if a later one already was.
const fn is_newest(started: u64, stored: u64) -> bool {
    started > stored
}

/// The ManageSieve side of the backend.
pub(crate) struct Service {
    pub(crate) settings: SieveSettings,
    accounts: Mutex<HashMap<AccountId, AccountState>>,
    /// Open push streams per account.
    watchers: Mutex<HashMap<AccountId, usize>>,
    /// The account whose scripts changed.
    changed: broadcast::Sender<AccountId>,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service").finish_non_exhaustive()
    }
}

/// ManageSieve sessions one account may have open at once. The web app
/// sends a few requests in parallel when it opens the settings; with one
/// they queued up past the bridge's limit of concurrent requests per
/// account. Syncs run in parallel, but a listing is never stored after
/// one that started later ([`Listings`]), and writes go one at a time
/// with no sync during them ([`AccountLocks`]).
const SESSIONS_PER_ACCOUNT: usize = 3;

/// First and longest wait between probes of a server that is down.
const PROBE_RETRY: (Duration, Duration) = (Duration::from_secs(30), Duration::from_mins(15));

impl Service {
    pub(crate) fn new(settings: SieveSettings) -> Self {
        Self {
            settings,
            accounts: Mutex::new(HashMap::new()),
            watchers: Mutex::new(HashMap::new()),
            changed: broadcast::channel(EVENT_BACKLOG).0,
        }
    }

    fn with_accounts<T>(&self, work: impl FnOnce(&mut HashMap<AccountId, AccountState>) -> T) -> T {
        work(&mut self.accounts.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The account started: probe the server in the background until a
    /// login works (then the capability is announced) or is refused, and
    /// from then on watch its scripts while someone listens.
    ///
    /// Called after every login, also for an account that is running (with
    /// a new password, perhaps): what the server announced stays, or a
    /// second sign-in would hide filters until the new probe answers.
    pub(crate) fn start(
        self: &Arc<Self>,
        inner: Weak<Inner>,
        account: AccountId,
        credentials: Credentials,
    ) {
        let stop = CancellationToken::new();
        self.with_accounts(|accounts| {
            if let Some(state) = accounts.get_mut(&account) {
                state.stop.cancel();
                state.stop = stop.clone();
                // New credentials: they may work.
                state.refused = false;
            } else {
                accounts.insert(
                    account,
                    AccountState {
                        capabilities: None,
                        refused: false,
                        locks: Arc::new(AccountLocks::new()),
                        stop: stop.clone(),
                    },
                );
            }
        });
        let service = Arc::clone(self);
        tokio::spawn(async move {
            if service.probe(account, &credentials, &stop).await {
                service.watch_scripts(&inner, account, &stop).await;
            }
        });
    }

    /// Tries until a login works (`true`), is refused or the account stops.
    async fn probe(
        &self,
        account: AccountId,
        credentials: &Credentials,
        stop: &CancellationToken,
    ) -> bool {
        let mut wait = PROBE_RETRY.0;
        loop {
            let attempt = tokio::select! {
                () = stop.cancelled() => return false,
                attempt = self.settings.connector.login(credentials) => attempt,
            };
            match attempt {
                Ok(client) => {
                    self.remember(account, client.capabilities());
                    client.logout().await;
                    return true;
                }
                Err(nebula_managesieve::Error::AuthFailed(_)) => {
                    tracing::warn!(
                        account = account.get(),
                        "the ManageSieve server refused the login: no Sieve for this account"
                    );
                    self.refuse(account);
                    return false;
                }
                Err(error) => {
                    tracing::warn!(%error, "ManageSieve server unreachable; retrying later");
                }
            }
            tokio::select! {
                () = stop.cancelled() => return false,
                () = tokio::time::sleep(wait) => {}
            }
            wait = wait.saturating_mul(2).min(PROBE_RETRY.1);
        }
    }

    /// ManageSieve can't announce changes: reads the scripts on a timer
    /// while the account has a push stream open. `Session::sync` tells
    /// the listeners. A refused login ends it: guessing with a password
    /// that stopped working could get the account locked.
    async fn watch_scripts(
        &self,
        inner: &Weak<Inner>,
        account: AccountId,
        stop: &CancellationToken,
    ) {
        let Some(every) = self.settings.poll_interval else {
            return;
        };
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(every) => {}
            }
            if !self.is_watched(account) {
                continue;
            }
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let outcome = tokio::select! {
                () = stop.cancelled() => return,
                outcome = read_scripts(&inner, account) => outcome,
            };
            match outcome {
                Ok(()) => {}
                Err(CallError::Sieve(nebula_managesieve::Error::AuthFailed(_))) => {
                    tracing::warn!(
                        account = account.get(),
                        "the ManageSieve server refused the login: not watching scripts"
                    );
                    return;
                }
                Err(error) => {
                    let error = error.into_method_error();
                    tracing::debug!(error = ?error.kind(), "scripts not read; trying later");
                }
            }
        }
    }

    fn with_watchers<T>(&self, work: impl FnOnce(&mut HashMap<AccountId, usize>) -> T) -> T {
        work(&mut self.watchers.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn is_watched(&self, account: AccountId) -> bool {
        self.with_watchers(|watchers| watchers.contains_key(&account))
    }

    /// Script changes of every account, and a mark that keeps this account
    /// watched for as long as it lives: a push stream of the account.
    pub(crate) fn listen(self: &Arc<Self>, account: AccountId) -> Listening {
        self.with_watchers(|watchers| *watchers.entry(account).or_insert(0) += 1);
        Listening {
            changes: self.changed.subscribe(),
            _watch: Watch {
                service: Arc::clone(self),
                account,
            },
        }
    }

    /// The account's scripts changed. No listeners is no error.
    pub(crate) fn notify(&self, account: AccountId) {
        let _ = self.changed.send(account);
    }

    pub(crate) fn stop(&self, account: AccountId) {
        if let Some(state) = self.with_accounts(|accounts| accounts.remove(&account)) {
            state.stop.cancel();
        }
    }

    fn remember(&self, account: AccountId, capabilities: &Capabilities) {
        self.with_accounts(|accounts| {
            if let Some(state) = accounts.get_mut(&account) {
                state.capabilities = Some(capabilities.clone());
            }
        });
    }

    /// The server refused the account's login: Sieve is off for it (not
    /// announced, no more logins, no more watching) until the next
    /// sign-in to the bridge ([`Self::start`]).
    fn refuse(&self, account: AccountId) {
        self.with_accounts(|accounts| {
            if let Some(state) = accounts.get_mut(&account) {
                state.capabilities = None;
                state.refused = true;
                state.stop.cancel();
            }
        });
    }

    fn capabilities(&self, account: AccountId) -> Option<Capabilities> {
        self.with_accounts(|accounts| accounts.get(&account)?.capabilities.clone())
    }

    /// The account's locks, or why there is no session: not connected, or
    /// its login was refused.
    fn locks(&self, account: AccountId) -> Result<Arc<AccountLocks>, CallError> {
        self.with_accounts(|accounts| match accounts.get(&account) {
            None => Err(unavailable("the account is not connected")),
            Some(state) if state.refused => {
                Err(unavailable("the ManageSieve server refused the login"))
            }
            Some(state) => Ok(Arc::clone(&state.locks)),
        })
    }
}

/// A push stream's view of an account's scripts.
pub(crate) struct Listening {
    pub(crate) changes: broadcast::Receiver<AccountId>,
    _watch: Watch,
}

struct Watch {
    service: Arc<Service>,
    account: AccountId,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.service.with_watchers(|watchers| {
            if let Some(count) = watchers.get_mut(&self.account) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    watchers.remove(&self.account);
                }
            }
        });
    }
}

/// One read of the account's scripts, for its side effect: `sync`
/// compares them with the store and tells who listens.
async fn read_scripts(inner: &Inner, account: AccountId) -> Result<(), CallError> {
    let mut session = Session::open(inner, account).await?;
    let synced = session.sync(inner).await;
    session.close().await;
    synced.map(|_| ())
}

/// The extensions announced to clients.
fn extensions(capabilities: &Capabilities) -> Vec<String> {
    capabilities
        .sieve
        .iter()
        .filter(|e| !HIDDEN_EXTENSIONS.iter().any(|h| e.eq_ignore_ascii_case(h)))
        .cloned()
        .collect()
}

/// The account's `urn:ietf:params:jmap:sieve` and whether it has a
/// vacation response, once its ManageSieve server answered.
pub(crate) fn capability(
    inner: &Inner,
    account: AccountId,
) -> (Option<SieveAccountCapability>, bool) {
    let Some(service) = &inner.sieve else {
        return (None, false);
    };
    let Some(capabilities) = service.capabilities(account) else {
        return (None, false);
    };
    let settings = &service.settings;
    let count = |n: usize| UnsignedInt::saturating(u64::try_from(n).unwrap_or(u64::MAX));
    let sieve = SieveAccountCapability {
        implementation: capabilities
            .implementation
            .clone()
            .unwrap_or_else(|| "ManageSieve".to_owned()),
        max_size_script_name: count(ScriptName::MAX_LEN),
        max_size_script: Some(count(settings.max_script_size)),
        max_number_scripts: settings.max_scripts.map(count),
        max_number_redirects: capabilities.max_redirects.map(UnsignedInt::from),
        sieve_extensions: extensions(&capabilities),
        notification_methods: (!capabilities.notify.is_empty())
            .then(|| capabilities.notify.clone()),
        external_lists: None,
    };
    (Some(sieve), vacation_writable(&capabilities))
}

/// Whether the bridge can keep the vacation response in a `vacation`
/// script there. Stalwart's ManageSieve refuses that name ("reserved",
/// checked against 0.16.27): its own JMAP owns it. Whoever runs Stalwart
/// uses its JMAP directly; through the bridge, Stalwart has scripts but no
/// vacation response.
pub(crate) fn vacation_writable(capabilities: &Capabilities) -> bool {
    capabilities.has_extension("vacation")
        && !capabilities
            .implementation
            .as_deref()
            .is_some_and(|name| name.to_ascii_lowercase().starts_with("stalwart"))
}

/// The first 64 bits of the SHA-256 of a script, in hex.
pub(crate) fn content_hash(content: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(content);
    digest.iter().take(8).fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

pub(crate) fn script_id(id: SieveScriptId) -> Result<JmapScriptId, Bug> {
    format!("z{}", id.get())
        .parse()
        .map_err(|_| Bug("script id"))
}

pub(crate) fn store_script(id: &JmapScriptId) -> Option<SieveScriptId> {
    parse_number(id.as_str(), 'z').and_then(SieveScriptId::from_raw)
}

pub(crate) fn blob_id(row: &SieveScriptRow) -> Result<BlobId, Bug> {
    blob_id_of(row.id, &row.content_hash)
}

pub(crate) fn blob_id_of(id: SieveScriptId, hash: &str) -> Result<BlobId, Bug> {
    format!("bz{}_{hash}", id.get())
        .parse()
        .map_err(|_| Bug("script blob id"))
}

/// `bz{n}_{hash}`: the script and the hash of its content.
pub(crate) fn parse_blob(text: &str) -> Option<(SieveScriptId, String)> {
    let rest = text.strip_prefix("bz")?;
    let (number, hash) = rest.split_once('_')?;
    let id = parse_number(&format!("z{number}"), 'z').and_then(SieveScriptId::from_raw)?;
    let valid = hash.len() == 16 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    valid.then(|| (id, hash.to_owned()))
}

/// A script as the server has it now: its row (with the hash of its
/// content) and name. The content itself is read when needed
/// ([`Session::content`]); a sync keeps one script in memory at a time.
pub(crate) struct Script {
    pub(crate) row: SieveScriptRow,
    pub(crate) name: ScriptName,
}

/// The account's scripts and their state after a sync.
pub(crate) struct Synced {
    pub(crate) scripts: Vec<Script>,
    pub(crate) state: State,
}

impl Synced {
    pub(crate) fn by_id(&self, id: SieveScriptId) -> Option<&Script> {
        self.scripts.iter().find(|s| s.row.id == id)
    }

    pub(crate) fn by_name(&self, name: &str) -> Option<&Script> {
        self.scripts.iter().find(|s| s.row.name == name)
    }
}

fn unavailable(why: &'static str) -> CallError {
    MethodError::new(MethodErrorType::ServerUnavailable)
        .with_description(why)
        .into()
}

/// One logged-in ManageSieve session for a method call, holding the
/// one of the account's session slots.
pub(crate) struct Session {
    pub(crate) client: Client<Authenticated>,
    pub(crate) account: AccountId,
    slot: Slot,
}

/// Whether a ManageSieve error is `NONEXISTENT` (the script is gone).
fn is_nonexistent(error: &nebula_managesieve::Error) -> bool {
    matches!(error.code(), Some(ResponseCode::Nonexistent))
}

/// The content read for `script` if it is still the one synced (same
/// hash); `None` if another client changed it since.
fn same_content(content: Vec<u8>, hash: &str) -> Option<Vec<u8>> {
    (content_hash(&content) == hash).then_some(content)
}

/// Whether a listing of `count` scripts is too long to read (each one is
/// a GETSCRIPT).
const fn too_many_scripts(count: usize, cap: usize) -> bool {
    count > cap
}

impl Session {
    /// Logs in with the account's login and password. A refused login
    /// turns Sieve off for the account until its next sign-in.
    pub(crate) async fn open(inner: &Inner, account: AccountId) -> Result<Self, CallError> {
        let service = inner
            .sieve
            .as_ref()
            .ok_or_else(|| unavailable("no ManageSieve server is configured"))?;
        let slot = service
            .locks(account)?
            .slot()
            .await
            .ok_or_else(|| unavailable("the account is not connected"))?;
        let login = inner
            .smtp_login(account)
            .ok_or_else(|| unavailable("the account is not connected"))?;
        let credentials = Credentials::new(login.login.clone(), login.password.clone());
        let client = match service.settings.connector.login(&credentials).await {
            Ok(client) => client,
            Err(error) => {
                if matches!(error, nebula_managesieve::Error::AuthFailed(_)) {
                    tracing::warn!(
                        account = account.get(),
                        "the ManageSieve server refused the login: Sieve off until the next sign-in"
                    );
                    service.refuse(account);
                }
                return Err(CallError::Sieve(error));
            }
        };
        service.remember(account, client.capabilities());
        Ok(Self {
            client,
            account,
            slot,
        })
    }

    pub(crate) async fn open_ctx(ctx: &Ctx<'_>) -> Result<Self, CallError> {
        Self::open(ctx.inner, ctx.id()).await
    }

    /// The order lock for a write (`SieveScript/set`,
    /// `VacationResponse/set`): syncs during it go through
    /// [`Self::sync_turn`].
    pub(crate) async fn write_turn(&self) -> WriteTurn {
        self.slot.write().await
    }

    /// Lists every script, hashes it and brings the store up to date
    /// (with the order lock for reading).
    pub(crate) async fn sync(&mut self, inner: &Inner) -> Result<Synced, CallError> {
        let _order = self.slot.read().await;
        self.sync_locked(inner).await
    }

    /// [`Self::sync`] inside a write that already holds the order lock.
    pub(crate) async fn sync_turn(
        &mut self,
        inner: &Inner,
        _turn: &WriteTurn,
    ) -> Result<Synced, CallError> {
        self.sync_locked(inner).await
    }

    /// The listing with each script's hash; the contents are dropped as
    /// they are read. A script deleted between LISTSCRIPTS and GETSCRIPT
    /// lists again once; if it happens again, that script is left out.
    async fn listing(&mut self, cap: usize) -> Result<Vec<ListedSieveScript>, CallError> {
        let mut relisted = false;
        loop {
            let listed = self.client.list_scripts().await.map_err(CallError::Sieve)?;
            if too_many_scripts(listed.len(), cap) {
                return Err(unavailable(
                    "too many Sieve scripts on the server for the bridge to read",
                ));
            }
            let mut out = Vec::with_capacity(listed.len());
            let mut vanished = false;
            for script in listed {
                match self.client.get_script(&script.name).await {
                    Ok(content) => out.push(ListedSieveScript {
                        name: script.name.as_str().to_owned(),
                        active: script.active,
                        content_hash: content_hash(&content),
                    }),
                    Err(error) if is_nonexistent(&error) => {
                        if !relisted {
                            vanished = true;
                            break;
                        }
                    }
                    Err(error) => return Err(CallError::Sieve(error)),
                }
            }
            if !vanished {
                return Ok(out);
            }
            relisted = true;
        }
    }

    async fn sync_locked(&mut self, inner: &Inner) -> Result<Synced, CallError> {
        let cap = inner
            .sieve
            .as_ref()
            .map_or(MAX_LISTED_SCRIPTS, |service| service.settings.listing_cap());
        let listings = &Arc::clone(&self.slot.locks).listings;
        let started = listings.start();
        let listing = self.listing(cap).await?;
        let mut stored = listings.stored.lock().await;
        let (rows, seq) = if is_newest(started, *stored) {
            *stored = started;
            let before = inner
                .store
                .state(self.account, DataType::SieveScript)
                .await?;
            let (rows, seq) = inner
                .store
                .sync_sieve_scripts(self.account, listing)
                .await?;
            if seq != before
                && let Some(service) = &inner.sieve
            {
                service.notify(self.account);
            }
            (rows, seq)
        } else {
            // A listing that started later is stored already: it is newer
            // than this one.
            let rows = inner.store.sieve_scripts(self.account).await?;
            let seq = inner
                .store
                .state(self.account, DataType::SieveScript)
                .await?;
            (rows, seq)
        };
        drop(stored);
        let scripts = rows
            .into_iter()
            .filter_map(|row| {
                let name = ScriptName::new(row.name.clone()).ok()?;
                Some(Script { row, name })
            })
            .collect();
        Ok(Synced {
            scripts,
            state: ids::state(seq),
        })
    }

    /// The content of `script` (GETSCRIPT), if it is still the one this
    /// sync saw: `None` if it is gone or another client changed it since.
    pub(crate) async fn content(&mut self, script: &Script) -> Result<Option<Vec<u8>>, CallError> {
        match self.client.get_script(&script.name).await {
            Ok(content) => Ok(same_content(content, &script.row.content_hash)),
            Err(error) if is_nonexistent(&error) => Ok(None),
            Err(error) => Err(CallError::Sieve(error)),
        }
    }

    pub(crate) async fn close(self) {
        self.client.logout().await;
    }
}

/// The content of script blob `bz…`, `None` if the script is gone or has
/// changed since.
pub(crate) async fn blob_content(
    inner: &Inner,
    account: AccountId,
    id: SieveScriptId,
    hash: &str,
) -> Result<Option<Vec<u8>>, CallError> {
    let rows = inner.store.sieve_scripts(account).await?;
    let Some(row) = rows.into_iter().find(|row| row.id == id) else {
        return Ok(None);
    };
    let Ok(name) = ScriptName::new(row.name) else {
        return Ok(None);
    };
    let mut session = Session::open(inner, account).await?;
    let result = session.client.get_script(&name).await;
    session.close().await;
    match result {
        Ok(content) if content_hash(&content) == hash => Ok(Some(content)),
        Ok(_) | Err(nebula_managesieve::Error::Refused(_)) => Ok(None),
        Err(error) => Err(CallError::Sieve(error)),
    }
}

/// A download of a script blob (`application/sieve`).
pub(crate) async fn download(
    inner: &Inner,
    account: &AccountContext,
    id: SieveScriptId,
    hash: &str,
) -> Result<Option<Blob>, BackendError> {
    match blob_content(inner, account.id, id, hash).await {
        Ok(content) => Ok(content.map(|content| Blob {
            media_type: "application/sieve"
                .parse()
                .unwrap_or_else(|_| MediaType::octet_stream()),
            body: BlobBody::Bytes(content.into()),
        })),
        Err(error) => {
            let error = error.into_method_error();
            tracing::warn!(error = ?error.kind(), "script download failed");
            Err(BackendError::Unavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let id = SieveScriptId::from_raw(7).unwrap();
        assert_eq!(script_id(id).unwrap().as_str(), "z7");
        assert_eq!(store_script(&"z7".parse().unwrap()), Some(id));
        assert_eq!(store_script(&"z07".parse().unwrap()), None);
        let hash = content_hash(b"keep;");
        assert_eq!(hash.len(), 16);
        let blob = blob_id_of(id, &hash).unwrap();
        assert_eq!(parse_blob(blob.as_str()), Some((id, hash)));
        for bad in [
            "bz7",
            "bz7_",
            "bz0_0011223344556677",
            "bz7_00112233445566",
            "bz7_0011223344556677a",
            "bz7_00112233445566GG",
            "bu7",
        ] {
            assert_eq!(parse_blob(bad), None, "{bad}");
        }
    }

    fn service() -> Arc<Service> {
        use nebula_managesieve::{SieveServer, Tls};
        use net_guard::{Port, SystemResolver};
        let server = SieveServer::new(
            "127.0.0.1".parse().unwrap(),
            Port::try_from(4190).unwrap(),
            Tls::StartTls,
        );
        let resolver = Arc::new(SystemResolver::new().unwrap());
        let mut settings = SieveSettings::new(Connector::new(server, resolver).unwrap());
        settings.poll_interval = None;
        Arc::new(Service::new(settings))
    }

    fn capabilities() -> Capabilities {
        use nebula_managesieve::proto::Item;
        let line = vec![
            Item::Quoted(b"SIEVE".to_vec()),
            Item::Quoted(b"vacation".to_vec()),
        ];
        Capabilities::parse(&[line]).unwrap()
    }

    /// Every login calls `start`, also for an account that is running. The
    /// capability must survive: the session is read right after a login.
    #[tokio::test]
    async fn a_second_login_keeps_the_capability() {
        let service = service();
        let account = AccountId::from_raw(1).unwrap();
        let credentials = || Credentials::new("alice".to_owned(), "secret".to_owned().into());
        service.start(Weak::new(), account, credentials());
        service.remember(account, &capabilities());
        let locks = service.locks(account).ok().unwrap();
        service.start(Weak::new(), account, credentials());
        assert!(service.capabilities(account).is_some());
        assert!(Arc::ptr_eq(&locks, &service.locks(account).ok().unwrap()));
        service.stop(account);
    }

    fn method_error(error: CallError) -> MethodErrorType {
        error.into_method_error().kind()
    }

    /// A refused login turns Sieve off for the account: not announced, and
    /// no session tries the server again (the connector points at a closed
    /// port, so a real attempt would give another error). The next sign-in
    /// turns it back on.
    #[tokio::test]
    async fn a_refused_login_is_not_retried() {
        let service = service();
        let account = AccountId::from_raw(1).unwrap();
        let credentials = || Credentials::new("alice".to_owned(), "secret".to_owned().into());
        service.start(Weak::new(), account, credentials());
        service.remember(account, &capabilities());
        service.refuse(account);
        assert!(service.capabilities(account).is_none());
        let Err(error) = service.locks(account) else {
            panic!("refused")
        };
        assert_eq!(method_error(error), MethodErrorType::ServerUnavailable);
        service.start(Weak::new(), account, credentials());
        assert!(service.locks(account).is_ok());
        service.stop(account);
    }

    /// Readers and a writer of one account, slot first and then the order
    /// lock: everyone finishes (3 slots, 4 tasks), and a writer waits for
    /// the readers and keeps a sync out until it is done.
    #[tokio::test]
    async fn locks_are_taken_in_order() {
        let locks = Arc::new(AccountLocks::new());
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let locks = Arc::clone(&locks);
            tasks.push(tokio::spawn(async move {
                let slot = locks.slot().await.unwrap();
                let _read = slot.read().await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }));
        }
        let writer = Arc::clone(&locks);
        tasks.push(tokio::spawn(async move {
            let slot = writer.slot().await.unwrap();
            let _turn = slot.write().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }));
        let all = async {
            for task in tasks {
                task.await.unwrap();
            }
        };
        tokio::time::timeout(Duration::from_secs(5), all)
            .await
            .unwrap();

        // A write in progress keeps a sync waiting.
        let slot = locks.slot().await.unwrap();
        let turn = slot.write().await;
        let reader = Arc::clone(&locks);
        let sync = tokio::spawn(async move {
            let slot = reader.slot().await.unwrap();
            let _read = slot.read().await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!sync.is_finished());
        drop(turn);
        tokio::time::timeout(Duration::from_secs(5), sync)
            .await
            .unwrap()
            .unwrap();
    }

    /// A listing that started before the last stored one is not stored.
    #[test]
    fn older_listings_are_not_stored() {
        let listings = Listings::default();
        let a = listings.start();
        let b = listings.start();
        assert!(b > a);
        assert!(is_newest(a, 0));
        // B is stored first: A, older, is not.
        assert!(is_newest(b, 0));
        assert!(!is_newest(a, b));
    }

    /// The listing cap (each script is a GETSCRIPT per method call), and
    /// a script read again is only used if it is the one synced.
    #[test]
    fn listing_cap_and_changed_contents() {
        let mut settings = service().settings.clone();
        assert_eq!(settings.max_scripts, Some(DEFAULT_MAX_SCRIPTS));
        assert_eq!(settings.listing_cap(), DEFAULT_MAX_SCRIPTS);
        settings.max_scripts = None;
        assert_eq!(settings.listing_cap(), MAX_LISTED_SCRIPTS);
        assert!(!too_many_scripts(100, 100));
        assert!(too_many_scripts(101, 100));
        let hash = content_hash(b"keep;");
        assert_eq!(
            same_content(b"keep;".to_vec(), &hash),
            Some(b"keep;".to_vec())
        );
        assert_eq!(same_content(b"discard;".to_vec(), &hash), None);
    }

    #[tokio::test]
    async fn watched_while_a_stream_listens() {
        let service = service();
        let account = AccountId::from_raw(1).unwrap();
        assert!(!service.is_watched(account));
        let one = service.listen(account);
        let mut two = service.listen(account);
        drop(one);
        assert!(service.is_watched(account));
        service.notify(account);
        assert_eq!(two.changes.recv().await.unwrap(), account);
        drop(two);
        assert!(!service.is_watched(account));
    }

    #[test]
    fn mailboxid_is_hidden() {
        use nebula_managesieve::proto::Item;
        let line = vec![
            Item::Quoted(b"SIEVE".to_vec()),
            Item::Quoted(b"fileinto mailboxid vacation".to_vec()),
        ];
        let capabilities = Capabilities::parse(&[line]).unwrap();
        assert_eq!(extensions(&capabilities), ["fileinto", "vacation"]);
    }
}
