// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Refused logins of running accounts, waiting for the bridge to end their
//! sessions.
//!
//! Not an event of the `broadcast` channel: that one drops what a slow
//! listener didn't read, and a refused password that nobody acts on keeps
//! the sessions (and their tokens) alive. This is a set per account, so
//! nothing is lost and a burst of failures of one account is one entry.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use nebula_store::AccountId;
use tokio::sync::Notify;

#[derive(Debug, Default)]
pub(crate) struct AuthFailures {
    pending: Mutex<BTreeSet<AccountId>>,
    notify: Notify,
}

impl AuthFailures {
    fn pending(&self) -> std::sync::MutexGuard<'_, BTreeSet<AccountId>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The server refused the current password of a running account.
    pub(crate) fn report(&self, account: AccountId) {
        self.pending().insert(account);
        // Keeps a permit if nobody is waiting: the next wait returns at once.
        self.notify.notify_one();
    }

    /// The account's sessions ended, or it logged in again: an older
    /// failure must not end the new ones.
    pub(crate) fn clear(&self, account: AccountId) {
        self.pending().remove(&account);
    }

    /// Waits for refused logins and takes them all. Safe to cancel (in a
    /// `select!`): the set is only emptied right before returning.
    pub(crate) async fn wait(&self) -> Vec<AccountId> {
        loop {
            let taken = std::mem::take(&mut *self.pending());
            if !taken.is_empty() {
                return taken.into_iter().collect();
            }
            self.notify.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use std::time::Duration;

    use super::*;

    fn id(n: i64) -> AccountId {
        AccountId::from_raw(n).unwrap()
    }

    #[tokio::test]
    async fn nothing_is_lost() {
        let failures = AuthFailures::default();
        // Reported while nobody waits.
        failures.report(id(1));
        assert_eq!(failures.wait().await, vec![id(1)]);

        for n in 1..=2000 {
            failures.report(id(n));
        }
        failures.report(id(7));
        let all = failures.wait().await;
        assert_eq!(all.len(), 2000);
        assert_eq!(all, (1..=2000).map(id).collect::<Vec<_>>());

        // Cleared before anyone looked: nothing to do.
        failures.report(id(3));
        failures.clear(id(3));
        let waited = tokio::time::timeout(Duration::from_millis(50), failures.wait()).await;
        assert!(waited.is_err());

        // A wait that is cancelled loses nothing.
        let waiting = tokio::time::timeout(Duration::from_millis(10), failures.wait()).await;
        assert!(waiting.is_err());
        failures.report(id(4));
        assert_eq!(failures.wait().await, vec![id(4)]);
    }
}
