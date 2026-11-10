// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, Queryable, Selectable,
    SelectableHelper as _,
};
use diesel_async::RunQueryDsl as _;

use crate::schema::{account_secrets, accounts};
use crate::{AccountId, Store, StoreError, UnixTime};

/// A mail account: one IMAP login on the configured server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// Row id.
    pub id: AccountId,
    /// IMAP login, as first accepted by the mail server (later logins with
    /// other case find this account and do not change it).
    pub login: String,
    /// Address of the default identity.
    pub email: String,
    /// The account's data key, wrapped with the master key (`nebula-crypto`).
    pub wrapped_dek: Vec<u8>,
    /// When the account was first seen.
    pub created_at: UnixTime,
    /// Last successful login.
    pub last_login_at: UnixTime,
}

/// What's needed to create an account.
#[derive(Debug, Clone, Copy)]
pub struct NewAccount<'a> {
    /// IMAP login.
    pub login: &'a str,
    /// Address of the default identity.
    pub email: &'a str,
    /// A freshly generated data key, wrapped with the master key.
    pub wrapped_dek: &'a [u8],
}

/// An account's wrapped data key, with what binds it (the login).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedKeyRow {
    /// Row id.
    pub id: AccountId,
    /// IMAP login, as first accepted by the mail server (later logins with
    /// other case find this account and do not change it).
    pub login: String,
    /// The wrapped data key.
    pub wrapped_dek: Vec<u8>,
}

/// Which secret of an account a sealed value holds. The number is what the
/// database stores: never reuse one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SecretField {
    /// The IMAP password (also used for SMTP).
    ImapPassword,
}

impl SecretField {
    fn code(self) -> i32 {
        match self {
            Self::ImapPassword => 1,
        }
    }
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = accounts, check_for_backend(diesel::sqlite::Sqlite))]
struct AccountRow {
    id: i64,
    login: String,
    email: String,
    wrapped_dek: Vec<u8>,
    created_at: i64,
    last_login_at: i64,
}

impl TryFrom<AccountRow> for Account {
    type Error = StoreError;

    fn try_from(row: AccountRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: AccountId::from_raw(row.id).ok_or(StoreError::Corrupt("account id"))?,
            login: row.login,
            email: row.email,
            wrapped_dek: row.wrapped_dek,
            created_at: UnixTime::from_secs(row.created_at),
            last_login_at: UnixTime::from_secs(row.last_login_at),
        })
    }
}

impl Store {
    /// The account with this login (case-insensitive), if any.
    ///
    /// # Errors
    /// On database errors.
    pub async fn account_by_login(&self, login: &str) -> Result<Option<Account>, StoreError> {
        let mut conn = self.conn().await?;
        accounts::table
            .filter(accounts::login.eq(login))
            .select(AccountRow::as_select())
            .first(&mut conn)
            .await
            .optional()?
            .map(Account::try_from)
            .transpose()
    }

    /// The account with this id, if any.
    ///
    /// # Errors
    /// On database errors.
    pub async fn account(&self, id: AccountId) -> Result<Option<Account>, StoreError> {
        let mut conn = self.conn().await?;
        accounts::table
            .find(id.get())
            .select(AccountRow::as_select())
            .first(&mut conn)
            .await
            .optional()?
            .map(Account::try_from)
            .transpose()
    }

    /// Records a successful login: creates the account the first time, and
    /// otherwise updates the login time. If the account already exists its
    /// data key and its login text are kept (logins are unique without
    /// case: `Alice` finds `alice`), so always use the returned
    /// [`Account::wrapped_dek`] and [`Account::login`], not the ones passed
    /// in. The stored spelling is the first one, which the mail server
    /// accepted; the caller checks that another spelling is the same user
    /// on the server before using the account.
    ///
    /// # Errors
    /// On database errors.
    pub async fn get_or_create_account(
        &self,
        new: NewAccount<'_>,
        now: UnixTime,
    ) -> Result<Account, StoreError> {
        let mut conn = self.conn().await?;
        diesel::insert_into(accounts::table)
            .values((
                accounts::login.eq(new.login),
                accounts::email.eq(new.email),
                accounts::wrapped_dek.eq(new.wrapped_dek),
                accounts::created_at.eq(now.secs()),
                accounts::last_login_at.eq(now.secs()),
            ))
            .on_conflict(accounts::login)
            .do_update()
            .set(accounts::last_login_at.eq(now.secs()))
            .returning(AccountRow::as_returning())
            .get_result(&mut conn)
            .await?
            .try_into()
    }

    /// Every account's id, login and wrapped data key, for re-wrapping them
    /// after the master key is rotated.
    ///
    /// # Errors
    /// On database errors.
    pub async fn wrapped_data_keys(&self) -> Result<Vec<WrappedKeyRow>, StoreError> {
        let mut conn = self.conn().await?;
        let rows: Vec<(i64, String, Vec<u8>)> = accounts::table
            .select((accounts::id, accounts::login, accounts::wrapped_dek))
            .order(accounts::id)
            .load(&mut conn)
            .await?;
        rows.into_iter()
            .map(|(id, login, wrapped_dek)| {
                Ok(WrappedKeyRow {
                    id: AccountId::from_raw(id).ok_or(StoreError::Corrupt("account id"))?,
                    login,
                    wrapped_dek,
                })
            })
            .collect()
    }

    /// Replaces an account's wrapped data key, only if it is still `old`
    /// (another process may have re-wrapped it already). Returns whether it
    /// was replaced.
    ///
    /// # Errors
    /// On database errors.
    pub async fn replace_wrapped_data_key(
        &self,
        account: AccountId,
        old: &[u8],
        new: &[u8],
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn().await?;
        let changed = diesel::update(
            accounts::table
                .find(account.get())
                .filter(accounts::wrapped_dek.eq(old)),
        )
        .set(accounts::wrapped_dek.eq(new))
        .execute(&mut conn)
        .await?;
        Ok(changed == 1)
    }

    /// Stores (or replaces) a sealed secret of an account.
    ///
    /// # Errors
    /// On database errors, including an unknown account.
    pub async fn put_secret(
        &self,
        account: AccountId,
        field: SecretField,
        sealed: &[u8],
    ) -> Result<(), StoreError> {
        let mut conn = self.conn().await?;
        diesel::insert_into(account_secrets::table)
            .values((
                account_secrets::account_id.eq(account.get()),
                account_secrets::field.eq(field.code()),
                account_secrets::sealed.eq(sealed),
            ))
            .on_conflict((account_secrets::account_id, account_secrets::field))
            .do_update()
            .set(account_secrets::sealed.eq(sealed))
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    /// A sealed secret of an account, if stored.
    ///
    /// # Errors
    /// On database errors.
    pub async fn secret(
        &self,
        account: AccountId,
        field: SecretField,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let mut conn = self.conn().await?;
        Ok(account_secrets::table
            .find((account.get(), field.code()))
            .select(account_secrets::sealed)
            .first(&mut conn)
            .await
            .optional()?)
    }

    /// Deletes every secret of an account (its last session ended).
    ///
    /// # Errors
    /// On database errors.
    pub async fn delete_secrets(&self, account: AccountId) -> Result<(), StoreError> {
        let mut conn = self.conn().await?;
        diesel::delete(
            account_secrets::table.filter(account_secrets::account_id.eq(account.get())),
        )
        .execute(&mut conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db::test_support::store;

    pub(crate) async fn alice(store: &Store) -> Account {
        let new = NewAccount {
            login: "alice",
            email: "alice@example.com",
            wrapped_dek: b"dek-1",
        };
        store
            .get_or_create_account(new, UnixTime::from_secs(1000))
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn creates_once_and_keeps_the_first_key() {
        let (store, _dir) = store().await;
        let first = alice(&store).await;
        assert_eq!(first.login, "alice");
        assert_eq!(first.wrapped_dek, b"dek-1");

        // Same login with other case: same account, first key, first login
        // text (the one the server accepted first).
        let again = store
            .get_or_create_account(
                NewAccount {
                    login: "Alice",
                    email: "other@example.com",
                    wrapped_dek: b"dek-2",
                },
                UnixTime::from_secs(2000),
            )
            .await
            .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.wrapped_dek, b"dek-1");
        assert_eq!(again.email, "alice@example.com");
        assert_eq!(again.login, "alice");
        assert_eq!(again.created_at.secs(), 1000);
        assert_eq!(again.last_login_at.secs(), 2000);

        assert_eq!(
            store.account_by_login("ALICE").await.unwrap(),
            Some(again.clone())
        );
        assert_eq!(store.account(first.id).await.unwrap(), Some(again));
        assert_eq!(store.account_by_login("bob").await.unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrapped_keys_are_replaced_only_if_unchanged() {
        let (store, _dir) = store().await;
        let account = alice(&store).await;
        let rows = store.wrapped_data_keys().await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].wrapped_dek, b"dek-1");
        assert!(
            store
                .replace_wrapped_data_key(account.id, b"dek-1", b"dek-2")
                .await
                .unwrap()
        );
        // Someone else changed it first: not replaced.
        assert!(
            !store
                .replace_wrapped_data_key(account.id, b"dek-1", b"dek-3")
                .await
                .unwrap()
        );
        assert_eq!(
            store
                .account(account.id)
                .await
                .unwrap()
                .unwrap()
                .wrapped_dek,
            b"dek-2"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secrets_are_replaced_and_deleted() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let field = SecretField::ImapPassword;
        assert_eq!(store.secret(account, field).await.unwrap(), None);
        store.put_secret(account, field, b"sealed-1").await.unwrap();
        store.put_secret(account, field, b"sealed-2").await.unwrap();
        assert_eq!(
            store.secret(account, field).await.unwrap(),
            Some(b"sealed-2".to_vec())
        );
        store.delete_secrets(account).await.unwrap();
        assert_eq!(store.secret(account, field).await.unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secrets_need_an_account() {
        let (store, _dir) = store().await;
        let missing = AccountId::from_raw(99).unwrap();
        assert!(
            store
                .put_secret(missing, SecretField::ImapPassword, b"x")
                .await
                .is_err()
        );
    }
}
