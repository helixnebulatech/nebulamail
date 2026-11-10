// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Uploads: written to the `uploads` table in chunks while they arrive
//! (never whole in memory), kept for a day, within a per-account total.

use std::time::Duration;

use bytes::BytesMut;
use nebula_http::{AccountContext, BackendError, MediaType, UploadBody, UploadError, UploadedBlob};
use nebula_store::{UnixTime, UploadId, UploadOutcome};

use super::BlobRef;
use crate::core::backend::Inner;

/// Bytes per stored chunk: the most of an upload held in memory at once.
pub(crate) const CHUNK_SIZE: usize = 256 * 1024;

/// How long an unfinished upload may take before the cleanup deletes it
/// (the HTTP layer cuts a request off after 5 minutes).
const PENDING_FOR: Duration = Duration::from_hours(1);

fn store_error(error: &nebula_store::StoreError) -> UploadError {
    tracing::error!(error = %error, "upload could not be stored");
    UploadError::Backend(BackendError::Internal("store"))
}

pub(crate) async fn store(
    inner: &Inner,
    account: &AccountContext,
    media_type: MediaType,
    body: UploadBody,
) -> Result<UploadedBlob, UploadError> {
    let now = UnixTime::now().map_err(|_| BackendError::Internal("clock"))?;
    let begun = inner
        .store
        .begin_upload(
            account.id,
            media_type.as_str().to_owned(),
            now,
            now.saturating_add(PENDING_FOR),
        )
        .await
        .map_err(|error| store_error(&error))?;
    let UploadOutcome::Stored(id) = begun else {
        tracing::warn!(account = account.id.get(), "upload count limit reached");
        return Err(UploadError::OverQuota);
    };
    match write(inner, account, id, body, now).await {
        Ok(size) => {
            let blob_id = BlobRef::Upload(id)
                .to_blob_id()
                .map_err(|_| BackendError::Internal("blob id"))?;
            Ok(UploadedBlob {
                blob_id,
                media_type,
                size,
            })
        }
        Err(error) => {
            if let Err(abort) = inner.store.abort_upload(account.id, id).await {
                // The cleanup deletes it when its short expiry passes.
                tracing::warn!(error = %abort, "unfinished upload not deleted");
            }
            if matches!(error, UploadError::OverQuota) {
                tracing::warn!(account = account.id.get(), "upload quota reached");
            }
            Err(error)
        }
    }
}

/// Copies the body into the upload's chunks and finishes it.
async fn write(
    inner: &Inner,
    account: &AccountContext,
    id: UploadId,
    mut body: UploadBody,
    now: UnixTime,
) -> Result<u64, UploadError> {
    let quota = inner.config.upload_quota;
    let mut buffer = BytesMut::with_capacity(CHUNK_SIZE);
    let mut seq = 0_u64;
    loop {
        let piece = body.chunk().await?;
        let done = piece.is_none();
        if let Some(piece) = piece {
            buffer.extend_from_slice(&piece);
        }
        while buffer.len() >= CHUNK_SIZE || (done && !buffer.is_empty()) {
            let take = buffer.len().min(CHUNK_SIZE);
            let chunk = buffer.split_to(take).to_vec();
            let outcome = inner
                .store
                .append_upload_chunk(account.id, id, seq, chunk, quota, now)
                .await
                .map_err(|error| store_error(&error))?;
            if outcome == UploadOutcome::OverQuota {
                return Err(UploadError::OverQuota);
            }
            seq = seq.saturating_add(1);
        }
        if done {
            break;
        }
    }
    let expires_at = now.saturating_add(inner.config.upload_lifetime);
    inner
        .store
        .finish_upload(account.id, id, expires_at)
        .await
        .map_err(|error| store_error(&error))
}
