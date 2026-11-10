// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Web Push subscriptions (RFC 8620 §7.2.7 extension) and the
//! bridge's own sealed secrets (the VAPID key).
//!
//! The store keeps the push URL and keys only sealed (opaque bytes made by
//! the caller with the account's data key, bound to the row id: the
//! sealing closure of [`Store::create_push_subscription`] runs inside the
//! transaction, once the id is known). Each subscription belongs to the
//! login session (OAuth grant) that made it (RFC 8620 §7.2): only that
//! session sees and changes it, it never outlives the session, and
//! revoking the session deletes it. A cap per session is checked in the
//! same transaction. Creating one replaces, in the same transaction, the
//! subscriptions of the same device (`deviceClientId`) made by the same
//! OAuth client in another session of the account (RFC 8620 §7.2: the id
//! names client and device); those of revoked or expired sessions no
//! longer exist. The session's own are left to the client.

use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};

use crate::schema::{oauth_grants, push_subscriptions, system_secrets};
use crate::{AccountId, GrantId, PushSubscriptionId, Store, StoreError, UnixTime};

/// Longest `deviceClientId` kept (Stalwart: 255).
pub const MAX_DEVICE_CLIENT_ID: usize = 255;
/// Most type names in `types`.
const MAX_TYPES: usize = 64;

/// A stored subscription.
#[derive(Clone, PartialEq, Eq)]
pub struct PushSubscriptionRow {
    /// Its id.
    pub id: PushSubscriptionId,
    /// The client's id for the device.
    pub device_client_id: String,
    /// URL and keys, sealed by the caller.
    pub sealed: Vec<u8>,
    /// JMAP type names; `None`: every type.
    pub types: Option<Vec<String>>,
    /// When it stops.
    pub expires: UnixTime,
    /// The code sent in the `PushVerification`.
    pub verification_code: String,
    /// The client sent the code back.
    pub verified: bool,
}

impl std::fmt::Debug for PushSubscriptionRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushSubscriptionRow")
            .field("id", &self.id)
            .field("types", &self.types)
            .field("expires", &self.expires)
            .field("verified", &self.verified)
            .finish_non_exhaustive()
    }
}

/// A subscription to create.
#[derive(Debug, Clone)]
pub struct NewPushSubscription {
    /// The client's id for the device (1 to 255 bytes).
    pub device_client_id: String,
    /// JMAP type names; `None`: every type.
    pub types: Option<Vec<String>>,
    /// When it stops.
    pub expires: UnixTime,
    /// The code to send in the `PushVerification`.
    pub verification_code: String,
}

/// A change to a subscription (`None`: unchanged).
#[derive(Debug, Clone, Default)]
pub struct PushSubscriptionChange {
    /// New `types` (`Some(None)`: every type).
    pub types: Option<Option<Vec<String>>>,
    /// New expiry.
    pub expires: Option<UnixTime>,
    /// Mark it verified.
    pub verified: bool,
    /// New sealed value (the caller sealed it for this row).
    pub sealed: Option<Vec<u8>>,
}

/// A secret of the bridge: its own data key (wrapped by the master key)
/// and the value sealed with it.
#[derive(Clone, PartialEq, Eq)]
pub struct SystemSecret {
    /// The data key, wrapped.
    pub wrapped_dek: Vec<u8>,
    /// The value, sealed.
    pub sealed: Vec<u8>,
}

impl std::fmt::Debug for SystemSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SystemSecret(..)")
    }
}

fn join_types(types: Option<&Vec<String>>) -> Result<Option<String>, StoreError> {
    let Some(types) = types else {
        return Ok(None);
    };
    let valid = types.len() <= MAX_TYPES
        && types.iter().all(|name| {
            (1..=64).contains(&name.len())
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        });
    if !valid {
        return Err(StoreError::Invalid("push types"));
    }
    Ok(Some(types.join(",")))
}

fn split_types(text: Option<String>) -> Option<Vec<String>> {
    text.map(|text| {
        text.split(',')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

type Row = (
    i64,
    String,
    Option<Vec<u8>>,
    Option<String>,
    i64,
    String,
    i32,
);

fn to_row(row: Row) -> Result<Option<PushSubscriptionRow>, StoreError> {
    let (id, device_client_id, sealed, types, expires, verification_code, verified) = row;
    // A row without its sealed value never committed that way; skip it.
    let Some(sealed) = sealed else {
        return Ok(None);
    };
    Ok(Some(PushSubscriptionRow {
        id: PushSubscriptionId::from_raw(id).ok_or(StoreError::Corrupt("push subscription id"))?,
        device_client_id,
        sealed,
        types: split_types(types),
        expires: UnixTime::from_secs(expires),
        verification_code,
        verified: verified != 0,
    }))
}

/// What [`Store::create_push_subscription`] made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatedPushSubscription {
    /// Its id.
    pub id: PushSubscriptionId,
    /// When it stops: the time asked for, or the end of its session if
    /// that comes first.
    pub expires: UnixTime,
    /// How many subscriptions of the same device (`deviceClientId`) and
    /// OAuth client, made in other sessions of the account, it replaced
    /// (deleted in the same transaction).
    pub replaced: usize,
}

/// An active login session: when it ends and its OAuth client.
struct ActiveSession {
    end: UnixTime,
    client_id: String,
}

/// The session `grant` of `account`, if it is still active at `now`.
fn active_session(
    conn: &mut diesel::SqliteConnection,
    account: AccountId,
    grant: GrantId,
    now: UnixTime,
) -> Result<Option<ActiveSession>, StoreError> {
    let found: Option<(i64, String)> = oauth_grants::table
        .find(grant.get())
        .filter(oauth_grants::account_id.eq(account.get()))
        .filter(oauth_grants::revoked_at.is_null())
        .filter(oauth_grants::expires_at.gt(now.secs()))
        .select((oauth_grants::expires_at, oauth_grants::client_id))
        .first(conn)
        .optional()?;
    Ok(found.map(|(end, client_id)| ActiveSession {
        end: UnixTime::from_secs(end),
        client_id,
    }))
}

/// Deletes the subscriptions of `account` for `device` made by
/// `client_id` in sessions other than `grant` (RFC 8620 §7.2: one device,
/// one subscription across sessions) whose sealed target `same_target`
/// says is the new one's (the same push URL). Returns how many.
///
/// Both must match: the `deviceClientId` alone would let another session
/// of the account that learnt it take away this device's notifications;
/// the push URL is a second secret only the device and we know.
fn replace_device<G>(
    conn: &mut diesel::SqliteConnection,
    account: AccountId,
    grant: GrantId,
    client_id: &str,
    device: &str,
    same_target: G,
) -> Result<usize, StoreError>
where
    G: Fn(PushSubscriptionId, &[u8]) -> bool,
{
    let same_client: Vec<i64> = oauth_grants::table
        .filter(oauth_grants::account_id.eq(account.get()))
        .filter(oauth_grants::client_id.eq(client_id))
        .select(oauth_grants::id)
        .load(conn)?;
    if same_client.is_empty() {
        return Ok(0);
    }
    let candidates: Vec<(i64, Option<Vec<u8>>)> = push_subscriptions::table
        .filter(push_subscriptions::account_id.eq(account.get()))
        .filter(push_subscriptions::device_client_id.eq(device))
        .filter(push_subscriptions::grant_id.ne(grant.get()))
        .filter(push_subscriptions::grant_id.eq_any(same_client))
        .select((push_subscriptions::id, push_subscriptions::sealed))
        .load(conn)?;
    let doomed: Vec<i64> = candidates
        .into_iter()
        .filter_map(|(id, sealed)| {
            let typed = PushSubscriptionId::from_raw(id)?;
            same_target(typed, sealed.as_deref()?).then_some(id)
        })
        .collect();
    if doomed.is_empty() {
        return Ok(0);
    }
    Ok(
        diesel::delete(push_subscriptions::table.filter(push_subscriptions::id.eq_any(doomed)))
            .execute(conn)?,
    )
}

const COLUMNS: (
    push_subscriptions::id,
    push_subscriptions::device_client_id,
    push_subscriptions::sealed,
    push_subscriptions::types,
    push_subscriptions::expires,
    push_subscriptions::verification_code,
    push_subscriptions::verified,
) = (
    push_subscriptions::id,
    push_subscriptions::device_client_id,
    push_subscriptions::sealed,
    push_subscriptions::types,
    push_subscriptions::expires,
    push_subscriptions::verification_code,
    push_subscriptions::verified,
);

fn to_rows(rows: Vec<Row>) -> Result<Vec<PushSubscriptionRow>, StoreError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(row) = to_row(row)? {
            out.push(row);
        }
    }
    Ok(out)
}

impl Store {
    /// Creates a subscription of the login session `grant` unless that
    /// session already has `max` (expired ones are deleted first; each
    /// session has its own cap, so one can't use up another's). `expires`
    /// is cut to the end of the session (RFC 8620 §7.2 lets the server
    /// shorten it). `seal` gets the new id and returns the sealed URL and
    /// keys, stored in the same transaction. `None` if the session is at
    /// its cap.
    ///
    /// Only when it creates one, it also deletes in the same transaction
    /// the subscriptions of the account with the same `deviceClientId`
    /// made by the same OAuth client in another session (RFC 8620 §7.2:
    /// the id names client and device, so a browser that logs in again
    /// replaces the one it made before; those of revoked or expired
    /// sessions no longer exist) and, by `same_target` (given each one's
    /// id and sealed value), the same push URL. The session's own are
    /// left to the client.
    ///
    /// # Errors
    /// [`StoreError::Invalid`] for a bad device id or type list;
    /// [`StoreError::NotFound`] if the session is not an active one of the
    /// account; whatever `seal` returns; database errors.
    #[expect(
        clippy::too_many_arguments,
        reason = "the two closures are what the store can't do itself"
    )]
    pub async fn create_push_subscription<F, G>(
        &self,
        account: AccountId,
        grant: GrantId,
        new: NewPushSubscription,
        max: usize,
        now: UnixTime,
        seal: F,
        same_target: G,
    ) -> Result<Option<CreatedPushSubscription>, StoreError>
    where
        F: FnOnce(PushSubscriptionId) -> Result<Vec<u8>, StoreError> + Send + 'static,
        G: Fn(PushSubscriptionId, &[u8]) -> bool + Send + 'static,
    {
        if new.device_client_id.is_empty() || new.device_client_id.len() > MAX_DEVICE_CLIENT_ID {
            return Err(StoreError::Invalid("deviceClientId"));
        }
        let types = join_types(new.types.as_ref())?;
        self.write(move |conn| {
            let session = active_session(conn, account, grant, now)?
                .ok_or(StoreError::NotFound("session"))?;
            diesel::delete(
                push_subscriptions::table
                    .filter(push_subscriptions::account_id.eq(account.get()))
                    .filter(push_subscriptions::expires.le(now.secs())),
            )
            .execute(conn)?;
            let count: i64 = push_subscriptions::table
                .filter(push_subscriptions::grant_id.eq(grant.get()))
                .count()
                .get_result(conn)?;
            if usize::try_from(count).unwrap_or(usize::MAX) >= max {
                return Ok(None);
            }
            let expires = new.expires.min(session.end);
            let id: i64 = diesel::insert_into(push_subscriptions::table)
                .values((
                    push_subscriptions::account_id.eq(account.get()),
                    push_subscriptions::grant_id.eq(grant.get()),
                    push_subscriptions::device_client_id.eq(&new.device_client_id),
                    push_subscriptions::types.eq(types),
                    push_subscriptions::expires.eq(expires.secs()),
                    push_subscriptions::verification_code.eq(&new.verification_code),
                    push_subscriptions::verified.eq(0),
                    push_subscriptions::created_at.eq(now.secs()),
                ))
                .returning(push_subscriptions::id)
                .get_result(conn)?;
            let id = PushSubscriptionId::from_raw(id)
                .ok_or(StoreError::Corrupt("push subscription id"))?;
            let sealed = seal(id)?;
            diesel::update(push_subscriptions::table.find(id.get()))
                .set(push_subscriptions::sealed.eq(Some(sealed)))
                .execute(conn)?;
            let replaced = replace_device(
                conn,
                account,
                grant,
                &session.client_id,
                &new.device_client_id,
                same_target,
            )?;
            Ok(Some(CreatedPushSubscription {
                id,
                expires,
                replaced,
            }))
        })
        .await
    }

    /// The subscriptions of the login session `grant` that have not
    /// expired at `now`, oldest first. Another session's are never here.
    ///
    /// # Errors
    /// On database errors.
    pub async fn push_subscriptions(
        &self,
        account: AccountId,
        grant: GrantId,
        now: UnixTime,
    ) -> Result<Vec<PushSubscriptionRow>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<Row> = push_subscriptions::table
                .filter(push_subscriptions::account_id.eq(account.get()))
                .filter(push_subscriptions::grant_id.eq(grant.get()))
                .filter(push_subscriptions::expires.gt(now.secs()))
                .order(push_subscriptions::id.asc())
                .select(COLUMNS)
                .load(conn)?;
            to_rows(rows)
        })
        .await
    }

    /// The account's subscriptions to push to at `now`: not expired, and
    /// their session is neither revoked nor expired. `expires` is the
    /// earlier of the subscription's and its session's end, so the
    /// delivery task drops it in time without reading again.
    ///
    /// # Errors
    /// On database errors.
    pub async fn deliverable_push_subscriptions(
        &self,
        account: AccountId,
        now: UnixTime,
    ) -> Result<Vec<PushSubscriptionRow>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<(Row, i64)> = push_subscriptions::table
                .inner_join(oauth_grants::table)
                .filter(push_subscriptions::account_id.eq(account.get()))
                .filter(push_subscriptions::expires.gt(now.secs()))
                .filter(oauth_grants::revoked_at.is_null())
                .filter(oauth_grants::expires_at.gt(now.secs()))
                .order(push_subscriptions::id.asc())
                .select((COLUMNS, oauth_grants::expires_at))
                .load(conn)?;
            let mut out = Vec::with_capacity(rows.len());
            for (row, session_end) in rows {
                if let Some(mut row) = to_row(row)? {
                    row.expires = row.expires.min(UnixTime::from_secs(session_end));
                    out.push(row);
                }
            }
            Ok(out)
        })
        .await
    }

    /// Changes a subscription of the login session `grant`; a new
    /// `expires` is cut to the end of the session. The `expires` stored
    /// now, or `None` if the subscription does not exist (or is another
    /// session's, or the session ended).
    ///
    /// # Errors
    /// [`StoreError::Invalid`] for a bad type list; database errors.
    pub async fn update_push_subscription(
        &self,
        account: AccountId,
        grant: GrantId,
        id: PushSubscriptionId,
        change: PushSubscriptionChange,
        now: UnixTime,
    ) -> Result<Option<UnixTime>, StoreError> {
        let types = match &change.types {
            Some(types) => Some(join_types(types.as_ref())?),
            None => None,
        };
        self.write(move |conn| {
            let Some(ActiveSession { end, .. }) = active_session(conn, account, grant, now)? else {
                return Ok(None);
            };
            let target = push_subscriptions::table
                .filter(push_subscriptions::id.eq(id.get()))
                .filter(push_subscriptions::account_id.eq(account.get()))
                .filter(push_subscriptions::grant_id.eq(grant.get()));
            let found: Option<i64> = target
                .select(push_subscriptions::expires)
                .first(conn)
                .optional()?;
            let Some(mut expires) = found.map(UnixTime::from_secs) else {
                return Ok(None);
            };
            if let Some(types) = types {
                diesel::update(target)
                    .set(push_subscriptions::types.eq(types))
                    .execute(conn)?;
            }
            if let Some(wanted) = change.expires {
                expires = wanted.min(end);
                diesel::update(target)
                    .set(push_subscriptions::expires.eq(expires.secs()))
                    .execute(conn)?;
            }
            if change.verified {
                diesel::update(target)
                    .set(push_subscriptions::verified.eq(1))
                    .execute(conn)?;
            }
            if let Some(sealed) = change.sealed {
                diesel::update(target)
                    .set(push_subscriptions::sealed.eq(Some(sealed)))
                    .execute(conn)?;
            }
            Ok(Some(expires))
        })
        .await
    }

    /// Deletes a subscription of the login session `grant`. `false` if it
    /// did not exist (or is another session's).
    ///
    /// # Errors
    /// On database errors.
    pub async fn delete_push_subscription(
        &self,
        account: AccountId,
        grant: GrantId,
        id: PushSubscriptionId,
    ) -> Result<bool, StoreError> {
        self.write(move |conn| {
            let deleted = diesel::delete(
                push_subscriptions::table
                    .filter(push_subscriptions::id.eq(id.get()))
                    .filter(push_subscriptions::account_id.eq(account.get()))
                    .filter(push_subscriptions::grant_id.eq(grant.get())),
            )
            .execute(conn)?;
            Ok(deleted > 0)
        })
        .await
    }

    /// Deletes a subscription of the account whatever its session: for the
    /// delivery task (the push service said it is gone, or it no longer
    /// opens). `false` if it did not exist.
    ///
    /// # Errors
    /// On database errors.
    pub async fn drop_push_subscription(
        &self,
        account: AccountId,
        id: PushSubscriptionId,
    ) -> Result<bool, StoreError> {
        self.write(move |conn| {
            let deleted = diesel::delete(
                push_subscriptions::table
                    .filter(push_subscriptions::id.eq(id.get()))
                    .filter(push_subscriptions::account_id.eq(account.get())),
            )
            .execute(conn)?;
            Ok(deleted > 0)
        })
        .await
    }

    /// Deletes every subscription of the account (its last session ended).
    ///
    /// # Errors
    /// On database errors.
    pub async fn delete_account_push_subscriptions(
        &self,
        account: AccountId,
    ) -> Result<usize, StoreError> {
        self.write(move |conn| {
            Ok(diesel::delete(
                push_subscriptions::table.filter(push_subscriptions::account_id.eq(account.get())),
            )
            .execute(conn)?)
        })
        .await
    }

    /// Deletes every subscription of every account (a new VAPID key: the
    /// browsers' subscriptions belong to the old one). Returns how many.
    ///
    /// # Errors
    /// On database errors.
    pub async fn delete_all_push_subscriptions(&self) -> Result<usize, StoreError> {
        self.write(move |conn| Ok(diesel::delete(push_subscriptions::table).execute(conn)?))
            .await
    }

    /// Accounts with a verified subscription that has not expired at
    /// `now`, in a session that is still active: they are started when the
    /// bridge starts, so new mail and reminders reach them with every tab
    /// closed.
    ///
    /// # Errors
    /// On database errors.
    pub async fn push_accounts(&self, now: UnixTime) -> Result<Vec<AccountId>, StoreError> {
        self.read(move |conn| {
            let ids: Vec<i64> = push_subscriptions::table
                .inner_join(oauth_grants::table)
                .filter(push_subscriptions::verified.eq(1))
                .filter(push_subscriptions::expires.gt(now.secs()))
                .filter(oauth_grants::revoked_at.is_null())
                .filter(oauth_grants::expires_at.gt(now.secs()))
                .select(push_subscriptions::account_id)
                .distinct()
                .load(conn)?;
            ids.into_iter()
                .map(|id| AccountId::from_raw(id).ok_or(StoreError::Corrupt("account id")))
                .collect()
        })
        .await
    }

    /// A secret of the bridge, if stored.
    ///
    /// # Errors
    /// On database errors.
    pub async fn system_secret(&self, name: &str) -> Result<Option<SystemSecret>, StoreError> {
        let name = name.to_owned();
        self.read(move |conn| {
            let row: Option<(Vec<u8>, Vec<u8>)> = system_secrets::table
                .find(name)
                .select((system_secrets::wrapped_dek, system_secrets::sealed))
                .first(conn)
                .optional()?;
            Ok(row.map(|(wrapped_dek, sealed)| SystemSecret {
                wrapped_dek,
                sealed,
            }))
        })
        .await
    }

    /// Stores a secret of the bridge unless one with that name exists (two
    /// processes starting at once); returns the one stored.
    ///
    /// # Errors
    /// On database errors.
    pub async fn put_system_secret_if_absent(
        &self,
        name: &str,
        secret: SystemSecret,
        now: UnixTime,
    ) -> Result<SystemSecret, StoreError> {
        let name = name.to_owned();
        self.write(move |conn| {
            diesel::insert_or_ignore_into(system_secrets::table)
                .values((
                    system_secrets::name.eq(&name),
                    system_secrets::wrapped_dek.eq(&secret.wrapped_dek),
                    system_secrets::sealed.eq(&secret.sealed),
                    system_secrets::created_at.eq(now.secs()),
                ))
                .execute(conn)?;
            let (wrapped_dek, sealed): (Vec<u8>, Vec<u8>) = system_secrets::table
                .find(&name)
                .select((system_secrets::wrapped_dek, system_secrets::sealed))
                .first(conn)?;
            Ok(SystemSecret {
                wrapped_dek,
                sealed,
            })
        })
        .await
    }

    /// Replaces the wrapped data key of a secret of the bridge, only if it
    /// is still `old` (after a master key rotation). `true` if replaced.
    ///
    /// # Errors
    /// On database errors.
    pub async fn replace_system_wrapped_key(
        &self,
        name: &str,
        old: &[u8],
        new: &[u8],
    ) -> Result<bool, StoreError> {
        let (name, old, new) = (name.to_owned(), old.to_vec(), new.to_vec());
        self.write(move |conn| {
            let changed = diesel::update(
                system_secrets::table
                    .filter(system_secrets::name.eq(name))
                    .filter(system_secrets::wrapped_dek.eq(old)),
            )
            .set(system_secrets::wrapped_dek.eq(new))
            .execute(conn)?;
            Ok(changed == 1)
        })
        .await
    }

    /// Deletes a secret of the bridge. `true` if it existed.
    ///
    /// # Errors
    /// On database errors.
    pub async fn delete_system_secret(&self, name: &str) -> Result<bool, StoreError> {
        let name = name.to_owned();
        self.write(move |conn| {
            Ok(diesel::delete(system_secrets::table.find(name)).execute(conn)? > 0)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::store;
    use crate::{NewAccount, NewTokens, TokenHash};

    /// Every candidate has the new one's push URL.
    fn any_target(_: PushSubscriptionId, _: &[u8]) -> bool {
        true
    }

    async fn account(store: &Store, login: &str) -> AccountId {
        store
            .get_or_create_account(
                NewAccount {
                    login,
                    email: "a@example.com",
                    wrapped_dek: b"k",
                },
                UnixTime::from_secs(1),
            )
            .await
            .unwrap()
            .id
    }

    fn new(device: &str, expires: i64) -> NewPushSubscription {
        NewPushSubscription {
            device_client_id: device.to_owned(),
            types: Some(vec!["EmailDelivery".to_owned(), "CalendarAlert".to_owned()]),
            expires: UnixTime::from_secs(expires),
            verification_code: "code".to_owned(),
        }
    }

    /// A login session of `account` (OAuth client `web`) that ends at `ends`.
    async fn session(store: &Store, account: AccountId, n: u8, ends: i64) -> GrantId {
        session_of(store, account, "web", n, ends).await
    }

    /// A login session of `account` made by `client`, ending at `ends`.
    async fn session_of(
        store: &Store,
        account: AccountId,
        client: &str,
        n: u8,
        ends: i64,
    ) -> GrantId {
        let ends = UnixTime::from_secs(ends);
        store
            .create_grant(
                account,
                client,
                "mail",
                UnixTime::from_secs(1),
                ends,
                NewTokens {
                    access: TokenHash([n; 32]),
                    access_expires_at: ends,
                    refresh: TokenHash([n.wrapping_add(128); 32]),
                    refresh_expires_at: ends,
                },
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "one story, step by step")]
    async fn subscriptions() {
        let (store, _dir) = store().await;
        let alice = account(&store, "alice").await;
        let bob = account(&store, "bob").await;
        let a1 = session(&store, alice, 1, 100_000).await;
        let a2 = session(&store, alice, 2, 100_000).await;
        let b1 = session(&store, bob, 3, 100_000).await;
        let now = UnixTime::from_secs(100);
        let created = store
            .create_push_subscription(
                alice,
                a1,
                new("dev1", 1000),
                2,
                now,
                |id| Ok(format!("sealed for {}", id.get()).into_bytes()),
                any_target,
            )
            .await
            .unwrap()
            .unwrap();
        let id = created.id;
        assert_eq!(created.expires, UnixTime::from_secs(1000));
        let list = store.push_subscriptions(alice, a1, now).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(
            list[0].sealed,
            format!("sealed for {}", id.get()).into_bytes()
        );
        assert_eq!(
            list[0].types.as_deref(),
            Some(&["EmailDelivery".to_owned(), "CalendarAlert".to_owned()][..])
        );
        assert!(!list[0].verified);
        assert!(!format!("{:?}", list[0]).contains("sealed for"));
        assert!(store.push_accounts(now).await.unwrap().is_empty());
        // Another session of the same account doesn't see it; the
        // delivery task does.
        assert!(
            store
                .push_subscriptions(alice, a2, now)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .deliverable_push_subscriptions(alice, now)
                .await
                .unwrap()
                .len(),
            1
        );

        // Cap of 2 per session; an expired one does not count (and goes).
        store
            .create_push_subscription(
                alice,
                a1,
                new("dev2", 150),
                2,
                now,
                |_| Ok(vec![1]),
                any_target,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            store
                .create_push_subscription(
                    alice,
                    a1,
                    new("dev3", 1000),
                    2,
                    now,
                    |_| Ok(vec![1]),
                    any_target
                )
                .await
                .unwrap()
                .is_none()
        );
        // A session at its cap doesn't stop another one.
        assert!(
            store
                .create_push_subscription(
                    alice,
                    a2,
                    new("dev4", 1000),
                    2,
                    now,
                    |_| Ok(vec![1]),
                    any_target
                )
                .await
                .unwrap()
                .is_some()
        );
        let later = UnixTime::from_secs(200);
        assert!(
            store
                .create_push_subscription(
                    alice,
                    a1,
                    new("dev3", 1000),
                    2,
                    later,
                    |_| Ok(vec![1]),
                    any_target
                )
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store
                .push_subscriptions(alice, a1, later)
                .await
                .unwrap()
                .len(),
            2
        );

        // A failed seal leaves nothing; nor does someone else's session.
        assert!(
            store
                .create_push_subscription(
                    bob,
                    b1,
                    new("dev", 1000),
                    2,
                    now,
                    |_| { Err(StoreError::Invalid("seal")) },
                    any_target
                )
                .await
                .is_err()
        );
        assert!(
            store
                .create_push_subscription(
                    bob,
                    a1,
                    new("dev", 1000),
                    2,
                    now,
                    |_| Ok(vec![1]),
                    any_target
                )
                .await
                .is_err()
        );
        assert!(
            store
                .push_subscriptions(bob, b1, now)
                .await
                .unwrap()
                .is_empty()
        );

        // Updates, only for the session that made it.
        let change = PushSubscriptionChange {
            types: Some(None),
            expires: Some(UnixTime::from_secs(2000)),
            verified: true,
            sealed: Some(vec![9, 9]),
        };
        for (who, grant) in [(bob, b1), (alice, a2), (bob, a1)] {
            assert!(
                store
                    .update_push_subscription(who, grant, id, change.clone(), later)
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(
            store
                .update_push_subscription(alice, a1, id, change, later)
                .await
                .unwrap(),
            Some(UnixTime::from_secs(2000))
        );
        let first = store.push_subscriptions(alice, a1, later).await.unwrap()[0].clone();
        assert!(first.verified && first.types.is_none());
        assert_eq!(first.sealed, vec![9, 9]);
        assert_eq!(first.expires, UnixTime::from_secs(2000));
        assert_eq!(store.push_accounts(later).await.unwrap(), [alice]);
        assert!(
            store
                .update_push_subscription(
                    alice,
                    a1,
                    id,
                    PushSubscriptionChange {
                        types: Some(Some(vec!["Bad,Name".to_owned()])),
                        ..PushSubscriptionChange::default()
                    },
                    later
                )
                .await
                .is_err()
        );

        for (who, grant) in [(bob, b1), (alice, a2)] {
            assert!(
                !store
                    .delete_push_subscription(who, grant, id)
                    .await
                    .unwrap()
            );
        }
        assert!(store.delete_push_subscription(alice, a1, id).await.unwrap());
        // The delivery task drops any of the account's.
        let other = store.push_subscriptions(alice, a2, later).await.unwrap()[0].id;
        assert!(!store.drop_push_subscription(bob, other).await.unwrap());
        assert!(store.drop_push_subscription(alice, other).await.unwrap());
        assert_eq!(
            store
                .delete_account_push_subscriptions(alice)
                .await
                .unwrap(),
            1
        );
        assert!(
            store
                .create_push_subscription(
                    alice,
                    a1,
                    new("", 1000),
                    2,
                    now,
                    |_| Ok(vec![]),
                    any_target
                )
                .await
                .is_err()
        );
        store
            .create_push_subscription(bob, b1, new("d", 1000), 2, now, |_| Ok(vec![1]), any_target)
            .await
            .unwrap();
        assert_eq!(store.delete_all_push_subscriptions().await.unwrap(), 1);
    }

    /// A subscription never outlives its login session (RFC 8620 §7.2):
    /// `expires` is cut to the session's end, and a revoked session takes
    /// its subscriptions with it.
    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "one story, step by step")]
    async fn subscriptions_end_with_their_session() {
        let (store, _dir) = store().await;
        let alice = account(&store, "alice").await;
        let short = session(&store, alice, 1, 500).await;
        let g1 = session(&store, alice, 2, 100_000).await;
        let g2 = session(&store, alice, 3, 100_000).await;
        let now = UnixTime::from_secs(100);
        let seal = |_| Ok(vec![1]);

        // Cut to the end of the session, when created and when changed.
        let cut = store
            .create_push_subscription(alice, short, new("short", 1000), 10, now, seal, any_target)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cut.expires, UnixTime::from_secs(500));
        let change = PushSubscriptionChange {
            expires: Some(UnixTime::from_secs(2000)),
            ..PushSubscriptionChange::default()
        };
        assert_eq!(
            store
                .update_push_subscription(alice, short, cut.id, change.clone(), now)
                .await
                .unwrap(),
            Some(UnixTime::from_secs(500))
        );
        // Past its end, nothing goes out, and the session can't create
        // or change any.
        let after = UnixTime::from_secs(600);
        assert!(
            store
                .deliverable_push_subscriptions(alice, after)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .create_push_subscription(
                    alice,
                    short,
                    new("late", 1000),
                    10,
                    after,
                    seal,
                    any_target
                )
                .await
                .is_err()
        );
        assert!(
            store
                .update_push_subscription(alice, short, cut.id, change, after)
                .await
                .unwrap()
                .is_none()
        );

        // Revoking a session deletes its subscriptions, not the others'.
        let one = store
            .create_push_subscription(alice, g1, new("one", 1000), 10, now, seal, any_target)
            .await
            .unwrap()
            .unwrap();
        let two = store
            .create_push_subscription(alice, g2, new("two", 1000), 10, now, seal, any_target)
            .await
            .unwrap()
            .unwrap();
        store
            .update_push_subscription(
                alice,
                g1,
                one.id,
                PushSubscriptionChange {
                    verified: true,
                    ..PushSubscriptionChange::default()
                },
                now,
            )
            .await
            .unwrap();
        assert_eq!(store.push_accounts(now).await.unwrap(), [alice]);
        store.revoke_grant(g1, now).await.unwrap();
        assert!(
            store
                .push_subscriptions(alice, g1, now)
                .await
                .unwrap()
                .is_empty()
        );
        let left: Vec<_> = store
            .deliverable_push_subscriptions(alice, now)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert_eq!(left, [cut.id, two.id]);
        // Only unverified ones are left: the bridge doesn't start for them.
        assert!(store.push_accounts(now).await.unwrap().is_empty());
        assert!(
            store
                .create_push_subscription(alice, g1, new("again", 1000), 10, now, seal, any_target)
                .await
                .is_err()
        );

        // Logging out everywhere deletes them all.
        store.revoke_account_grants(alice, now).await.unwrap();
        assert!(
            store
                .push_subscriptions(alice, g2, now)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .delete_account_push_subscriptions(alice)
                .await
                .unwrap(),
            0
        );
    }

    /// One device, one subscription across sessions (RFC 8620 §7.2): a new
    /// one from another session of the same client and `deviceClientId`
    /// replaces the old, in the same transaction and only if it is made.
    #[tokio::test]
    #[expect(clippy::too_many_lines, reason = "one story, step by step")]
    async fn creating_replaces_the_device_in_other_sessions() {
        let (store, _dir) = store().await;
        let alice = account(&store, "alice").await;
        let bob = account(&store, "bob").await;
        let g1 = session(&store, alice, 1, 100_000).await;
        let g2 = session(&store, alice, 2, 100_000).await;
        let short = session(&store, alice, 3, 500).await;
        let app = session_of(&store, alice, "other", 4, 100_000).await;
        let b1 = session(&store, bob, 5, 100_000).await;
        let g3 = session(&store, alice, 6, 100_000).await;
        let now = UnixTime::from_secs(100);
        let create = async |who, grant, device: &str, max, at| {
            store
                .create_push_subscription(
                    who,
                    grant,
                    new(device, 1000),
                    max,
                    at,
                    |_| Ok(vec![1]),
                    any_target,
                )
                .await
        };
        let ids = |rows: Vec<PushSubscriptionRow>| -> Vec<PushSubscriptionId> {
            rows.into_iter().map(|row| row.id).collect()
        };

        // Same device id, but another OAuth client and another account.
        let s4 = create(alice, app, "dev", 10, now).await.unwrap().unwrap();
        let s5 = create(bob, b1, "dev", 10, now).await.unwrap().unwrap();
        assert_eq!((s4.replaced, s5.replaced), (0, 0));
        let s2 = create(alice, g1, "keep", 10, now).await.unwrap().unwrap();
        let s3 = create(alice, short, "dev", 10, now).await.unwrap().unwrap();
        assert_eq!(s3.replaced, 0);
        // g1 subscribes "dev": the one of the session `short` goes, not
        // the other client's nor bob's.
        let s1 = create(alice, g1, "dev", 10, now).await.unwrap().unwrap();
        assert_eq!(s1.replaced, 1);
        assert!(!store.drop_push_subscription(alice, s3.id).await.unwrap());
        assert_eq!(
            ids(store.push_subscriptions(alice, app, now).await.unwrap()),
            [s4.id]
        );
        assert_eq!(
            ids(store.push_subscriptions(bob, b1, now).await.unwrap()),
            [s5.id]
        );
        store
            .update_push_subscription(
                alice,
                g1,
                s1.id,
                PushSubscriptionChange {
                    verified: true,
                    ..PushSubscriptionChange::default()
                },
                now,
            )
            .await
            .unwrap();
        assert_eq!(store.push_accounts(now).await.unwrap(), [alice]);
        let has_s1 = |rows: Vec<PushSubscriptionRow>| rows.iter().any(|row| row.id == s1.id);

        // Nothing made, nothing replaced: at its cap …
        let x1 = create(alice, g2, "x1", 10, now).await.unwrap().unwrap();
        let x2 = create(alice, g2, "x2", 10, now).await.unwrap().unwrap();
        assert!(create(alice, g2, "dev", 2, now).await.unwrap().is_none());
        assert!(has_s1(
            store.push_subscriptions(alice, g1, now).await.unwrap()
        ));
        // … a failed seal (the transaction rolls back) …
        assert!(
            store
                .create_push_subscription(
                    alice,
                    g2,
                    new("dev", 1000),
                    10,
                    now,
                    |_| { Err(StoreError::Invalid("seal")) },
                    any_target
                )
                .await
                .is_err()
        );
        assert!(has_s1(
            store.push_subscriptions(alice, g1, now).await.unwrap()
        ));
        // … or a session that is no longer active.
        store.revoke_grant(g3, now).await.unwrap();
        assert!(matches!(
            create(alice, g3, "dev", 10, now).await,
            Err(StoreError::NotFound(_))
        ));
        assert!(has_s1(
            store.push_subscriptions(alice, g1, now).await.unwrap()
        ));

        // A row of an expired session is already gone when another
        // session subscribes the same device: the sweep of expired ones,
        // before the cap, took it (it never outlives its session), so it
        // is not counted as replaced.
        let late = create(alice, short, "late", 10, now)
            .await
            .unwrap()
            .unwrap();
        let now2 = UnixTime::from_secs(600);
        let relogin = create(alice, g2, "late", 10, now2).await.unwrap().unwrap();
        assert_eq!(relogin.replaced, 0);
        assert!(!store.drop_push_subscription(alice, late.id).await.unwrap());
        assert!(
            store
                .drop_push_subscription(alice, relogin.id)
                .await
                .unwrap()
        );

        // g2 subscribes "dev": g1's (verified) one goes; "keep" stays.
        let n1 = create(alice, g2, "dev", 10, now2).await.unwrap().unwrap();
        assert_eq!(n1.replaced, 1);
        assert_eq!(
            ids(store.push_subscriptions(alice, g1, now2).await.unwrap()),
            [s2.id]
        );
        assert_eq!(
            ids(store
                .deliverable_push_subscriptions(alice, now2)
                .await
                .unwrap()),
            [s4.id, s2.id, x1.id, x2.id, n1.id]
        );
        assert_eq!(
            ids(store.push_subscriptions(bob, b1, now2).await.unwrap()),
            [s5.id]
        );
        assert!(!store.drop_push_subscription(alice, s1.id).await.unwrap());
        let kept_new = |rows: Vec<PushSubscriptionRow>| rows.iter().any(|row| row.id == n1.id);
        // s1 was the only verified one.
        assert!(store.push_accounts(now2).await.unwrap().is_empty());

        // Same device id from another session, but another push URL (the
        // sealed value says where): not replaced. Only someone with both
        // secrets of the device can take its notifications away.
        let elsewhere = store
            .create_push_subscription(
                alice,
                g1,
                new("dev", 1000),
                10,
                now2,
                |_| Ok(b"https://push.example/other".to_vec()),
                |_, sealed| sealed == b"https://push.example/other",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(elsewhere.replaced, 0);
        assert!(kept_new(
            store.push_subscriptions(alice, g2, now2).await.unwrap()
        ));
        assert!(
            store
                .drop_push_subscription(alice, elsewhere.id)
                .await
                .unwrap()
        );

        // The session's own are left to the client.
        let n2 = create(alice, g2, "dev", 10, now2).await.unwrap().unwrap();
        assert_eq!(n2.replaced, 0);
        assert_eq!(
            ids(store.push_subscriptions(alice, g2, now2).await.unwrap()),
            [x1.id, x2.id, n1.id, n2.id]
        );
    }

    #[tokio::test]
    async fn system_secrets() {
        let (store, _dir) = store().await;
        let now = UnixTime::from_secs(1);
        assert!(store.system_secret("vapid").await.unwrap().is_none());
        let first = SystemSecret {
            wrapped_dek: vec![1],
            sealed: vec![2],
        };
        let second = SystemSecret {
            wrapped_dek: vec![3],
            sealed: vec![4],
        };
        assert_eq!(
            store
                .put_system_secret_if_absent("vapid", first.clone(), now)
                .await
                .unwrap(),
            first
        );
        // The first one stays.
        assert_eq!(
            store
                .put_system_secret_if_absent("vapid", second, now)
                .await
                .unwrap(),
            first
        );
        assert!(
            !store
                .replace_system_wrapped_key("vapid", &[9], &[5])
                .await
                .unwrap()
        );
        assert!(
            store
                .replace_system_wrapped_key("vapid", &[1], &[5])
                .await
                .unwrap()
        );
        assert_eq!(
            store
                .system_secret("vapid")
                .await
                .unwrap()
                .unwrap()
                .wrapped_dek,
            [5]
        );
        assert!(store.delete_system_secret("vapid").await.unwrap());
        assert!(!store.delete_system_secret("vapid").await.unwrap());
    }
}
