// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! OAuth codes, grants and tokens. Only SHA-256
//! hashes of codes and tokens are stored.

use std::time::Duration;

use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, OptionalExtension as _, QueryDsl as _,
    Queryable, Selectable, SelectableHelper as _,
};
use diesel_async::RunQueryDsl as _;

use crate::db::Conn;
use crate::schema::{account_secrets, oauth_codes, oauth_grants, oauth_tokens, push_subscriptions};
use crate::{AccountId, GrantId, Store, StoreError, UnixTime};

/// SHA-256 of a code or token (computed by `nebula-crypto`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenHash(pub [u8; 32]);

impl std::fmt::Debug for TokenHash {
    // A hash isn't secret, but it identifies a session: keep it out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenHash(..)")
    }
}

/// The kind of a stored token. The number is what the database stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Short-lived token for the JMAP API.
    Access,
    /// Long-lived token to get new access tokens; rotates on every use.
    Refresh,
}

impl TokenKind {
    fn code(self) -> i32 {
        match self {
            Self::Access => 1,
            Self::Refresh => REFRESH_KIND,
        }
    }
}

/// An authorization code waiting to be exchanged.
#[derive(Debug, Clone, Copy)]
pub struct NewCode<'a> {
    /// Hash of the code.
    pub hash: TokenHash,
    /// Who logged in.
    pub account: AccountId,
    /// The OAuth client that asked.
    pub client_id: &'a str,
    /// Must match exactly when the code is exchanged.
    pub redirect_uri: &'a str,
    /// PKCE S256 challenge.
    pub code_challenge: &'a str,
    /// Granted scope.
    pub scope: &'a str,
    /// After this the code is useless.
    pub expires_at: UnixTime,
}

/// An authorization code taken from the store (it can't be taken twice).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    /// Who logged in.
    pub account: AccountId,
    /// The OAuth client that asked.
    pub client_id: String,
    /// Redirect URI of the authorization request.
    pub redirect_uri: String,
    /// PKCE S256 challenge.
    pub code_challenge: String,
    /// Granted scope.
    pub scope: String,
}

/// What happened when an authorization code was presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeOutcome {
    /// First use of a valid code: exchange it with
    /// [`Store::create_grant_for_code`].
    Valid(Code),
    /// The code was already used: the grant issued from it (if any) is now
    /// revoked (RFC 6749 §4.1.2). The caller should stop the account if it
    /// has no session left.
    Reused {
        /// Whose code it was.
        account: AccountId,
    },
    /// Unknown or expired.
    Invalid,
}

/// How long a used code is kept after it expires, so a late second use is
/// still recognised and revokes its tokens.
pub const KEEP_USED_CODES: Duration = Duration::from_hours(1);

/// Who a valid token belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenOwner {
    /// The account.
    pub account: AccountId,
    /// The login session.
    pub grant: GrantId,
    /// The OAuth client.
    pub client_id: String,
    /// Granted scope.
    pub scope: String,
}

/// A refresh that rotated the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rotated {
    /// Who the session belongs to.
    pub owner: TokenOwner,
    /// When the new access token expires, after the cut at the grant's end:
    /// what the client must be told (`expires_in`).
    pub access_expires_at: UnixTime,
}

/// What happened when a refresh token was used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The old token is now used; the new tokens are stored.
    Rotated(Rotated),
    /// The token was already used moments ago (two tabs refreshing at once,
    /// within [`REUSE_GRACE`]). Nothing changed; the caller should answer
    /// "try again" rather than end the session.
    Raced,
    /// An old token was used again: it may have been stolen, so the whole
    /// grant is revoked (RFC 9700 §4.14.2). The caller should stop the
    /// account (delete its password) if it has no session left.
    Reused(TokenOwner),
    /// Unknown, expired or revoked.
    Invalid,
}

/// How long after a rotation a second use of the same refresh token counts
/// as a race and not as reuse.
pub const REUSE_GRACE: Duration = Duration::from_secs(30);

/// The new pair of tokens issued on login or refresh.
#[derive(Debug, Clone, Copy)]
pub struct NewTokens {
    /// Hash of the access token.
    pub access: TokenHash,
    /// When the access token expires.
    pub access_expires_at: UnixTime,
    /// Hash of the refresh token.
    pub refresh: TokenHash,
    /// When the refresh token expires (never after the grant).
    pub refresh_expires_at: UnixTime,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = oauth_codes, check_for_backend(diesel::sqlite::Sqlite))]
struct CodeRow {
    account_id: i64,
    used_at: Option<i64>,
    grant_id: Option<i64>,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    scope: String,
    expires_at: i64,
}

fn account_id(raw: i64) -> Result<AccountId, StoreError> {
    AccountId::from_raw(raw).ok_or(StoreError::Corrupt("account id"))
}

fn grant_id(raw: i64) -> Result<GrantId, StoreError> {
    GrantId::from_raw(raw).ok_or(StoreError::Corrupt("grant id"))
}

type OwnerColumns = (
    oauth_grants::account_id,
    oauth_grants::id,
    oauth_grants::client_id,
    oauth_grants::scope,
);

const OWNER: OwnerColumns = (
    oauth_grants::account_id,
    oauth_grants::id,
    oauth_grants::client_id,
    oauth_grants::scope,
);

fn owner(
    (account, grant, client_id, scope): (i64, i64, String, String),
) -> Result<TokenOwner, StoreError> {
    Ok(TokenOwner {
        account: account_id(account)?,
        grant: grant_id(grant)?,
        client_id,
        scope,
    })
}

/// A login session to start from a code.
#[derive(Debug, Clone, Copy)]
pub struct NewGrant<'a> {
    /// Who logged in.
    pub account: AccountId,
    /// The OAuth client.
    pub client_id: &'a str,
    /// Granted scope.
    pub scope: &'a str,
    /// Now.
    pub now: UnixTime,
    /// The session ends at this time whatever happens.
    pub expires_at: UnixTime,
}

async fn insert_grant(
    conn: &mut Conn,
    account: AccountId,
    client_id: &str,
    scope: &str,
    now: UnixTime,
    expires_at: UnixTime,
    tokens: NewTokens,
) -> Result<GrantId, StoreError> {
    let id: i64 = diesel::insert_into(oauth_grants::table)
        .values((
            oauth_grants::account_id.eq(account.get()),
            oauth_grants::client_id.eq(client_id),
            oauth_grants::scope.eq(scope),
            oauth_grants::created_at.eq(now.secs()),
            oauth_grants::expires_at.eq(expires_at.secs()),
        ))
        .returning(oauth_grants::id)
        .get_result(conn)
        .await?;
    let grant = grant_id(id)?;
    insert_tokens(conn, grant, tokens).await?;
    Ok(grant)
}

async fn insert_tokens(
    conn: &mut Conn,
    grant: GrantId,
    tokens: NewTokens,
) -> Result<(), StoreError> {
    let row = |hash: TokenHash, kind: TokenKind, expires_at: UnixTime| {
        (
            oauth_tokens::token_hash.eq(hash.0.to_vec()),
            oauth_tokens::grant_id.eq(grant.get()),
            oauth_tokens::kind.eq(kind.code()),
            oauth_tokens::expires_at.eq(expires_at.secs()),
        )
    };
    // One statement per row: diesel can't batch-insert tuples on SQLite.
    for values in [
        row(tokens.access, TokenKind::Access, tokens.access_expires_at),
        row(
            tokens.refresh,
            TokenKind::Refresh,
            tokens.refresh_expires_at,
        ),
    ] {
        diesel::insert_into(oauth_tokens::table)
            .values(values)
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// A login session that is still active: not revoked, not past its end,
/// and with a refresh token that is neither used nor expired. Without a
/// live refresh token the session can't go on (`refresh_token_days` shorter
/// than the session), so its password must go. Rotation
/// marks the old token and inserts the new one in one transaction, and a
/// grant is created with its tokens, so an active session never lacks one.
#[diesel::dsl::auto_type]
fn active_grant(now: i64) -> _ {
    let refresh: i32 = REFRESH_KIND;
    let live_refresh = oauth_tokens::table
        .filter(oauth_tokens::grant_id.eq(oauth_grants::id))
        .filter(oauth_tokens::kind.eq(refresh))
        .filter(oauth_tokens::used_at.is_null())
        .filter(oauth_tokens::expires_at.gt(now));
    oauth_grants::revoked_at
        .is_null()
        .and(oauth_grants::expires_at.gt(now))
        .and(diesel::dsl::exists(live_refresh))
}

/// [`TokenKind::Refresh`] as stored (a constant for [`active_grant`]).
const REFRESH_KIND: i32 = 2;

impl Store {
    /// Stores a new authorization code.
    ///
    /// # Errors
    /// On database errors.
    pub async fn insert_code(&self, code: NewCode<'_>) -> Result<(), StoreError> {
        let mut conn = self.conn().await?;
        diesel::insert_into(oauth_codes::table)
            .values((
                oauth_codes::code_hash.eq(code.hash.0.as_slice()),
                oauth_codes::account_id.eq(code.account.get()),
                oauth_codes::client_id.eq(code.client_id),
                oauth_codes::redirect_uri.eq(code.redirect_uri),
                oauth_codes::code_challenge.eq(code.code_challenge),
                oauth_codes::scope.eq(code.scope),
                oauth_codes::expires_at.eq(code.expires_at.secs()),
            ))
            .execute(&mut conn)
            .await?;
        Ok(())
    }

    /// Takes an authorization code: it is marked used whether or not it is
    /// still valid, so it only works once. A code presented again revokes
    /// the grant issued from it (RFC 6749 §4.1.2). See [`CodeOutcome`].
    ///
    /// # Errors
    /// On database errors.
    pub async fn take_code(
        &self,
        hash: TokenHash,
        now: UnixTime,
    ) -> Result<CodeOutcome, StoreError> {
        let mut conn = self.conn().await?;
        conn.immediate_transaction::<_, StoreError, _>(async |conn| {
            let row = oauth_codes::table
                .find(hash.0.as_slice())
                .select(CodeRow::as_select())
                .first(conn)
                .await
                .optional()?;
            let Some(row) = row else {
                return Ok(CodeOutcome::Invalid);
            };
            let account = account_id(row.account_id)?;
            if row.used_at.is_some() {
                diesel::update(oauth_codes::table.find(hash.0.as_slice()))
                    .set(oauth_codes::reused.eq(1))
                    .execute(conn)
                    .await?;
                if let Some(grant) = row.grant_id {
                    revoke(conn, grant_id(grant)?, now).await?;
                }
                return Ok(CodeOutcome::Reused { account });
            }
            diesel::update(oauth_codes::table.find(hash.0.as_slice()))
                .set(oauth_codes::used_at.eq(now.secs()))
                .execute(conn)
                .await?;
            if row.expires_at <= now.secs() {
                return Ok(CodeOutcome::Invalid);
            }
            Ok(CodeOutcome::Valid(Code {
                account,
                client_id: row.client_id,
                redirect_uri: row.redirect_uri,
                code_challenge: row.code_challenge,
                scope: row.scope,
            }))
        })
        .await
    }

    /// Starts the login session of a code taken with [`Self::take_code`],
    /// and links it to the code. `None` (and no session) if the code was
    /// presented again in the meantime.
    ///
    /// # Errors
    /// On database errors, including an unknown account.
    pub async fn create_grant_for_code(
        &self,
        code: TokenHash,
        grant: NewGrant<'_>,
        tokens: NewTokens,
    ) -> Result<Option<GrantId>, StoreError> {
        let mut conn = self.conn().await?;
        let client_id = grant.client_id.to_owned();
        let scope = grant.scope.to_owned();
        conn.immediate_transaction::<_, StoreError, _>(async |conn| {
            let reused: Option<i32> = oauth_codes::table
                .find(code.0.as_slice())
                .select(oauth_codes::reused)
                .first(conn)
                .await
                .optional()?;
            if reused != Some(0) {
                return Ok(None);
            }
            let id = insert_grant(
                conn,
                grant.account,
                &client_id,
                &scope,
                grant.now,
                grant.expires_at,
                tokens,
            )
            .await?;
            diesel::update(oauth_codes::table.find(code.0.as_slice()))
                .set(oauth_codes::grant_id.eq(id.get()))
                .execute(conn)
                .await?;
            Ok(Some(id))
        })
        .await
    }

    /// Starts a login session (grant) with its first tokens.
    ///
    /// # Errors
    /// On database errors, including an unknown account.
    pub async fn create_grant(
        &self,
        account: AccountId,
        client_id: &str,
        scope: &str,
        now: UnixTime,
        expires_at: UnixTime,
        tokens: NewTokens,
    ) -> Result<GrantId, StoreError> {
        let mut conn = self.conn().await?;
        let client_id = client_id.to_owned();
        let scope = scope.to_owned();
        conn.immediate_transaction::<_, StoreError, _>(async |conn| {
            insert_grant(conn, account, &client_id, &scope, now, expires_at, tokens).await
        })
        .await
    }

    /// Who a valid access token belongs to. `None` if unknown, expired, a
    /// refresh token, or its grant is revoked or expired.
    ///
    /// # Errors
    /// On database errors.
    pub async fn access_token(
        &self,
        hash: TokenHash,
        now: UnixTime,
    ) -> Result<Option<TokenOwner>, StoreError> {
        let mut conn = self.conn().await?;
        oauth_tokens::table
            .inner_join(oauth_grants::table)
            .filter(oauth_tokens::token_hash.eq(hash.0.as_slice()))
            .filter(oauth_tokens::kind.eq(TokenKind::Access.code()))
            .filter(oauth_tokens::expires_at.gt(now.secs()))
            .filter(oauth_grants::revoked_at.is_null())
            .filter(oauth_grants::expires_at.gt(now.secs()))
            .select(OWNER)
            .first(&mut conn)
            .await
            .optional()?
            .map(owner)
            .transpose()
    }

    /// Who a refresh token belongs to, used or not, while its session is
    /// active (for revocation, RFC 7009). `None` if unknown, expired, an
    /// access token, or its grant is revoked or expired.
    ///
    /// # Errors
    /// On database errors.
    pub async fn refresh_token_owner(
        &self,
        hash: TokenHash,
        now: UnixTime,
    ) -> Result<Option<TokenOwner>, StoreError> {
        let mut conn = self.conn().await?;
        oauth_tokens::table
            .inner_join(oauth_grants::table)
            .filter(oauth_tokens::token_hash.eq(hash.0.as_slice()))
            .filter(oauth_tokens::kind.eq(TokenKind::Refresh.code()))
            .filter(oauth_tokens::expires_at.gt(now.secs()))
            .filter(oauth_grants::revoked_at.is_null())
            .filter(oauth_grants::expires_at.gt(now.secs()))
            .select(OWNER)
            .first(&mut conn)
            .await
            .optional()?
            .map(owner)
            .transpose()
    }

    /// Exchanges a refresh token for `new` tokens (rotation). See
    /// [`RefreshOutcome`] for what can happen.
    ///
    /// # Errors
    /// On database errors.
    pub async fn rotate_refresh_token(
        &self,
        old: TokenHash,
        new: NewTokens,
        now: UnixTime,
    ) -> Result<RefreshOutcome, StoreError> {
        let mut conn = self.conn().await?;
        conn.immediate_transaction::<_, StoreError, _>(async |conn| {
            let found = oauth_tokens::table
                .inner_join(oauth_grants::table)
                .filter(oauth_tokens::token_hash.eq(old.0.as_slice()))
                .filter(oauth_tokens::kind.eq(TokenKind::Refresh.code()))
                .select((
                    OWNER,
                    oauth_tokens::expires_at,
                    oauth_tokens::used_at,
                    oauth_grants::expires_at,
                    oauth_grants::revoked_at,
                ))
                .first::<(
                    (i64, i64, String, String),
                    i64,
                    Option<i64>,
                    i64,
                    Option<i64>,
                )>(conn)
                .await
                .optional()?;
            let Some((owner_row, expires_at, used_at, grant_expires_at, revoked_at)) = found else {
                return Ok(RefreshOutcome::Invalid);
            };
            let owner = owner(owner_row)?;
            if revoked_at.is_some() || grant_expires_at <= now.secs() || expires_at <= now.secs() {
                return Ok(RefreshOutcome::Invalid);
            }
            if let Some(used_at) = used_at {
                if UnixTime::from_secs(used_at).saturating_add(REUSE_GRACE) > now {
                    return Ok(RefreshOutcome::Raced);
                }
                revoke(conn, owner.grant, now).await?;
                return Ok(RefreshOutcome::Reused(owner));
            }
            diesel::update(oauth_tokens::table.find(old.0.as_slice()))
                .set(oauth_tokens::used_at.eq(now.secs()))
                .execute(conn)
                .await?;
            // A refresh token never outlives its grant.
            let new = NewTokens {
                refresh_expires_at: new
                    .refresh_expires_at
                    .min(UnixTime::from_secs(grant_expires_at)),
                access_expires_at: new
                    .access_expires_at
                    .min(UnixTime::from_secs(grant_expires_at)),
                ..new
            };
            insert_tokens(conn, owner.grant, new).await?;
            Ok(RefreshOutcome::Rotated(Rotated {
                owner,
                access_expires_at: new.access_expires_at,
            }))
        })
        .await
    }

    /// Ends a login session: its tokens stop working at once and its push
    /// subscriptions are deleted, in one transaction.
    ///
    /// # Errors
    /// On database errors.
    pub async fn revoke_grant(&self, grant: GrantId, now: UnixTime) -> Result<(), StoreError> {
        let mut conn = self.conn().await?;
        conn.immediate_transaction::<_, StoreError, _>(async |conn| revoke(conn, grant, now).await)
            .await
    }

    /// Ends every login session of an account ("log out everywhere"),
    /// with their tokens and push subscriptions, and expires its
    /// authorization codes not exchanged yet, so a login still in flight
    /// can't start a new session afterwards; in one transaction.
    ///
    /// # Errors
    /// On database errors.
    pub async fn revoke_account_grants(
        &self,
        account: AccountId,
        now: UnixTime,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn().await?;
        conn.immediate_transaction::<_, StoreError, _>(async |conn| {
            diesel::update(
                oauth_grants::table
                    .filter(oauth_grants::account_id.eq(account.get()))
                    .filter(oauth_grants::revoked_at.is_null()),
            )
            .set(oauth_grants::revoked_at.eq(now.secs()))
            .execute(conn)
            .await?;
            diesel::delete(
                oauth_tokens::table.filter(
                    oauth_tokens::grant_id.eq_any(
                        oauth_grants::table
                            .filter(oauth_grants::account_id.eq(account.get()))
                            .select(oauth_grants::id),
                    ),
                ),
            )
            .execute(conn)
            .await?;
            // A login code not exchanged yet would make a new session
            // after this one ended everything.
            diesel::update(
                oauth_codes::table
                    .filter(oauth_codes::account_id.eq(account.get()))
                    .filter(oauth_codes::used_at.is_null())
                    .filter(oauth_codes::expires_at.gt(now.secs())),
            )
            .set(oauth_codes::expires_at.eq(now.secs()))
            .execute(conn)
            .await?;
            // Their push subscriptions go with them (RFC 8620 §7.2).
            diesel::delete(
                push_subscriptions::table.filter(push_subscriptions::account_id.eq(account.get())),
            )
            .execute(conn)
            .await?;
            Ok(())
        })
        .await
    }

    /// Whether the account still has a login session: an active grant
    /// ([`Self::has_active_grant`]), or an authorization code issued that
    /// has not started a session yet (a login in flight, which will need
    /// the secrets, [`Self::has_pending_code`]). When it has none, its
    /// secrets should be deleted.
    ///
    /// # Errors
    /// On database errors.
    pub async fn has_active_session(
        &self,
        account: AccountId,
        now: UnixTime,
    ) -> Result<bool, StoreError> {
        Ok(self.has_active_grant(account, now).await?
            || self.has_pending_code(account, now).await?)
    }

    /// Whether the account has an active login grant: not revoked or
    /// expired, with a live refresh token.
    ///
    /// # Errors
    /// On database errors.
    pub async fn has_active_grant(
        &self,
        account: AccountId,
        now: UnixTime,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn().await?;
        let count: i64 = oauth_grants::table
            .filter(oauth_grants::account_id.eq(account.get()))
            .filter(active_grant(now.secs()))
            .count()
            .get_result(&mut conn)
            .await?;
        Ok(count > 0)
    }

    /// Whether the account has a code that hasn't expired and hasn't
    /// started a session yet: a login waiting for (or in) its exchange,
    /// whose password must stay. A code that already made a session (and
    /// is reused now) doesn't count.
    ///
    /// # Errors
    /// On database errors.
    pub async fn has_pending_code(
        &self,
        account: AccountId,
        now: UnixTime,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn().await?;
        let count: i64 = oauth_codes::table
            .filter(oauth_codes::account_id.eq(account.get()))
            .filter(oauth_codes::expires_at.gt(now.secs()))
            .filter(oauth_codes::grant_id.is_null())
            .count()
            .get_result(&mut conn)
            .await?;
        Ok(count > 0)
    }

    /// Deletes expired codes and tokens, and grants that ended (expired or
    /// revoked) before `now - keep_ended`. Returns the accounts that no
    /// longer have an active session (see [`Self::has_active_session`]), so
    /// the caller can delete their secrets.
    ///
    /// # Errors
    /// On database errors.
    pub async fn cleanup_oauth(
        &self,
        now: UnixTime,
        keep_ended: Duration,
    ) -> Result<Vec<AccountId>, StoreError> {
        let mut conn = self.conn().await?;
        let cutoff = now.saturating_sub(keep_ended).secs();
        conn.immediate_transaction::<_, StoreError, _>(async |conn| {
            // Used codes stay a while, so a late reuse still revokes.
            let codes_cutoff = now.saturating_sub(KEEP_USED_CODES).secs();
            diesel::delete(oauth_codes::table.filter(oauth_codes::expires_at.le(codes_cutoff)))
                .execute(conn)
                .await?;
            diesel::delete(oauth_tokens::table.filter(oauth_tokens::expires_at.le(now.secs())))
                .execute(conn)
                .await?;
            diesel::delete(
                oauth_grants::table.filter(
                    oauth_grants::expires_at
                        .le(cutoff)
                        .or(oauth_grants::revoked_at.le(cutoff)),
                ),
            )
            .execute(conn)
            .await?;
            let active = oauth_grants::table
                .filter(active_grant(now.secs()))
                .select(oauth_grants::account_id);
            // A code that has not started a session yet is a login in
            // flight (as `has_pending_code`).
            let logging_in = oauth_codes::table
                .filter(oauth_codes::grant_id.is_null())
                .filter(oauth_codes::expires_at.gt(now.secs()))
                .select(oauth_codes::account_id);
            let without: Vec<i64> = account_secrets::table
                .filter(account_secrets::account_id.ne_all(active))
                .filter(account_secrets::account_id.ne_all(logging_in))
                .select(account_secrets::account_id)
                .distinct()
                .load(conn)
                .await?;
            without.into_iter().map(account_id).collect()
        })
        .await
    }
}

async fn revoke(conn: &mut Conn, grant: GrantId, now: UnixTime) -> Result<(), StoreError> {
    diesel::update(
        oauth_grants::table
            .find(grant.get())
            .filter(oauth_grants::revoked_at.is_null()),
    )
    .set(oauth_grants::revoked_at.eq(now.secs()))
    .execute(conn)
    .await?;
    diesel::delete(oauth_tokens::table.filter(oauth_tokens::grant_id.eq(grant.get())))
        .execute(conn)
        .await?;
    // The session's push subscriptions end with it (RFC 8620 §7.2): a
    // stolen token can't keep receiving pushes after the revocation.
    diesel::delete(push_subscriptions::table.filter(push_subscriptions::grant_id.eq(grant.get())))
        .execute(conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretField;
    use crate::account::tests::alice;
    use crate::db::test_support::store;

    const HOUR: Duration = Duration::from_hours(1);

    fn hash(n: u8) -> TokenHash {
        TokenHash([n; 32])
    }

    fn tokens(access: u8, refresh: u8, now: UnixTime) -> NewTokens {
        NewTokens {
            access: hash(access),
            access_expires_at: now.saturating_add(Duration::from_mins(15)),
            refresh: hash(refresh),
            refresh_expires_at: now.saturating_add(HOUR * 24 * 30),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn codes_work_once_and_expire() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let now = UnixTime::from_secs(10_000);
        let code = NewCode {
            hash: hash(1),
            account,
            client_id: "web",
            redirect_uri: "https://mail.example.com/auth/callback",
            code_challenge: "challenge",
            scope: "mail",
            expires_at: now.saturating_add(Duration::from_secs(60)),
        };
        store.insert_code(code).await.unwrap();
        let CodeOutcome::Valid(taken) = store.take_code(hash(1), now).await.unwrap() else {
            panic!("the code should work once");
        };
        assert_eq!(taken.account, account);
        assert_eq!(taken.redirect_uri, "https://mail.example.com/auth/callback");
        assert_eq!(
            store.take_code(hash(1), now).await.unwrap(),
            CodeOutcome::Reused { account }
        );
        assert_eq!(
            store.take_code(hash(9), now).await.unwrap(),
            CodeOutcome::Invalid
        );

        store
            .insert_code(NewCode {
                hash: hash(2),
                ..code
            })
            .await
            .unwrap();
        let late = now.saturating_add(Duration::from_secs(61));
        assert_eq!(
            store.take_code(hash(2), late).await.unwrap(),
            CodeOutcome::Invalid
        );
        // Even an expired code is used up after one try.
        assert_eq!(
            store.take_code(hash(2), now).await.unwrap(),
            CodeOutcome::Reused { account }
        );

        // Used codes are kept a while after they expire, then deleted.
        let kept = now.saturating_add(Duration::from_secs(60) + KEEP_USED_CODES);
        store.cleanup_oauth(kept, HOUR).await.unwrap();
        assert_eq!(
            store.take_code(hash(1), now).await.unwrap(),
            CodeOutcome::Invalid
        );
    }

    /// A sign-in while other writes commit (the first sync of a big
    /// mailbox): reading the code and then marking it used must not fail
    /// with "database is locked". A deferred transaction did, at once
    /// (`SQLITE_BUSY_SNAPSHOT`: the busy timeout does not help), and the web
    /// app's sign-in ended in "unavailable".
    #[tokio::test(flavor = "multi_thread")]
    async fn codes_are_taken_while_others_write() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let now = UnixTime::from_secs(10_000);
        let mut tasks = Vec::new();
        for task in 0..6_u8 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                for n in 0..40_u8 {
                    let mut bytes = [0; 32];
                    bytes[0] = task;
                    bytes[1] = n;
                    let code = TokenHash(bytes);
                    bytes[2] = 1;
                    let access = TokenHash(bytes);
                    bytes[2] = 2;
                    let refresh = TokenHash(bytes);
                    store
                        .insert_code(NewCode {
                            hash: code,
                            account,
                            client_id: "web",
                            redirect_uri: "https://mail.example.com/auth/callback",
                            code_challenge: "challenge",
                            scope: "mail",
                            expires_at: now.saturating_add(Duration::from_secs(60)),
                        })
                        .await
                        .unwrap();
                    assert!(matches!(
                        store.take_code(code, now).await.unwrap(),
                        CodeOutcome::Valid(_)
                    ));
                    let tokens = NewTokens {
                        access,
                        access_expires_at: now.saturating_add(Duration::from_mins(15)),
                        refresh,
                        refresh_expires_at: now.saturating_add(HOUR),
                    };
                    let grant = store
                        .create_grant_for_code(code, grant_of(account, now), tokens)
                        .await
                        .unwrap();
                    assert!(grant.is_some());
                }
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    }

    fn grant_of(account: AccountId, now: UnixTime) -> NewGrant<'static> {
        NewGrant {
            account,
            client_id: "web",
            scope: "mail",
            now,
            expires_at: now.saturating_add(HOUR),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_reused_code_revokes_its_tokens() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let now = UnixTime::from_secs(10_000);
        let code = NewCode {
            hash: hash(1),
            account,
            client_id: "web",
            redirect_uri: "https://mail.example.com/auth/callback",
            code_challenge: "challenge",
            scope: "mail",
            expires_at: now.saturating_add(Duration::from_secs(60)),
        };
        store.insert_code(code).await.unwrap();
        assert!(matches!(
            store.take_code(hash(1), now).await.unwrap(),
            CodeOutcome::Valid(_)
        ));
        let grant = store
            .create_grant_for_code(hash(1), grant_of(account, now), tokens(2, 3, now))
            .await
            .unwrap();
        assert!(grant.is_some());
        assert!(store.access_token(hash(2), now).await.unwrap().is_some());

        // An attacker replays the code: the tokens it gave stop working.
        assert_eq!(
            store.take_code(hash(1), now).await.unwrap(),
            CodeOutcome::Reused { account }
        );
        assert_eq!(store.access_token(hash(2), now).await.unwrap(), None);
        assert_eq!(store.refresh_token_owner(hash(3), now).await.unwrap(), None);
        assert!(!store.has_active_session(account, now).await.unwrap());

        // Replayed before the first exchange created its grant: none is made.
        store
            .insert_code(NewCode {
                hash: hash(4),
                ..code
            })
            .await
            .unwrap();
        assert!(matches!(
            store.take_code(hash(4), now).await.unwrap(),
            CodeOutcome::Valid(_)
        ));
        assert!(matches!(
            store.take_code(hash(4), now).await.unwrap(),
            CodeOutcome::Reused { .. }
        ));
        assert_eq!(
            store
                .create_grant_for_code(hash(4), grant_of(account, now), tokens(5, 6, now))
                .await
                .unwrap(),
            None
        );
        assert_eq!(store.access_token(hash(5), now).await.unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_token_owner_finds_used_and_unused_tokens() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t0 = UnixTime::from_secs(10_000);
        let grant = store
            .create_grant(
                account,
                "web",
                "mail",
                t0,
                t0.saturating_add(HOUR),
                tokens(1, 2, t0),
            )
            .await
            .unwrap();
        let owner = store
            .refresh_token_owner(hash(2), t0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((owner.account, owner.grant), (account, grant));
        // An access token is not a refresh token.
        assert_eq!(store.refresh_token_owner(hash(1), t0).await.unwrap(), None);
        // A rotated (used) token still names its session, without changing it.
        store
            .rotate_refresh_token(hash(2), tokens(3, 4, t0), t0)
            .await
            .unwrap();
        assert_eq!(
            store
                .refresh_token_owner(hash(2), t0)
                .await
                .unwrap()
                .map(|o| o.grant),
            Some(grant)
        );
        assert!(store.access_token(hash(3), t0).await.unwrap().is_some());
        store.revoke_grant(grant, t0).await.unwrap();
        assert_eq!(store.refresh_token_owner(hash(4), t0).await.unwrap(), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn access_tokens_expire_and_die_with_the_grant() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let now = UnixTime::from_secs(10_000);
        let grant = store
            .create_grant(
                account,
                "web",
                "mail",
                now,
                now.saturating_add(HOUR * 24),
                tokens(1, 2, now),
            )
            .await
            .unwrap();

        let owner = store.access_token(hash(1), now).await.unwrap().unwrap();
        assert_eq!((owner.account, owner.grant), (account, grant));
        // A refresh token is not an access token.
        assert_eq!(store.access_token(hash(2), now).await.unwrap(), None);
        let later = now.saturating_add(Duration::from_secs(901));
        assert_eq!(store.access_token(hash(1), later).await.unwrap(), None);

        store.revoke_grant(grant, now).await.unwrap();
        assert_eq!(store.access_token(hash(1), now).await.unwrap(), None);
        assert!(!store.has_active_session(account, now).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_rotates_and_reuse_revokes_everything() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t0 = UnixTime::from_secs(10_000);
        store
            .create_grant(
                account,
                "web",
                "mail",
                t0,
                t0.saturating_add(HOUR * 24),
                tokens(1, 2, t0),
            )
            .await
            .unwrap();

        let t1 = t0.saturating_add(Duration::from_secs(600));
        let outcome = store
            .rotate_refresh_token(hash(2), tokens(3, 4, t1), t1)
            .await
            .unwrap();
        assert!(matches!(outcome, RefreshOutcome::Rotated(ref o) if o.owner.account == account));
        assert!(store.access_token(hash(3), t1).await.unwrap().is_some());

        // Another tab refreshing with the same token a moment later.
        let t2 = t1.saturating_add(Duration::from_secs(5));
        assert_eq!(
            store
                .rotate_refresh_token(hash(2), tokens(5, 6, t2), t2)
                .await
                .unwrap(),
            RefreshOutcome::Raced
        );
        assert!(store.access_token(hash(3), t2).await.unwrap().is_some());

        // The old token used much later: stolen. Everything is revoked.
        let t3 = t1.saturating_add(Duration::from_secs(120));
        assert!(matches!(
            store
                .rotate_refresh_token(hash(2), tokens(7, 8, t3), t3)
                .await
                .unwrap(),
            RefreshOutcome::Reused(ref o) if o.account == account
        ));
        assert_eq!(store.access_token(hash(3), t3).await.unwrap(), None);
        assert_eq!(
            store
                .rotate_refresh_token(hash(4), tokens(9, 10, t3), t3)
                .await
                .unwrap(),
            RefreshOutcome::Invalid
        );
        assert!(!store.has_active_session(account, t3).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn new_tokens_never_outlive_the_grant() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t0 = UnixTime::from_secs(10_000);
        let grant_end = t0.saturating_add(HOUR);
        store
            .create_grant(account, "web", "mail", t0, grant_end, tokens(1, 2, t0))
            .await
            .unwrap();
        let t1 = t0.saturating_add(Duration::from_secs(60));
        let RefreshOutcome::Rotated(rotated) = store
            .rotate_refresh_token(hash(2), tokens(3, 4, t1), t1)
            .await
            .unwrap()
        else {
            panic!("the refresh should rotate");
        };
        assert_eq!(
            rotated.access_expires_at,
            t1.saturating_add(Duration::from_mins(15))
        );
        // Close to the grant's end, the cut time is what comes back.
        let near = grant_end.saturating_sub(Duration::from_secs(100));
        let RefreshOutcome::Rotated(rotated) = store
            .rotate_refresh_token(hash(4), tokens(7, 8, near), near)
            .await
            .unwrap()
        else {
            panic!("the refresh should rotate");
        };
        assert_eq!(rotated.access_expires_at, grant_end);
        let after = grant_end.saturating_add(Duration::from_secs(1));
        assert_eq!(
            store
                .rotate_refresh_token(hash(8), tokens(5, 6, after), after)
                .await
                .unwrap(),
            RefreshOutcome::Invalid
        );
    }

    /// A 90-day session whose refresh tokens last 7 days: unused for 8 days,
    /// it is over, and its password must go.
    #[tokio::test(flavor = "multi_thread")]
    async fn grant_without_live_refresh_token_is_inactive() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        store
            .put_secret(account, SecretField::ImapPassword, b"sealed")
            .await
            .unwrap();
        let day = HOUR * 24;
        let t0 = UnixTime::from_secs(10_000);
        let short = NewTokens {
            refresh_expires_at: t0.saturating_add(day * 7),
            ..tokens(1, 2, t0)
        };
        store
            .create_grant(
                account,
                "web",
                "mail",
                t0,
                t0.saturating_add(day * 90),
                short,
            )
            .await
            .unwrap();
        let t6 = t0.saturating_add(day * 6);
        assert!(store.has_active_grant(account, t6).await.unwrap());
        assert!(store.cleanup_oauth(t6, HOUR).await.unwrap().is_empty());
        // Rotated on day 6: a new live token keeps it going.
        let renewed = NewTokens {
            refresh_expires_at: t6.saturating_add(day * 7),
            ..tokens(3, 4, t6)
        };
        store
            .rotate_refresh_token(hash(2), renewed, t6)
            .await
            .unwrap();
        let t12 = t0.saturating_add(day * 12);
        assert!(store.has_active_grant(account, t12).await.unwrap());

        let t14 = t0.saturating_add(day * 14);
        assert!(!store.has_active_grant(account, t14).await.unwrap());
        assert_eq!(store.cleanup_oauth(t14, HOUR).await.unwrap(), vec![account]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pending_codes_until_they_make_a_session() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t0 = UnixTime::from_secs(10_000);
        assert!(!store.has_pending_code(account, t0).await.unwrap());
        store
            .insert_code(NewCode {
                hash: hash(1),
                account,
                client_id: "web",
                redirect_uri: "https://mail.example.com/cb",
                code_challenge: "c",
                scope: "mail",
                expires_at: t0.saturating_add(Duration::from_secs(60)),
            })
            .await
            .unwrap();
        assert!(store.has_pending_code(account, t0).await.unwrap());
        // Taken, its exchange still running: still pending.
        assert!(matches!(
            store.take_code(hash(1), t0).await.unwrap(),
            CodeOutcome::Valid(_)
        ));
        assert!(store.has_pending_code(account, t0).await.unwrap());
        let grant = NewGrant {
            account,
            client_id: "web",
            scope: "mail",
            now: t0,
            expires_at: t0.saturating_add(HOUR),
        };
        store
            .create_grant_for_code(hash(1), grant, tokens(2, 3, t0))
            .await
            .unwrap()
            .unwrap();
        assert!(!store.has_pending_code(account, t0).await.unwrap());
        // An unused one that expired doesn't count either.
        store
            .insert_code(NewCode {
                hash: hash(4),
                account,
                client_id: "web",
                redirect_uri: "https://mail.example.com/cb",
                code_challenge: "c",
                scope: "mail",
                expires_at: t0.saturating_add(Duration::from_secs(60)),
            })
            .await
            .unwrap();
        let late = t0.saturating_add(Duration::from_secs(60));
        assert!(!store.has_pending_code(account, late).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cleanup_reports_accounts_without_sessions() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        store
            .put_secret(account, SecretField::ImapPassword, b"sealed")
            .await
            .unwrap();
        let t0 = UnixTime::from_secs(10_000);
        let grant = store
            .create_grant(
                account,
                "web",
                "mail",
                t0,
                t0.saturating_add(HOUR),
                tokens(1, 2, t0),
            )
            .await
            .unwrap();
        assert!(store.cleanup_oauth(t0, HOUR).await.unwrap().is_empty());

        store.revoke_grant(grant, t0).await.unwrap();
        assert_eq!(store.cleanup_oauth(t0, HOUR).await.unwrap(), vec![account]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn logout_everywhere() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t0 = UnixTime::from_secs(10_000);
        let end = t0.saturating_add(HOUR);
        store
            .create_grant(account, "web", "mail", t0, end, tokens(1, 2, t0))
            .await
            .unwrap();
        store
            .create_grant(account, "web", "mail", t0, end, tokens(3, 4, t0))
            .await
            .unwrap();
        store.revoke_account_grants(account, t0).await.unwrap();
        assert_eq!(store.access_token(hash(1), t0).await.unwrap(), None);
        assert_eq!(store.access_token(hash(3), t0).await.unwrap(), None);
        assert!(!store.has_active_session(account, t0).await.unwrap());
    }

    fn live_code(n: u8, account: AccountId, now: UnixTime) -> NewCode<'static> {
        NewCode {
            hash: hash(n),
            account,
            client_id: "web",
            redirect_uri: "https://mail.example.com/auth/callback",
            code_challenge: "challenge",
            scope: "mail",
            expires_at: now.saturating_add(Duration::from_secs(60)),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_login_in_flight_is_a_session() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        store
            .put_secret(account, SecretField::ImapPassword, b"sealed")
            .await
            .unwrap();
        let t0 = UnixTime::from_secs(10_000);
        store.insert_code(live_code(1, account, t0)).await.unwrap();
        // A code issued and not exchanged yet keeps the secrets.
        assert!(store.has_active_session(account, t0).await.unwrap());
        assert!(store.cleanup_oauth(t0, HOUR).await.unwrap().is_empty());
        // Expired: no longer.
        let late = t0.saturating_add(Duration::from_secs(61));
        assert!(!store.has_active_session(account, late).await.unwrap());
        assert_eq!(
            store.cleanup_oauth(late, HOUR).await.unwrap(),
            vec![account]
        );

        // Used, and its grant revoked: no longer either.
        store
            .insert_code(live_code(2, account, late))
            .await
            .unwrap();
        assert!(matches!(
            store.take_code(hash(2), late).await.unwrap(),
            CodeOutcome::Valid(_)
        ));
        let grant = store
            .create_grant_for_code(
                hash(2),
                NewGrant {
                    account,
                    client_id: "web",
                    scope: "mail",
                    now: late,
                    expires_at: late.saturating_add(HOUR),
                },
                tokens(3, 4, late),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(store.has_active_session(account, late).await.unwrap());
        store.revoke_grant(grant, late).await.unwrap();
        assert!(!store.has_active_session(account, late).await.unwrap());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn logout_everywhere_expires_codes_in_flight() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t0 = UnixTime::from_secs(10_000);
        store.insert_code(live_code(1, account, t0)).await.unwrap();
        assert!(store.has_active_session(account, t0).await.unwrap());
        store.revoke_account_grants(account, t0).await.unwrap();
        assert!(!store.has_active_session(account, t0).await.unwrap());
        assert_eq!(
            store.take_code(hash(1), t0).await.unwrap(),
            CodeOutcome::Invalid
        );
    }

    /// A login code issued before "log out everywhere" (or a refused
    /// password) can't make a new session afterwards.
    #[tokio::test]
    async fn revoke_account_grants_kills_pending_codes() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let now = UnixTime::from_secs(10_000);
        store
            .insert_code(NewCode {
                hash: hash(1),
                account,
                client_id: "web",
                redirect_uri: "https://mail.example.com/auth/callback",
                code_challenge: "challenge",
                scope: "mail",
                expires_at: now.saturating_add(Duration::from_secs(60)),
            })
            .await
            .unwrap();
        store.revoke_account_grants(account, now).await.unwrap();
        assert_eq!(
            store.take_code(hash(1), now).await.unwrap(),
            CodeOutcome::Invalid
        );
    }
}
