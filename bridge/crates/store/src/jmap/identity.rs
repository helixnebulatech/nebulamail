// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sending identities (RFC 8621 §6).

use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, Queryable, RunQueryDsl as _,
    Selectable, SelectableHelper as _, SqliteConnection,
};
use nebula_jmap_proto::object::email::EmailAddress;

use crate::mail::convert::{addresses_from_db, addresses_to_db};
use crate::record::record_in;
use crate::schema::identities;
use crate::{AccountId, Change, ChangeKind, DataType, IdentityId, Seq, Store, StoreError};

/// Most identities per account.
pub const MAX_IDENTITIES: i64 = 100;
/// Longest identity name, in bytes.
pub const MAX_IDENTITY_NAME: usize = 256;
/// Longest signature (text and HTML each), in bytes.
pub const MAX_SIGNATURE: usize = 64 * 1024;
/// Most addresses in `replyTo` and in `bcc` each.
pub const MAX_IDENTITY_ADDRESSES: usize = 32;

/// Checks the sizes of an identity's properties (jmap-proto limits the
/// requests too; this keeps the table bounded whoever calls).
fn check_sizes(
    name: &str,
    reply_to: Option<&Vec<EmailAddress>>,
    bcc: Option<&Vec<EmailAddress>>,
    text_signature: &str,
    html_signature: &str,
) -> Result<(), StoreError> {
    if name.len() > MAX_IDENTITY_NAME {
        return Err(StoreError::Invalid("identity name"));
    }
    if text_signature.len() > MAX_SIGNATURE || html_signature.len() > MAX_SIGNATURE {
        return Err(StoreError::Invalid("signature"));
    }
    if reply_to.is_some_and(|list| list.len() > MAX_IDENTITY_ADDRESSES)
        || bcc.is_some_and(|list| list.len() > MAX_IDENTITY_ADDRESSES)
    {
        return Err(StoreError::Invalid("identity addresses"));
    }
    Ok(())
}

/// A stored identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// Its id.
    pub id: IdentityId,
    /// Display name (may be empty).
    pub name: String,
    /// The From address.
    pub email: String,
    /// Default `Reply-To`.
    pub reply_to: Option<Vec<EmailAddress>>,
    /// Default `Bcc`.
    pub bcc: Option<Vec<EmailAddress>>,
    /// Plain-text signature.
    pub text_signature: String,
    /// HTML signature (clients sanitize it).
    pub html_signature: String,
    /// Whether the user may delete it.
    pub may_delete: bool,
}

/// An identity to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIdentity {
    /// Display name.
    pub name: String,
    /// The From address.
    pub email: String,
    /// Default `Reply-To`.
    pub reply_to: Option<Vec<EmailAddress>>,
    /// Default `Bcc`.
    pub bcc: Option<Vec<EmailAddress>>,
    /// Plain-text signature.
    pub text_signature: String,
    /// HTML signature.
    pub html_signature: String,
    /// Whether the user may delete it.
    pub may_delete: bool,
}

/// Changes to an identity; `None` leaves a property as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityUpdate {
    /// New name.
    pub name: Option<String>,
    /// New `Reply-To` (`Some(None)` clears it).
    pub reply_to: Option<Option<Vec<EmailAddress>>>,
    /// New `Bcc` (`Some(None)` clears it).
    pub bcc: Option<Option<Vec<EmailAddress>>>,
    /// New text signature.
    pub text_signature: Option<String>,
    /// New HTML signature.
    pub html_signature: Option<String>,
}

#[derive(Queryable, Selectable)]
#[diesel(table_name = identities, check_for_backend(diesel::sqlite::Sqlite))]
struct IdentityRow {
    id: i64,
    name: String,
    email: String,
    reply_to: Option<String>,
    bcc: Option<String>,
    text_signature: String,
    html_signature: String,
    may_delete: bool,
}

impl TryFrom<IdentityRow> for Identity {
    type Error = StoreError;

    fn try_from(row: IdentityRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: IdentityId::from_raw(row.id).ok_or(StoreError::Corrupt("identity id"))?,
            name: row.name,
            email: row.email,
            reply_to: addresses_from_db(row.reply_to.as_deref())?,
            bcc: addresses_from_db(row.bcc.as_deref())?,
            text_signature: row.text_signature,
            html_signature: row.html_signature,
            may_delete: row.may_delete,
        })
    }
}

fn load(conn: &mut SqliteConnection, account: AccountId) -> Result<Vec<Identity>, StoreError> {
    identities::table
        .filter(identities::account_id.eq(account.get()))
        .order(identities::id.asc())
        .select(IdentityRow::as_select())
        .load(conn)?
        .into_iter()
        .map(Identity::try_from)
        .collect()
}

fn insert(
    conn: &mut SqliteConnection,
    account: AccountId,
    new: &NewIdentity,
) -> Result<(IdentityId, Seq), StoreError> {
    check_sizes(
        &new.name,
        new.reply_to.as_ref(),
        new.bcc.as_ref(),
        &new.text_signature,
        &new.html_signature,
    )?;
    let count: i64 = identities::table
        .filter(identities::account_id.eq(account.get()))
        .count()
        .get_result(conn)?;
    if count >= MAX_IDENTITIES {
        return Err(StoreError::OverQuota("identities"));
    }
    let id: i64 = diesel::insert_into(identities::table)
        .values((
            identities::account_id.eq(account.get()),
            identities::name.eq(&new.name),
            identities::email.eq(&new.email),
            identities::reply_to.eq(addresses_to_db(new.reply_to.as_ref())?),
            identities::bcc.eq(addresses_to_db(new.bcc.as_ref())?),
            identities::text_signature.eq(&new.text_signature),
            identities::html_signature.eq(&new.html_signature),
            identities::may_delete.eq(new.may_delete),
        ))
        .returning(identities::id)
        .get_result(conn)?;
    let seq = record_in(
        conn,
        account,
        DataType::Identity,
        &[Change {
            object: id,
            kind: ChangeKind::Created,
        }],
    )?;
    let id = IdentityId::from_raw(id).ok_or(StoreError::Corrupt("identity id"))?;
    Ok((id, seq))
}

/// Fails with `NotFound` unless the identity is the account's.
fn check_owner(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: IdentityId,
) -> Result<Identity, StoreError> {
    identities::table
        .filter(identities::id.eq(id.get()))
        .filter(identities::account_id.eq(account.get()))
        .select(IdentityRow::as_select())
        .first(conn)
        .optional()?
        .ok_or(StoreError::NotFound("identity"))
        .and_then(Identity::try_from)
}

impl Store {
    /// The account's identities, oldest first. If it has none yet,
    /// `default` is created first (in the same transaction, so two requests
    /// can't both create it).
    ///
    /// # Errors
    /// On database errors.
    pub async fn identities_or_default(
        &self,
        account: AccountId,
        default: NewIdentity,
    ) -> Result<Vec<Identity>, StoreError> {
        // Read first: the usual case writes nothing and takes no write lock.
        let list = self.read(move |conn| load(conn, account)).await?;
        if !list.is_empty() {
            return Ok(list);
        }
        self.write(move |conn| {
            // Again inside the write transaction: another request may have
            // created it in the meantime.
            let list = load(conn, account)?;
            if !list.is_empty() {
                return Ok(list);
            }
            insert(conn, account, &default)?;
            load(conn, account)
        })
        .await
    }

    /// Creates an identity and returns it with the new Identity state.
    ///
    /// # Errors
    /// [`StoreError::OverQuota`] if the account has [`MAX_IDENTITIES`];
    /// [`StoreError::Invalid`] for a property over its size limit; on
    /// database errors.
    pub async fn create_identity(
        &self,
        account: AccountId,
        new: NewIdentity,
    ) -> Result<(IdentityId, Seq), StoreError> {
        self.write(move |conn| insert(conn, account, &new)).await
    }

    /// Changes an identity and returns the new Identity state.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it isn't the account's;
    /// [`StoreError::Invalid`] for a property over its size limit.
    pub async fn update_identity(
        &self,
        account: AccountId,
        id: IdentityId,
        update: IdentityUpdate,
    ) -> Result<Seq, StoreError> {
        self.write(move |conn| {
            let mut current = check_owner(conn, account, id)?;
            let before = current.clone();
            if let Some(name) = update.name {
                current.name = name;
            }
            if let Some(reply_to) = update.reply_to {
                current.reply_to = reply_to;
            }
            if let Some(bcc) = update.bcc {
                current.bcc = bcc;
            }
            if let Some(text) = update.text_signature {
                current.text_signature = text;
            }
            if let Some(html) = update.html_signature {
                current.html_signature = html;
            }
            if current == before {
                return record_in(conn, account, DataType::Identity, &[]);
            }
            check_sizes(
                &current.name,
                current.reply_to.as_ref(),
                current.bcc.as_ref(),
                &current.text_signature,
                &current.html_signature,
            )?;
            diesel::update(identities::table.find(id.get()))
                .set((
                    identities::name.eq(&current.name),
                    identities::reply_to.eq(addresses_to_db(current.reply_to.as_ref())?),
                    identities::bcc.eq(addresses_to_db(current.bcc.as_ref())?),
                    identities::text_signature.eq(&current.text_signature),
                    identities::html_signature.eq(&current.html_signature),
                ))
                .execute(conn)?;
            record_in(
                conn,
                account,
                DataType::Identity,
                &[Change {
                    object: id.get(),
                    kind: ChangeKind::Updated,
                }],
            )
        })
        .await
    }

    /// Deletes an identity and returns the new Identity state.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it isn't the account's.
    pub async fn destroy_identity(
        &self,
        account: AccountId,
        id: IdentityId,
    ) -> Result<Seq, StoreError> {
        self.write(move |conn| {
            check_owner(conn, account, id)?;
            diesel::delete(identities::table.find(id.get())).execute(conn)?;
            record_in(
                conn,
                account,
                DataType::Identity,
                &[Change {
                    object: id.get(),
                    kind: ChangeKind::Destroyed,
                }],
            )
        })
        .await
    }
}
