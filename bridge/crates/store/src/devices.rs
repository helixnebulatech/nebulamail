// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Known browsers: a browser that logged in to an
//! account keeps a random cookie, and only its SHA-256 is stored here. The
//! login limits give a known browser its own window, so failures from
//! elsewhere can't lock the owner out.

use diesel::{ExpressionMethods as _, QueryDsl as _};
use diesel_async::RunQueryDsl as _;

use crate::schema::{accounts, login_devices};
use crate::{AccountId, Store, StoreError, TokenHash, UnixTime};

/// Browsers remembered per account; a new one past this forgets the one
/// that logged in longest ago.
pub const MAX_DEVICES_PER_ACCOUNT: i64 = 16;

impl Store {
    /// Remembers (or renews) a browser that just logged in to `account`,
    /// until `expires_at`.
    ///
    /// # Errors
    /// On database errors.
    pub async fn remember_device(
        &self,
        account: AccountId,
        hash: TokenHash,
        now: UnixTime,
        expires_at: UnixTime,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn().await?;
        conn.immediate_transaction::<_, StoreError, _>(async |conn| {
            diesel::insert_into(login_devices::table)
                .values((
                    login_devices::token_hash.eq(&hash.0[..]),
                    login_devices::account_id.eq(account.get()),
                    login_devices::last_login_at.eq(now.secs()),
                    login_devices::expires_at.eq(expires_at.secs()),
                ))
                .on_conflict((login_devices::token_hash, login_devices::account_id))
                .do_update()
                .set((
                    login_devices::last_login_at.eq(now.secs()),
                    login_devices::expires_at.eq(expires_at.secs()),
                ))
                .execute(conn)
                .await?;
            let keep = login_devices::table
                .filter(login_devices::account_id.eq(account.get()))
                .order((
                    login_devices::last_login_at.desc(),
                    login_devices::token_hash.desc(),
                ))
                .limit(MAX_DEVICES_PER_ACCOUNT)
                .select(login_devices::token_hash)
                .load::<Vec<u8>>(conn)
                .await?;
            diesel::delete(
                login_devices::table
                    .filter(login_devices::account_id.eq(account.get()))
                    .filter(login_devices::token_hash.ne_all(keep)),
            )
            .execute(conn)
            .await?;
            Ok(())
        })
        .await
    }

    /// Whether the browser with this cookie hash logged in to the account
    /// `login` (case-insensitive, like accounts) and is still remembered.
    ///
    /// # Errors
    /// On database errors.
    pub async fn is_known_device(
        &self,
        hash: TokenHash,
        login: &str,
        now: UnixTime,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn().await?;
        let account = accounts::table
            .filter(accounts::login.eq(login))
            .select(accounts::id);
        let count: i64 = login_devices::table
            .filter(login_devices::token_hash.eq(&hash.0[..]))
            .filter(login_devices::account_id.eq_any(account))
            .filter(login_devices::expires_at.gt(now.secs()))
            .count()
            .get_result(&mut conn)
            .await?;
        Ok(count > 0)
    }

    /// Forgets browsers whose cookie expired.
    ///
    /// # Errors
    /// On database errors.
    pub async fn cleanup_devices(&self, now: UnixTime) -> Result<(), StoreError> {
        let mut conn = self.conn().await?;
        diesel::delete(login_devices::table.filter(login_devices::expires_at.le(now.secs())))
            .execute(&mut conn)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::NewAccount;
    use crate::account::tests::alice;
    use crate::db::test_support::store;

    fn hash(n: u8) -> TokenHash {
        TokenHash([n; 32])
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn devices_are_per_account_capped_and_expire() {
        let (store, _dir) = store().await;
        let alice = alice(&store).await.id;
        let bob = store
            .get_or_create_account(
                NewAccount {
                    login: "bob",
                    email: "[EMAIL]",
                    wrapped_dek: b"dek-2",
                },
                UnixTime::from_secs(1000),
            )
            .await
            .unwrap()
            .id;
        let t0 = UnixTime::from_secs(10_000);
        let end = t0.saturating_add(Duration::from_hours(24));
        store
            .remember_device(alice, hash(1), t0, end)
            .await
            .unwrap();
        assert!(store.is_known_device(hash(1), "alice", t0).await.unwrap());
        assert!(store.is_known_device(hash(1), "ALICE", t0).await.unwrap());
        // Not another account's cookie, nor an unknown one.
        assert!(!store.is_known_device(hash(1), "bob", t0).await.unwrap());
        assert!(!store.is_known_device(hash(2), "alice", t0).await.unwrap());
        assert!(!store.is_known_device(hash(1), "nobody", t0).await.unwrap());
        // The same browser logs in to bob too: alice keeps it.
        store.remember_device(bob, hash(1), t0, end).await.unwrap();
        assert!(store.is_known_device(hash(1), "alice", t0).await.unwrap());
        assert!(store.is_known_device(hash(1), "bob", t0).await.unwrap());

        // 16 per account: the 17th forgets the oldest login.
        for n in 2..=17 {
            let at = t0.saturating_add(Duration::from_secs(u64::from(n)));
            store
                .remember_device(alice, hash(n), at, end)
                .await
                .unwrap();
        }
        assert!(!store.is_known_device(hash(1), "alice", t0).await.unwrap());
        assert!(store.is_known_device(hash(2), "alice", t0).await.unwrap());
        assert!(store.is_known_device(hash(17), "alice", t0).await.unwrap());
        assert!(store.is_known_device(hash(1), "bob", t0).await.unwrap());

        // Expired: not known, then gone.
        assert!(!store.is_known_device(hash(2), "alice", end).await.unwrap());
        store.cleanup_devices(end).await.unwrap();
        assert!(!store.is_known_device(hash(17), "alice", t0).await.unwrap());
    }
}
