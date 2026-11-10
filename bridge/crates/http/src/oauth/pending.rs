// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Authorization requests waiting for the user to log in.
//!
//! The validated request parameters stay here, on the server: the login
//! form only carries a random id, so nothing in the form can change the
//! client, redirect URI or PKCE challenge. Each record is also bound to the
//! browser's CSRF cookie.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use nebula_crypto::{CodeChallenge, Token, TokenHash};
use nebula_store::UnixTime;

use crate::config::{ClientId, Lifetimes, RedirectUri, ip_bucket, ip_bucket_wide};
use crate::oauth::scope::Scope;

/// Records kept at most (an attacker can't grow memory without bound).
/// When full, the oldest record of the network with the most goes first.
const MAX_PENDING: usize = 10_000;
/// Records per address (IPv6: per /64). Past it, that address's oldest
/// form goes: a busy NAT only shortens the life of its own forms.
const MAX_PER_BUCKET: usize = 32;
/// Records per IPv6 /48 (65,536 /64s), past which its oldest form goes.
const MAX_PER_WIDE: usize = MAX_PER_BUCKET * 16;

/// A validated authorization request.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    pub(crate) client_id: ClientId,
    pub(crate) redirect_uri: RedirectUri,
    /// The client's `state`, echoed back; RECOMMENDED, not required
    /// (RFC 6749 §4.1.1).
    pub(crate) state: Option<String>,
    pub(crate) code_challenge: CodeChallenge,
    pub(crate) scope: Scope,
    /// Hash of the CSRF cookie of the browser that opened the form.
    pub(crate) csrf: TokenHash,
    expires_at: UnixTime,
}

impl Pending {
    pub(crate) fn new(
        client_id: ClientId,
        redirect_uri: RedirectUri,
        state: Option<String>,
        code_challenge: CodeChallenge,
        scope: Scope,
        csrf: TokenHash,
        now: UnixTime,
    ) -> Self {
        Self {
            client_id,
            redirect_uri,
            state,
            code_challenge,
            scope,
            csrf,
            expires_at: now.saturating_add(Lifetimes::LOGIN_FORM),
        }
    }
}

/// No id could be made for the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Full;

type Id = [u8; 32];

#[derive(Debug)]
struct Entry {
    pending: Pending,
    bucket: IpAddr,
    wide: IpAddr,
    seq: u64,
}

/// Every record, also by age (all live [`Lifetimes::LOGIN_FORM`], so the
/// oldest is the first to expire) and by network.
#[derive(Debug, Default)]
struct Records {
    map: HashMap<Id, Entry>,
    next: u64,
    order: BTreeMap<u64, Id>,
    per_bucket: HashMap<IpAddr, BTreeMap<u64, Id>>,
    per_wide: HashMap<IpAddr, BTreeMap<u64, Id>>,
    /// (records, network) of every `per_wide` group: the largest is last.
    by_size: BTreeSet<(usize, IpAddr)>,
}

impl Records {
    fn remove(&mut self, id: &Id) -> Option<Entry> {
        let entry = self.map.remove(id)?;
        self.order.remove(&entry.seq);
        if let Some(group) = self.per_bucket.get_mut(&entry.bucket) {
            group.remove(&entry.seq);
            if group.is_empty() {
                self.per_bucket.remove(&entry.bucket);
            }
        }
        if let Some(group) = self.per_wide.get_mut(&entry.wide) {
            self.by_size.remove(&(group.len(), entry.wide));
            group.remove(&entry.seq);
            if group.is_empty() {
                self.per_wide.remove(&entry.wide);
            } else {
                self.by_size.insert((group.len(), entry.wide));
            }
        }
        Some(entry)
    }

    fn insert(&mut self, id: Id, pending: Pending, bucket: IpAddr, wide: IpAddr) {
        let seq = self.next;
        self.next = self.next.saturating_add(1);
        self.order.insert(seq, id);
        self.per_bucket.entry(bucket).or_default().insert(seq, id);
        let group = self.per_wide.entry(wide).or_default();
        self.by_size.remove(&(group.len(), wide));
        group.insert(seq, id);
        self.by_size.insert((group.len(), wide));
        self.map.insert(
            id,
            Entry {
                pending,
                bucket,
                wide,
                seq,
            },
        );
    }

    /// The oldest record of a group.
    fn oldest(group: Option<&BTreeMap<u64, Id>>) -> Option<Id> {
        group.and_then(|group| group.values().next().copied())
    }

    fn remove_oldest(&mut self, group: Option<Id>) {
        if let Some(id) = group {
            self.remove(&id);
        }
    }

    fn prune(&mut self, now: UnixTime) {
        while let Some(id) = self.order.values().next().copied() {
            if self
                .map
                .get(&id)
                .is_some_and(|e| e.pending.expires_at > now)
            {
                break;
            }
            self.remove(&id);
        }
    }
}

/// Pending requests by the hash of their id.
#[derive(Debug)]
pub(crate) struct PendingAuthorizations {
    records: Mutex<Records>,
    max: usize,
}

impl Default for PendingAuthorizations {
    fn default() -> Self {
        Self::with_max(MAX_PENDING)
    }
}

impl PendingAuthorizations {
    fn with_max(max: usize) -> Self {
        Self {
            records: Mutex::new(Records::default()),
            max,
        }
    }

    fn records(&self) -> std::sync::MutexGuard<'_, Records> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores a request from the client address `ip`; returns its id for
    /// the form. Never refused for lack of room: an older form goes, first
    /// from the same address, then from the same /48, then from the /48
    /// with the most forms.
    pub(crate) fn insert(
        &self,
        pending: Pending,
        ip: IpAddr,
        now: UnixTime,
    ) -> Result<Token, Full> {
        let id = Token::generate().map_err(|_| Full)?;
        let bucket = ip_bucket(ip);
        let wide = ip_bucket_wide(ip);
        let mut records = self.records();
        records.prune(now);
        if records.per_bucket.get(&bucket).map_or(0, BTreeMap::len) >= MAX_PER_BUCKET {
            let oldest = Records::oldest(records.per_bucket.get(&bucket));
            records.remove_oldest(oldest);
        } else if records.per_wide.get(&wide).map_or(0, BTreeMap::len) >= MAX_PER_WIDE {
            let oldest = Records::oldest(records.per_wide.get(&wide));
            records.remove_oldest(oldest);
        }
        if records.map.len() >= self.max {
            let oldest = match records.by_size.last() {
                // A network with more than one address's share pays first.
                Some(&(size, network)) if size > MAX_PER_BUCKET => {
                    Records::oldest(records.per_wide.get(&network))
                }
                _ => records.order.values().next().copied(),
            };
            records.remove_oldest(oldest);
        }
        records.insert(*id.hash().as_bytes(), pending, bucket, wide);
        Ok(id)
    }

    /// The request with this id, if it hasn't expired.
    pub(crate) fn get(&self, id: &Token, now: UnixTime) -> Option<Pending> {
        self.records()
            .map
            .get(id.hash().as_bytes())
            .filter(|e| e.pending.expires_at > now)
            .map(|e| e.pending.clone())
    }

    /// Removes a request (used or cancelled): a form works only once.
    pub(crate) fn remove(&self, id: &Token) {
        self.records().remove(id.hash().as_bytes());
    }

    /// Drops expired requests.
    pub(crate) fn prune(&self, now: UnixTime) {
        self.records().prune(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(now: UnixTime) -> Pending {
        Pending::new(
            "web".parse().unwrap(),
            RedirectUri::parse("https://mail.example.com/auth/callback").unwrap(),
            Some("state".to_owned()),
            CodeChallenge::parse("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM").unwrap(),
            Scope::granted(None),
            Token::generate().unwrap().hash(),
            now,
        )
    }

    #[test]
    fn expire_and_single_use() {
        let store = PendingAuthorizations::default();
        let now = UnixTime::from_secs(1000);
        let id = store.insert(pending(now), IP, now).unwrap();
        assert!(store.get(&id, now).is_some());
        assert!(store.get(&Token::generate().unwrap(), now).is_none());
        let late = now.saturating_add(Lifetimes::LOGIN_FORM);
        assert!(store.get(&id, late).is_none());
        store.remove(&id);
        assert!(store.get(&id, now).is_none());
        store.insert(pending(now), IP, now).unwrap();
        store.prune(late);
        assert!(store.records().map.is_empty());
    }

    const IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));

    fn v4(n: u32) -> IpAddr {
        IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000_u32 + n))
    }

    /// An address in its own /64 of the /48 `net`.
    fn v6(net: u16, n: u16) -> IpAddr {
        IpAddr::V6(std::net::Ipv6Addr::new(0x2001, 0xdb8, net, n, 0, 0, 0, 1))
    }

    fn live(store: &PendingAuthorizations, ids: &[Token], now: UnixTime) -> usize {
        ids.iter().filter(|id| store.get(id, now).is_some()).count()
    }

    #[test]
    fn per_bucket_quota_evicts_its_own_oldest() {
        let store = PendingAuthorizations::default();
        let now = UnixTime::from_secs(1000);
        let other = store.insert(pending(now), v4(9), now).unwrap();
        let ids: Vec<Token> = (0..40)
            .map(|_| store.insert(pending(now), IP, now).unwrap())
            .collect();
        // The address keeps its newest 32; the first ones went.
        assert_eq!(live(&store, &ids, now), MAX_PER_BUCKET);
        assert!(store.get(&ids[0], now).is_none());
        assert!(store.get(&ids[39], now).is_some());
        // Nobody else lost anything.
        assert!(store.get(&other, now).is_some());
    }

    #[test]
    fn per_48_quota() {
        let store = PendingAuthorizations::default();
        let now = UnixTime::from_secs(1000);
        let other = store.insert(pending(now), v6(2, 0), now).unwrap();
        let ids: Vec<Token> = (0..600)
            .map(|n| store.insert(pending(now), v6(1, n), now).unwrap())
            .collect();
        assert_eq!(live(&store, &ids, now), MAX_PER_WIDE);
        assert!(store.get(&ids[0], now).is_none());
        assert!(store.get(&other, now).is_some());
    }

    #[test]
    fn full_store_evicts_from_the_largest_network_first() {
        let store = PendingAuthorizations::with_max(100);
        let now = UnixTime::from_secs(1000);
        // The oldest records are from single IPv4 addresses.
        let single: Vec<Token> = (0..40)
            .map(|n| store.insert(pending(now), v4(n), now).unwrap())
            .collect();
        let wide: Vec<Token> = (0..60)
            .map(|n| store.insert(pending(now), v6(1, n), now).unwrap())
            .collect();
        let newer = store.insert(pending(now), v4(500), now).unwrap();
        // The /48 lost its oldest; the old IPv4 forms are all there.
        assert!(store.get(&newer, now).is_some());
        assert_eq!(live(&store, &single, now), 40);
        assert!(store.get(&wide[0], now).is_none());
        assert_eq!(live(&store, &wide, now), 59);
        assert_eq!(store.records().map.len(), 100);
    }

    #[test]
    fn full_store_of_small_networks_evicts_the_oldest() {
        let store = PendingAuthorizations::with_max(10);
        let now = UnixTime::from_secs(1000);
        let ids: Vec<Token> = (0..10)
            .map(|n| store.insert(pending(now), v4(n), now).unwrap())
            .collect();
        store.insert(pending(now), v4(99), now).unwrap();
        assert!(store.get(&ids[0], now).is_none());
        assert_eq!(live(&store, &ids, now), 9);
    }

    #[test]
    fn counts_follow_remove_and_prune() {
        let store = PendingAuthorizations::default();
        let t0 = UnixTime::from_secs(1000);
        let first = store.insert(pending(t0), v6(1, 1), t0).unwrap();
        let t1 = t0.saturating_add(std::time::Duration::from_secs(60));
        let second = store.insert(pending(t1), v6(1, 1), t1).unwrap();
        store.insert(pending(t1), IP, t1).unwrap();
        store.remove(&second);
        {
            let records = store.records();
            assert_eq!(records.per_bucket[&ip_bucket(v6(1, 1))].len(), 1);
            assert_eq!(records.per_wide[&ip_bucket_wide(v6(1, 1))].len(), 1);
            assert_eq!(records.by_size.len(), 2);
        }
        // The first expires before the IPv4 one.
        let late = t0.saturating_add(Lifetimes::LOGIN_FORM);
        store.prune(late);
        assert!(store.get(&first, late).is_none());
        let records = store.records();
        assert_eq!(records.map.len(), 1);
        assert_eq!(records.order.len(), 1);
        assert!(!records.per_wide.contains_key(&ip_bucket_wide(v6(1, 1))));
        assert_eq!(records.by_size.len(), 1);
    }
}
