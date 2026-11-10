// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Raw messages kept in memory, bounded by total bytes (the
//! body is never stored on disk).

use std::sync::Arc;

use nebula_store::{AccountId, EmailId};
use quick_cache::Weighter;
use quick_cache::sync::Cache;

/// Weight = size in bytes (at least 1: weight 0 is never evicted).
#[derive(Debug, Clone, Copy, Default)]
struct Bytes;

impl Weighter<(AccountId, EmailId), Arc<[u8]>> for Bytes {
    fn weight(&self, _key: &(AccountId, EmailId), value: &Arc<[u8]>) -> u64 {
        u64::try_from(value.len()).unwrap_or(u64::MAX).max(1)
    }
}

pub(crate) struct RawCache {
    cache: Cache<(AccountId, EmailId), Arc<[u8]>, Bytes>,
    max_item: u64,
}

impl std::fmt::Debug for RawCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawCache")
            .field("items", &self.cache.len())
            .field("bytes", &self.cache.weight())
            .field("max_item", &self.max_item)
            .finish()
    }
}

/// Typical message size, to size the cache's index.
const AVERAGE_MESSAGE: u64 = 64 * 1024;

impl RawCache {
    pub(crate) fn new(total_bytes: u64, max_item: u64) -> Self {
        let items = usize::try_from((total_bytes / AVERAGE_MESSAGE).max(16)).unwrap_or(16);
        Self {
            cache: Cache::with_weighter(items, total_bytes.max(1), Bytes),
            max_item,
        }
    }

    pub(crate) fn get(&self, account: AccountId, email: EmailId) -> Option<Arc<[u8]>> {
        self.cache.get(&(account, email))
    }

    pub(crate) fn insert(&self, account: AccountId, email: EmailId, raw: Arc<[u8]>) {
        if u64::try_from(raw.len()).is_ok_and(|len| len <= self.max_item) {
            self.cache.insert((account, email), raw);
        }
    }

    pub(crate) fn forget(&self, account: AccountId, email: EmailId) {
        self.cache.remove(&(account, email));
    }

    /// Drops every message of an account (when its sessions end).
    pub(crate) fn forget_account(&self, account: AccountId) {
        self.cache.retain(|(owner, _), _| *owner != account);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: i64) -> (AccountId, EmailId) {
        (
            AccountId::from_raw(1).unwrap(),
            EmailId::from_raw(n).unwrap(),
        )
    }

    #[test]
    fn bounded_by_bytes() {
        let cache = RawCache::new(1000, 600);
        let (account, one) = ids(1);
        cache.insert(account, one, Arc::from(vec![0_u8; 700]));
        assert!(
            cache.get(account, one).is_none(),
            "bigger than an item may be"
        );
        cache.insert(account, one, Arc::from(vec![0_u8; 500]));
        assert!(cache.get(account, one).is_some());
        for n in 2..10 {
            cache.insert(account, ids(n).1, Arc::from(vec![0_u8; 500]));
        }
        assert!(cache.cache.weight() <= 1000);
        cache.forget_account(account);
        assert_eq!(cache.cache.len(), 0);
    }
}
