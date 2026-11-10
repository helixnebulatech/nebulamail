// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Uploaded blobs (RFC 8620 §6.1), stored in chunks: written while they
//! arrive and read while they are sent, never whole in memory. Each account
//! has a cap on the bytes of its unexpired uploads, and on how many there
//! are ([`MAX_UPLOADS_PER_ACCOUNT`]), so one account can't fill the disk.
//!
//! Writing an upload: [`Store::begin_upload`], then
//! [`Store::append_upload_chunk`] for each chunk (one short transaction
//! each, so the database isn't locked while the client sends), then
//! [`Store::finish_upload`]. Until finished it can't be read; if it never
//! is, [`Store::abort_upload`] (or the cleanup, when its short expiry
//! passes) deletes it.

use diesel::SqliteConnection;
use diesel::dsl::sql;
use diesel::sql_types::{BigInt, Nullable};
use diesel::{ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _};

use crate::mail::convert::{u64_from_db, u64_to_db};
use crate::schema::{upload_chunks, uploads};
use crate::{AccountId, Store, StoreError, UnixTime, UploadId};

/// What happened to an upload (or a chunk of it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UploadOutcome {
    /// Stored with this id.
    Stored(UploadId),
    /// It doesn't fit under the account's cap, even after deleting the
    /// account's older finished uploads: not stored.
    OverQuota,
}

/// A stored upload, whole (for the methods that use one: `Email/import`,
/// attachments of `Email/set`).
#[derive(Clone, PartialEq, Eq)]
pub struct Upload {
    /// Its id.
    pub id: UploadId,
    /// The media type given at upload.
    pub media_type: String,
    /// The bytes. Message content: never log them.
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Upload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upload")
            .field("id", &self.id)
            .field("media_type", &self.media_type)
            .field("len", &self.data.len())
            .finish()
    }
}

/// A finished upload, without its bytes (read them with
/// [`Store::upload_chunk`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadInfo {
    /// The media type given at upload.
    pub media_type: String,
    /// Size in bytes.
    pub size: u64,
    /// Number of chunks.
    pub chunks: u64,
}

pub(super) fn purge(conn: &mut SqliteConnection, now: UnixTime) -> Result<(), StoreError> {
    // Chunks go with their upload (ON DELETE CASCADE).
    diesel::delete(uploads::table.filter(uploads::expires_at.le(now.secs()))).execute(conn)?;
    Ok(())
}

/// Bytes of the account's unexpired uploads, finished or not.
fn used(conn: &mut SqliteConnection, account: AccountId, now: UnixTime) -> Result<u64, StoreError> {
    let used: Option<i64> = uploads::table
        .filter(uploads::account_id.eq(account.get()))
        .filter(uploads::expires_at.gt(now.secs()))
        .select(sql::<Nullable<BigInt>>("SUM(size)"))
        .first(conn)?;
    u64_from_db(used.unwrap_or(0), "upload size")
}

/// Makes room for `extra` more bytes under `cap`: deletes the account's
/// oldest finished uploads, as RFC 8620 §6 asks, until they fit. Returns
/// whether they fit (uploads still being written are never deleted).
fn make_room(
    conn: &mut SqliteConnection,
    account: AccountId,
    keep: UploadId,
    extra: u64,
    cap: u64,
    now: UnixTime,
) -> Result<bool, StoreError> {
    let mut used = used(conn, account, now)?;
    if used.saturating_add(extra) <= cap {
        return Ok(true);
    }
    // Delete nothing if deleting every finished upload wouldn't be enough.
    let deletable: Option<i64> = uploads::table
        .filter(uploads::account_id.eq(account.get()))
        .filter(uploads::expires_at.gt(now.secs()))
        .filter(uploads::complete.eq(true))
        .filter(uploads::id.ne(keep.get()))
        .select(sql::<Nullable<BigInt>>("SUM(size)"))
        .first(conn)?;
    let deletable = u64_from_db(deletable.unwrap_or(0), "upload size")?;
    if used.saturating_sub(deletable).saturating_add(extra) > cap {
        return Ok(false);
    }
    while used.saturating_add(extra) > cap {
        let oldest: Option<(i64, i64)> = uploads::table
            .filter(uploads::account_id.eq(account.get()))
            .filter(uploads::expires_at.gt(now.secs()))
            .filter(uploads::complete.eq(true))
            .filter(uploads::id.ne(keep.get()))
            .order((uploads::created_at.asc(), uploads::id.asc()))
            .select((uploads::id, uploads::size))
            .first(conn)
            .optional()?;
        let Some((id, size)) = oldest else {
            return Ok(false);
        };
        diesel::delete(uploads::table.find(id)).execute(conn)?;
        used = used.saturating_sub(u64_from_db(size, "upload size")?);
    }
    Ok(true)
}

/// Most unexpired uploads (rows, finished or not) per account.
pub const MAX_UPLOADS_PER_ACCOUNT: i64 = 1000;

/// Makes room for one more upload row: under the limit, deletes the
/// account's oldest finished uploads. Returns whether it fits (uploads
/// still being written are never deleted).
fn make_row_room(
    conn: &mut SqliteConnection,
    account: AccountId,
    now: UnixTime,
) -> Result<bool, StoreError> {
    let count: i64 = uploads::table
        .filter(uploads::account_id.eq(account.get()))
        .filter(uploads::expires_at.gt(now.secs()))
        .count()
        .get_result(conn)?;
    let over = count
        .saturating_sub(MAX_UPLOADS_PER_ACCOUNT)
        .saturating_add(1);
    if over <= 0 {
        return Ok(true);
    }
    let oldest: Vec<i64> = uploads::table
        .filter(uploads::account_id.eq(account.get()))
        .filter(uploads::expires_at.gt(now.secs()))
        .filter(uploads::complete.eq(true))
        .order((uploads::created_at.asc(), uploads::id.asc()))
        .select(uploads::id)
        .limit(over)
        .load(conn)?;
    if i64::try_from(oldest.len()).unwrap_or(i64::MAX) < over {
        return Ok(false);
    }
    // Chunks go with their upload (ON DELETE CASCADE).
    diesel::delete(uploads::table.filter(uploads::id.eq_any(&oldest))).execute(conn)?;
    Ok(true)
}

fn upload_id(raw: i64) -> Result<UploadId, StoreError> {
    UploadId::from_raw(raw).ok_or(StoreError::Corrupt("upload id"))
}

impl Store {
    /// Starts an upload: an empty, unfinished row that expires at
    /// `pending_until` unless [`Self::finish_upload`] runs before.
    ///
    /// An account has at most [`MAX_UPLOADS_PER_ACCOUNT`] unexpired
    /// uploads, finished or not, so empty ones (which cost no bytes of the
    /// cap) can't pile up: at the limit the oldest finished ones are
    /// deleted to make room (RFC 8620 §6), and if every one is still being
    /// written, `OverQuota` and nothing is stored.
    ///
    /// # Errors
    /// On database errors.
    pub async fn begin_upload(
        &self,
        account: AccountId,
        media_type: String,
        now: UnixTime,
        pending_until: UnixTime,
    ) -> Result<UploadOutcome, StoreError> {
        self.write(move |conn| {
            if !make_row_room(conn, account, now)? {
                return Ok(UploadOutcome::OverQuota);
            }
            let id: i64 = diesel::insert_into(uploads::table)
                .values((
                    uploads::account_id.eq(account.get()),
                    uploads::media_type.eq(media_type),
                    uploads::size.eq(0_i64),
                    uploads::complete.eq(false),
                    uploads::created_at.eq(now.secs()),
                    uploads::expires_at.eq(pending_until.secs()),
                ))
                .returning(uploads::id)
                .get_result(conn)?;
            upload_id(id).map(UploadOutcome::Stored)
        })
        .await
    }

    /// Adds chunk number `seq` (0, 1, 2…) to an unfinished upload, if the
    /// account's uploads still fit under `account_cap` bytes (older finished
    /// uploads are deleted to make room). On `OverQuota` nothing is written:
    /// abort the upload.
    ///
    /// # Errors
    /// On database errors, and [`StoreError::Invalid`] if the upload isn't
    /// an unfinished one of the account.
    pub async fn append_upload_chunk(
        &self,
        account: AccountId,
        id: UploadId,
        seq: u64,
        data: Vec<u8>,
        account_cap: u64,
        now: UnixTime,
    ) -> Result<UploadOutcome, StoreError> {
        self.write(move |conn| {
            let len = u64::try_from(data.len()).map_err(|_| StoreError::Invalid("chunk size"))?;
            let size: Option<i64> = uploads::table
                .filter(uploads::id.eq(id.get()))
                .filter(uploads::account_id.eq(account.get()))
                .filter(uploads::complete.eq(false))
                .select(uploads::size)
                .first(conn)
                .optional()?;
            let size = size.ok_or(StoreError::Invalid("upload not pending"))?;
            if !make_room(conn, account, id, len, account_cap, now)? {
                return Ok(UploadOutcome::OverQuota);
            }
            diesel::insert_into(upload_chunks::table)
                .values((
                    upload_chunks::upload_id.eq(id.get()),
                    upload_chunks::seq.eq(u64_to_db(seq, "chunk number")?),
                    upload_chunks::data.eq(data),
                ))
                .execute(conn)?;
            let total = u64_from_db(size, "upload size")?.saturating_add(len);
            diesel::update(uploads::table.find(id.get()))
                .set(uploads::size.eq(u64_to_db(total, "upload size")?))
                .execute(conn)?;
            Ok(UploadOutcome::Stored(id))
        })
        .await
    }

    /// Finishes an upload: readable until `expires_at`. Returns its size.
    ///
    /// # Errors
    /// On database errors, and [`StoreError::Invalid`] if the upload isn't
    /// an unfinished one of the account (it may have expired).
    pub async fn finish_upload(
        &self,
        account: AccountId,
        id: UploadId,
        expires_at: UnixTime,
    ) -> Result<u64, StoreError> {
        self.write(move |conn| {
            let size: Option<i64> = diesel::update(
                uploads::table
                    .filter(uploads::id.eq(id.get()))
                    .filter(uploads::account_id.eq(account.get()))
                    .filter(uploads::complete.eq(false)),
            )
            .set((
                uploads::complete.eq(true),
                uploads::expires_at.eq(expires_at.secs()),
            ))
            .returning(uploads::size)
            .get_result(conn)
            .optional()?;
            u64_from_db(
                size.ok_or(StoreError::Invalid("upload not pending"))?,
                "upload size",
            )
        })
        .await
    }

    /// Deletes an unfinished upload (the client went away, or it was too
    /// big).
    ///
    /// # Errors
    /// On database errors.
    pub async fn abort_upload(&self, account: AccountId, id: UploadId) -> Result<(), StoreError> {
        self.write(move |conn| {
            diesel::delete(
                uploads::table
                    .filter(uploads::id.eq(id.get()))
                    .filter(uploads::account_id.eq(account.get()))
                    .filter(uploads::complete.eq(false)),
            )
            .execute(conn)?;
            Ok(())
        })
        .await
    }

    /// Stores a whole upload in one chunk until `expires_at` (see
    /// [`Self::append_upload_chunk`] for the cap).
    ///
    /// # Errors
    /// On database errors.
    pub async fn put_upload(
        &self,
        account: AccountId,
        media_type: String,
        data: Vec<u8>,
        now: UnixTime,
        expires_at: UnixTime,
        account_cap: u64,
    ) -> Result<UploadOutcome, StoreError> {
        let UploadOutcome::Stored(id) = self
            .begin_upload(account, media_type, now, expires_at)
            .await?
        else {
            return Ok(UploadOutcome::OverQuota);
        };
        match self
            .append_upload_chunk(account, id, 0, data, account_cap, now)
            .await
        {
            Ok(UploadOutcome::Stored(_)) => {
                self.finish_upload(account, id, expires_at).await?;
                Ok(UploadOutcome::Stored(id))
            }
            Ok(UploadOutcome::OverQuota) => {
                self.abort_upload(account, id).await?;
                Ok(UploadOutcome::OverQuota)
            }
            Err(error) => {
                self.abort_upload(account, id).await?;
                Err(error)
            }
        }
    }

    /// A finished, unexpired upload of the account, without its bytes.
    ///
    /// # Errors
    /// On database errors.
    pub async fn upload_info(
        &self,
        account: AccountId,
        id: UploadId,
        now: UnixTime,
    ) -> Result<Option<UploadInfo>, StoreError> {
        self.read(move |conn| upload_info(conn, account, id, now))
            .await
    }

    /// Chunk number `seq` of a finished, unexpired upload of the account.
    /// `None` past the last chunk, or if the upload is gone.
    ///
    /// # Errors
    /// On database errors.
    pub async fn upload_chunk(
        &self,
        account: AccountId,
        id: UploadId,
        seq: u64,
        now: UnixTime,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.read(move |conn| {
            Ok(upload_chunks::table
                .inner_join(uploads::table)
                .filter(upload_chunks::upload_id.eq(id.get()))
                .filter(upload_chunks::seq.eq(u64_to_db(seq, "chunk number")?))
                .filter(uploads::account_id.eq(account.get()))
                .filter(uploads::complete.eq(true))
                .filter(uploads::expires_at.gt(now.secs()))
                .select(upload_chunks::data)
                .first(conn)
                .optional()?)
        })
        .await
    }

    /// A finished, unexpired upload of the account, whole in memory (only
    /// for what has to parse it; downloads read it chunk by chunk).
    ///
    /// # Errors
    /// On database errors.
    pub async fn upload(
        &self,
        account: AccountId,
        id: UploadId,
        now: UnixTime,
    ) -> Result<Option<Upload>, StoreError> {
        self.read(move |conn| {
            let Some(info) = upload_info(conn, account, id, now)? else {
                return Ok(None);
            };
            let chunks: Vec<Vec<u8>> = upload_chunks::table
                .filter(upload_chunks::upload_id.eq(id.get()))
                .order(upload_chunks::seq.asc())
                .select(upload_chunks::data)
                .load(conn)?;
            let mut data = Vec::with_capacity(
                usize::try_from(info.size).map_err(|_| StoreError::Invalid("upload size"))?,
            );
            for chunk in chunks {
                data.extend_from_slice(&chunk);
            }
            Ok(Some(Upload {
                id,
                media_type: info.media_type,
                data,
            }))
        })
        .await
    }
}

fn upload_info(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: UploadId,
    now: UnixTime,
) -> Result<Option<UploadInfo>, StoreError> {
    let row: Option<(String, i64)> = uploads::table
        .filter(uploads::id.eq(id.get()))
        .filter(uploads::account_id.eq(account.get()))
        .filter(uploads::complete.eq(true))
        .filter(uploads::expires_at.gt(now.secs()))
        .select((uploads::media_type, uploads::size))
        .first(conn)
        .optional()?;
    let Some((media_type, size)) = row else {
        return Ok(None);
    };
    let chunks: i64 = upload_chunks::table
        .filter(upload_chunks::upload_id.eq(id.get()))
        .count()
        .get_result(conn)?;
    Ok(Some(UploadInfo {
        media_type,
        size: u64_from_db(size, "upload size")?,
        chunks: u64_from_db(chunks, "chunk count")?,
    }))
}
