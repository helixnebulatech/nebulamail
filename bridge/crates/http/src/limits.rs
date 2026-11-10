// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! In-memory limits: failed logins per login and per IP, and concurrent
//! requests per account.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::ConnectInfo;
use http::{Extensions, HeaderMap};
use nebula_store::{AccountId, TokenHash, UnixTime};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::{ClientIpSource, LoginLimits, RateLimit, ip_bucket, ip_bucket_wide};

/// Keys tracked per map: bounds the memory an attacker can make us use.
/// When full, the keys with the oldest last attempt are forgotten (a
/// batch at a time), never refusing new ones: a full map must not shut
/// the login for everyone. A key being attacked right now can only be
/// pushed out by this many newer keys, i.e. ~100,000 attempts, all
/// through the per-IP and per-/48 windows.
const MAX_KEYS: usize = 100_000;
/// Fraction of [`MAX_KEYS`] forgotten at once when full, so the cost of
/// finding the oldest is shared by many inserts.
const EVICT_DIVISOR: usize = 64;
/// The per-/48 window (IPv6) allows this many times the per-IP limit.
const WIDE_FACTOR: u32 = 16;

/// Recent attempts per key, as a sliding window.
#[derive(Debug)]
struct Window<K> {
    max: u32,
    period: Duration,
    max_keys: usize,
    hits: HashMap<K, VecDeque<UnixTime>>,
}

impl<K: Hash + Eq + Clone> Window<K> {
    fn new(max: u32, period: Duration) -> Self {
        Self {
            max,
            period,
            max_keys: MAX_KEYS,
            hits: HashMap::new(),
        }
    }

    /// Makes room for a new key: drops stale keys, then if still full the
    /// batch with the oldest last attempt.
    fn make_room(&mut self, now: UnixTime) {
        self.prune(now);
        if self.hits.len() < self.max_keys {
            return;
        }
        let evict = (self.max_keys / EVICT_DIVISOR).max(1);
        let mut ages: Vec<(UnixTime, K)> = self
            .hits
            .iter()
            .map(|(key, hits)| {
                let last = hits.back().copied().unwrap_or(UnixTime::from_secs(0));
                (last, key.clone())
            })
            .collect();
        if evict < ages.len() {
            ages.select_nth_unstable_by_key(evict, |(last, _)| *last);
        }
        for (_, key) in ages.into_iter().take(evict) {
            self.hits.remove(&key);
        }
    }

    fn of(limit: RateLimit) -> Self {
        Self::new(limit.max(), limit.period())
    }

    fn since(&self, now: UnixTime) -> UnixTime {
        now.saturating_sub(self.period)
    }

    /// Whether `key` may try again now.
    fn allows(&mut self, key: &K, now: UnixTime) -> bool {
        let since = self.since(now);
        if let Some(hits) = self.hits.get_mut(key) {
            while hits.front().is_some_and(|t| *t <= since) {
                hits.pop_front();
            }
            return hits.len() < usize::try_from(self.max).unwrap_or(usize::MAX);
        }
        if self.hits.len() >= self.max_keys {
            self.make_room(now);
        }
        true
    }

    fn push(&mut self, key: K, now: UnixTime) {
        self.hits.entry(key).or_default().push_back(now);
    }

    fn pop(&mut self, key: &K) {
        if let Some(hits) = self.hits.get_mut(key) {
            hits.pop_back();
            if hits.is_empty() {
                self.hits.remove(key);
            }
        }
    }

    fn clear(&mut self, key: &K) {
        self.hits.remove(key);
    }

    fn prune(&mut self, now: UnixTime) {
        let since = self.since(now);
        self.hits
            .retain(|_, hits| hits.back().is_some_and(|t| *t > since));
    }
}

/// Too many failed logins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limited;

/// Which per-login window an attempt counts in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum LoginBucket {
    /// A browser without a known-device cookie for this login: the shared
    /// window of the login, as strict as ever against guessing.
    Login(String),
    /// A browser that logged in to this login before (its cookie hash):
    /// its own window, which failures from elsewhere don't fill.
    Device(TokenHash, String),
}

impl LoginBucket {
    /// The shared window of `login` (case-insensitive like the store).
    pub(crate) fn login(login: &str) -> Self {
        Self::Login(login.to_ascii_lowercase())
    }

    /// The window of a known browser for `login`.
    pub(crate) fn device(hash: TokenHash, login: &str) -> Self {
        Self::Device(hash, login.to_ascii_lowercase())
    }
}

#[derive(Debug)]
struct LoginWindows {
    logins: Window<LoginBucket>,
    ips: Window<IpAddr>,
    /// Per IPv6 /48 (none for IPv4): one site has 65,536 /64s.
    wide: Window<IpAddr>,
}

/// Counts login attempts. An attempt is counted when it starts (so many
/// parallel tries can't slip through before the first failure is known)
/// and uncounted if it succeeds; if the server was unavailable, it is
/// uncounted for the login only.
#[derive(Debug)]
pub(crate) struct LoginLimiter {
    inner: Mutex<LoginWindows>,
}

/// The key of the per-/48 window, IPv6 only.
fn wide_key(ip: IpAddr) -> Option<IpAddr> {
    match ip_bucket_wide(ip) {
        wide @ IpAddr::V6(_) => Some(wide),
        IpAddr::V4(_) => None,
    }
}

impl LoginLimiter {
    pub(crate) fn new(limits: LoginLimits) -> Self {
        let per_ip = limits.per_ip;
        Self {
            inner: Mutex::new(LoginWindows {
                logins: Window::of(limits.per_login),
                ips: Window::of(per_ip),
                wide: Window::new(per_ip.max().saturating_mul(WIDE_FACTOR), per_ip.period()),
            }),
        }
    }

    fn windows(&self) -> std::sync::MutexGuard<'_, LoginWindows> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts an attempt, or refuses it.
    pub(crate) fn begin(
        &self,
        ip: IpAddr,
        bucket: &LoginBucket,
        now: UnixTime,
    ) -> Result<(), Limited> {
        let mut windows = self.windows();
        let LoginWindows { logins, ips, wide } = &mut *windows;
        let key = ip_bucket(ip);
        let wide_key = wide_key(ip);
        // Check all before counting, so a refused attempt counts nowhere.
        if !logins.allows(bucket, now)
            || !ips.allows(&key, now)
            || wide_key.is_some_and(|w| !wide.allows(&w, now))
        {
            return Err(Limited);
        }
        logins.push(bucket.clone(), now);
        ips.push(key, now);
        if let Some(w) = wide_key {
            wide.push(w, now);
        }
        Ok(())
    }

    /// The attempt worked: forget the window's failures and this attempt.
    pub(crate) fn succeeded(&self, ip: IpAddr, bucket: &LoginBucket) {
        let mut windows = self.windows();
        windows.logins.clear(bucket);
        windows.ips.pop(&ip_bucket(ip));
        if let Some(w) = wide_key(ip) {
            windows.wide.pop(&w);
        }
    }

    /// The attempt didn't reach a verdict (server down): it doesn't count
    /// against the login, so the owner isn't locked out by an outage. It
    /// still counts against the IP, in case the server can be made to look
    /// down (closing the connection on a wrong password, say).
    pub(crate) fn not_counted(&self, bucket: &LoginBucket) {
        self.windows().logins.pop(bucket);
    }

    /// Drops windows with nothing recent.
    pub(crate) fn prune(&self, now: UnixTime) {
        let mut windows = self.windows();
        windows.logins.prune(now);
        windows.ips.prune(now);
        windows.wide.prune(now);
    }
}

/// Failed OAuth client checks allowed per address: a real client never
/// fails, so a few are plenty, and each one costs an argon2 hash.
const CLIENT_AUTH_FAILURES: u32 = 10;
/// The window for [`CLIENT_AUTH_FAILURES`].
const CLIENT_AUTH_PERIOD: Duration = Duration::from_mins(15);

/// Counts failed OAuth client checks (wrong `client_secret`) per address
/// (IPv6: per /64), before argon2 runs. Like logins, an attempt counts when
/// it starts and is uncounted if it succeeds or no hash ran.
#[derive(Debug)]
pub(crate) struct ClientAuthLimiter {
    window: Mutex<Window<IpAddr>>,
}

impl ClientAuthLimiter {
    pub(crate) fn new() -> Self {
        Self {
            window: Mutex::new(Window::new(CLIENT_AUTH_FAILURES, CLIENT_AUTH_PERIOD)),
        }
    }

    fn window(&self) -> std::sync::MutexGuard<'_, Window<IpAddr>> {
        self.window.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a check, or refuses it.
    pub(crate) fn begin(&self, ip: IpAddr, now: UnixTime) -> Result<(), Limited> {
        let mut window = self.window();
        let ip = ip_bucket(ip);
        if !window.allows(&ip, now) {
            return Err(Limited);
        }
        window.push(ip, now);
        Ok(())
    }

    /// The check passed.
    pub(crate) fn succeeded(&self, ip: IpAddr) {
        self.window().pop(&ip_bucket(ip));
    }

    /// No hash ran (all busy).
    pub(crate) fn not_counted(&self, ip: IpAddr) {
        self.window().pop(&ip_bucket(ip));
    }

    pub(crate) fn prune(&self, now: UnixTime) {
        self.window().prune(now);
    }
}

/// The client address for rate limiting.
pub(crate) fn client_ip(
    source: ClientIpSource,
    headers: &HeaderMap,
    extensions: &Extensions,
) -> IpAddr {
    let peer = extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    let forwarded = match source {
        ClientIpSource::Peer => None,
        ClientIpSource::XForwardedFor => headers
            .get_all("x-forwarded-for")
            .iter()
            .next_back()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .and_then(|last| last.trim().parse().ok()),
    };
    // Without an address (tests, unusual transports) every request shares
    // one bucket: stricter, never looser.
    forwarded
        .or(peer)
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// Downloads at once per account. A `bp…` blob keeps a whole message in
/// memory until it is sent, so a client that doesn't read can't hold more.
const DOWNLOADS_PER_ACCOUNT: usize = 4;
/// Downloads at once for everyone (organization mode: many accounts).
const DOWNLOADS: usize = 64;
/// How long a download waits for a slot before a 429: the web app asks
/// for several inline images at once.
pub(crate) const DOWNLOAD_WAIT: Duration = Duration::from_secs(30);

/// A download's slots (its account's and the global one), held until its
/// body is sent or dropped.
#[derive(Debug)]
pub(crate) struct DownloadPermit {
    _account: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

/// Concurrent downloads, per account and in total.
#[derive(Debug)]
pub(crate) struct Downloads {
    per_account: Slots,
    global: Arc<Semaphore>,
}

impl Default for Downloads {
    fn default() -> Self {
        Self::new(DOWNLOADS_PER_ACCOUNT, DOWNLOADS)
    }
}

impl Downloads {
    pub(crate) fn new(per_account: usize, global: usize) -> Self {
        Self {
            per_account: Slots::new(per_account),
            global: Arc::new(Semaphore::new(global)),
        }
    }

    /// Both slots, waiting up to `wait` in all.
    pub(crate) async fn acquire(
        &self,
        account: AccountId,
        wait: Duration,
    ) -> Option<DownloadPermit> {
        let semaphore = self.per_account.semaphore(account);
        let global = Arc::clone(&self.global);
        tokio::time::timeout(wait, async move {
            let account = semaphore.acquire_owned().await.ok()?;
            let global = global.acquire_owned().await.ok()?;
            Some(DownloadPermit {
                _account: account,
                _global: global,
            })
        })
        .await
        .ok()?
    }

    /// Forgets an account that stopped (see [`Slots::remove`]).
    pub(crate) fn remove(&self, account: AccountId) {
        self.per_account.remove(account);
    }
}

/// A fixed number of concurrent slots per account (JMAP's
/// `maxConcurrentRequests`, `maxConcurrentUpload`, push streams).
#[derive(Debug)]
pub(crate) struct Slots {
    per_account: usize,
    semaphores: Mutex<HashMap<AccountId, Arc<Semaphore>>>,
}

impl Slots {
    pub(crate) fn new(per_account: usize) -> Self {
        Self {
            per_account,
            semaphores: Mutex::new(HashMap::new()),
        }
    }

    /// A slot, held until the permit is dropped; `None` if all are taken.
    pub(crate) fn try_acquire(&self, account: AccountId) -> Option<OwnedSemaphorePermit> {
        self.semaphore(account).try_acquire_owned().ok()
    }

    fn semaphore(&self, account: AccountId) -> Arc<Semaphore> {
        let mut map = self
            .semaphores
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(
            map.entry(account)
                .or_insert_with(|| Arc::new(Semaphore::new(self.per_account))),
        )
    }

    /// Forgets an account that stopped. Permits still held keep the old
    /// semaphore alive, so for a moment more than `per_account` may run
    /// (the account just stopped: its requests fail soon anyway).
    pub(crate) fn remove(&self, account: AccountId) {
        self.semaphores
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&account);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn limiter() -> LoginLimiter {
        LoginLimiter::new(LoginLimits {
            per_login: RateLimit::new(3, secs(60)).unwrap(),
            per_ip: RateLimit::new(5, secs(60)).unwrap(),
        })
    }

    const IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    const OTHER_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));

    fn alice() -> LoginBucket {
        LoginBucket::login("alice")
    }

    #[test]
    fn per_login_limit_is_case_insensitive_and_slides() {
        let limiter = limiter();
        let t0 = UnixTime::from_secs(1000);
        for login in ["alice", "Alice", "ALICE"] {
            limiter.begin(IP, &LoginBucket::login(login), t0).unwrap();
        }
        assert_eq!(limiter.begin(OTHER_IP, &alice(), t0), Err(Limited));
        // Another login from the same IP still works.
        limiter.begin(IP, &LoginBucket::login("bob"), t0).unwrap();
        // After the window, alice may try again.
        let later = t0.saturating_add(secs(61));
        limiter.begin(IP, &alice(), later).unwrap();
    }

    #[test]
    fn per_ip_limit() {
        let limiter = limiter();
        let t0 = UnixTime::from_secs(1000);
        for n in 0..5 {
            limiter
                .begin(IP, &LoginBucket::login(&format!("user{n}")), t0)
                .unwrap();
        }
        let someone = LoginBucket::login("someone");
        assert_eq!(limiter.begin(IP, &someone, t0), Err(Limited));
        assert!(limiter.begin(OTHER_IP, &someone, t0).is_ok());
    }

    #[test]
    fn success_and_unavailable_are_not_failures() {
        let limiter = limiter();
        let t0 = UnixTime::from_secs(1000);
        limiter.begin(IP, &alice(), t0).unwrap();
        limiter.begin(IP, &alice(), t0).unwrap();
        limiter.succeeded(IP, &alice());
        for _ in 0..3 {
            limiter.begin(OTHER_IP, &alice(), t0).unwrap();
            limiter.not_counted(&alice());
        }
        limiter.begin(IP, &alice(), t0).unwrap();
        limiter.prune(t0.saturating_add(secs(3600)));
        assert!(limiter.windows().logins.hits.is_empty());
    }

    #[test]
    fn not_counted_keeps_the_ip_attempt() {
        let limiter = limiter();
        let t0 = UnixTime::from_secs(1000);
        for _ in 0..5 {
            limiter.begin(IP, &alice(), t0).unwrap();
            limiter.not_counted(&alice());
        }
        // The login window is untouched, the IP's is full.
        assert_eq!(limiter.begin(IP, &alice(), t0), Err(Limited));
        limiter.begin(OTHER_IP, &alice(), t0).unwrap();
    }

    #[test]
    fn known_device_is_not_locked_out_by_others() {
        let limiter = limiter();
        let t0 = UnixTime::from_secs(1000);
        for _ in 0..3 {
            limiter.begin(OTHER_IP, &alice(), t0).unwrap();
        }
        assert_eq!(limiter.begin(IP, &alice(), t0), Err(Limited));
        // Alice's browser has its own window...
        let device = LoginBucket::device(TokenHash([1; 32]), "Alice");
        limiter.begin(IP, &device, t0).unwrap();
        limiter.begin(IP, &device, t0).unwrap();
        limiter.begin(IP, &device, t0).unwrap();
        // ...as strict for its own failures.
        assert_eq!(limiter.begin(IP, &device, t0), Err(Limited));
        // Another browser's cookie is another window.
        let other = LoginBucket::device(TokenHash([2; 32]), "alice");
        limiter.begin(IP, &other, t0).unwrap();
    }

    #[test]
    fn full_map_evicts_oldest_instead_of_refusing() {
        let mut window: Window<u32> = Window::new(3, secs(3600));
        window.max_keys = 64;
        let t0 = UnixTime::from_secs(1000);
        // The key under attack has a recent attempt; the rest are older.
        for key in 0..63 {
            window.push(key, t0.saturating_add(secs(u64::from(key))));
        }
        let recent = t0.saturating_add(secs(100));
        window.push(1000, recent);
        assert_eq!(window.hits.len(), 64);
        // A new key gets in; the oldest goes.
        assert!(window.allows(&2000, recent));
        window.push(2000, recent);
        assert!(window.hits.len() <= 64);
        assert!(!window.hits.contains_key(&0));
        assert!(window.hits.contains_key(&1000));
        assert!(window.hits.contains_key(&62));
    }

    #[test]
    fn ipv6_48_limit() {
        // Per IP 5: per /48 80.
        let limiter = limiter();
        let t0 = UnixTime::from_secs(1000);
        let from = |n: u16| IpAddr::V6(std::net::Ipv6Addr::new(0x2001, 0xdb8, 1, n, 0, 0, 0, 1));
        for n in 0..80 {
            let login = LoginBucket::login(&format!("user{n}"));
            limiter.begin(from(n), &login, t0).unwrap();
        }
        let login = LoginBucket::login("user99");
        assert_eq!(limiter.begin(from(500), &login, t0), Err(Limited));
        // Another /48 is fine; IPv4 has no such window.
        let elsewhere = IpAddr::V6(std::net::Ipv6Addr::new(0x2001, 0xdb8, 2, 0, 0, 0, 0, 1));
        limiter.begin(elsewhere, &login, t0).unwrap();
    }

    #[test]
    fn slots_per_account() {
        let slots = Slots::new(2);
        let a = AccountId::from_raw(1).unwrap();
        let b = AccountId::from_raw(2).unwrap();
        let first = slots.try_acquire(a).unwrap();
        let _second = slots.try_acquire(a).unwrap();
        assert!(slots.try_acquire(a).is_none());
        assert!(slots.try_acquire(b).is_some());
        drop(first);
        assert!(slots.try_acquire(a).is_some());
    }

    #[tokio::test]
    async fn downloads_per_account_and_in_total() {
        let downloads = Downloads::new(2, 3);
        let a = AccountId::from_raw(1).unwrap();
        let b = AccountId::from_raw(2).unwrap();
        let wait = Duration::from_millis(50);
        let first = downloads.acquire(a, wait).await.unwrap();
        let _second = downloads.acquire(a, wait).await.unwrap();
        assert!(downloads.acquire(a, wait).await.is_none());
        let _third = downloads.acquire(b, wait).await.unwrap();
        // Three in total: b waits for the global slot, then gives up.
        assert!(downloads.acquire(b, wait).await.is_none());
        drop(first);
        assert!(downloads.acquire(b, wait).await.is_some());
    }

    #[tokio::test]
    async fn a_waiting_download_gets_the_freed_slot() {
        let downloads = Arc::new(Downloads::new(1, 64));
        let a = AccountId::from_raw(1).unwrap();
        let first = downloads
            .acquire(a, Duration::from_millis(50))
            .await
            .unwrap();
        let waiting = {
            let downloads = Arc::clone(&downloads);
            tokio::spawn(
                async move { downloads.acquire(a, Duration::from_secs(5)).await.is_some() },
            )
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(first);
        assert!(waiting.await.unwrap());
    }
}
