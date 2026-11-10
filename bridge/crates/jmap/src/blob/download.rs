// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Downloads: uploads (streamed chunk by chunk from the store), whole
//! messages (`message/rfc822`) and body parts (decoded, with the part's
//! media type).
//!
//! A message that fits the IMAP layer's cache comes whole (and stays
//! cached); a bigger one is streamed from the server in pieces, and so is
//! a part with a `bs…` id (its IMAP section was checked against our parse,
//! see `blob`). `Content-Length` is always exact: the stream is cut short
//! rather than send another length.
//!
//! Some downloads need the whole message in memory: `bp…` parts (cut from
//! the parsed message), and the fallbacks when streaming isn't possible.
//! Their memory comes from the mail layer's budget for whole messages (a
//! [`Lease`]): the raw message plus the decoded part, asked for with the
//! stored size, topped up with the real one, and given back only when the
//! body has been sent (or the connection closed). A message bigger than
//! the cache is loaded with the account's turn, one at a time; the turn is
//! given back before the body goes to HTTP, so a slow client doesn't hold
//! it. A download that can't get its share in time gets a 503. The HTTP
//! layer's download slots (a few per account, held until the body ends)
//! bound how many run at once.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt as _;
use nebula_http::{AccountContext, BackendError, Blob, BlobBody, MediaType};
use nebula_jmap_proto::BlobId;
use nebula_mail::{ChunkReader, Copies, Lease, MailAccount, MailError, PartSection};
use nebula_store::{AccountId, EmailId, Store, UnixTime, UploadId};

use super::BlobRef;
use crate::core::backend::Inner;
use crate::email::body;

/// Pieces a held body is sent in (slices of one buffer, no copies).
const PIECE: usize = 64 * 1024;

/// `bytes` as the HTTP body, holding `lease` until the last piece has
/// been taken (or the body is dropped).
fn held(bytes: Bytes, lease: Lease) -> BlobBody {
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let chunks = futures::stream::unfold((bytes, lease), |(mut rest, lease)| async move {
        if rest.is_empty() {
            drop(lease);
            return None;
        }
        let piece = rest.split_to(PIECE.min(rest.len()));
        Some((Ok(piece), (rest, lease)))
    })
    .boxed();
    BlobBody::Stream { size, chunks }
}

/// The whole message and the lease that counts it: room for the message
/// and for a decoded copy as big (`copies`: one to send it as it is, two
/// to cut a part from it). A cached message is shared with the cache:
/// only the copy is counted.
async fn load_whole(
    inner: &Inner,
    account: &AccountContext,
    email: EmailId,
    copies: Copies,
) -> Result<Option<(Arc<[u8]>, Lease)>, BackendError> {
    let mail = mail_account(inner, account)?;
    let mut lease = mail.lease();
    match mail.load_message(email, &mut lease, copies).await {
        Ok(raw) => Ok(Some((raw, lease))),
        Err(MailError::NotFound(_)) => Ok(None),
        Err(error) => Err(mail_error(&error)),
    }
}

fn len_of(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

fn media_type(text: &str) -> MediaType {
    text.parse().unwrap_or_else(|_| MediaType::octet_stream())
}

pub(crate) async fn fetch(
    inner: &Inner,
    account: &AccountContext,
    blob_id: &BlobId,
) -> Result<Option<Blob>, BackendError> {
    let Some(blob) = BlobRef::parse(blob_id) else {
        return Ok(None);
    };
    match blob {
        BlobRef::Upload(id) => {
            let now = UnixTime::now().map_err(|_| BackendError::Internal("clock"))?;
            let info = inner
                .store
                .upload_info(account.id, id, now)
                .await
                .map_err(|_| BackendError::Internal("store"))?;
            Ok(info.map(|info| Blob {
                media_type: media_type(&info.media_type),
                body: BlobBody::Stream {
                    size: info.size,
                    chunks: upload_chunks(inner.store.clone(), account.id, id, info.chunks, now),
                },
            }))
        }
        BlobRef::Message(email) => message(inner, account, email).await,
        BlobRef::Part(email, part) => whole_part(inner, account, email, part).await,
        BlobRef::Section(email, part, section) => {
            section_part(inner, account, email, part, &section).await
        }
        BlobRef::Sieve(id, hash) => crate::sieve::download(inner, account, id, &hash).await,
        BlobRef::Nested(outer, part) => nested_part(inner, account, &outer, part).await,
    }
}

/// `bn…`: part `part` of the message in blob `outer` (`Email/parse`, RFC
/// 8621 §4.9). The outer blob is read whole, counted in a lease of its
/// own (it and the part cut from it), then only the part is kept.
async fn nested_part(
    inner: &Inner,
    account: &AccountContext,
    outer: &BlobRef,
    part: u32,
) -> Result<Option<Blob>, BackendError> {
    let outer_id = outer
        .to_blob_id()
        .map_err(|_| BackendError::Internal("blob id"))?;
    // Boxed: the outer blob may be nested too (up to `MAX_NESTING`).
    let Some(found) = Box::pin(fetch(inner, account, &outer_id)).await? else {
        return Ok(None);
    };
    let size = match &found.body {
        BlobBody::Bytes(bytes) => len_of(bytes),
        BlobBody::Stream { size, .. } => *size,
        _ => return Err(BackendError::Internal("unknown blob body")),
    };
    let mut lease = mail_account(inner, account)?.lease();
    lease
        .reserve(size.saturating_mul(2))
        .await
        .map_err(|error| mail_error(&error))?;
    let raw: Vec<u8> = match found.body {
        BlobBody::Bytes(bytes) => bytes.to_vec(),
        BlobBody::Stream { chunks, .. } => {
            let mut raw = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
            let mut chunks = chunks;
            while let Some(piece) = chunks.next().await {
                let piece = piece?;
                // Never more than announced: the lease counts `size`.
                if len_of(&raw).saturating_add(len_of(&piece)) > size {
                    return Err(BackendError::Internal("blob longer than its size"));
                }
                raw.extend_from_slice(&piece);
            }
            raw
        }
        _ => return Err(BackendError::Internal("unknown blob body")),
    };
    let (content, mut lease) = tokio::task::spawn_blocking(move || {
        let content = body::part_content(&raw, part);
        drop(raw);
        (content, lease)
    })
    .await
    .map_err(|_| BackendError::Internal("part download"))?;
    let Some((bytes, kind)) = content else {
        return Ok(None);
    };
    lease.keep_only(len_of(&bytes));
    Ok(Some(Blob {
        body: held(Bytes::from(bytes), lease),
        media_type: media_type(&kind),
    }))
}

fn mail_account(inner: &Inner, account: &AccountContext) -> Result<Arc<MailAccount>, BackendError> {
    inner
        .mail
        .account(account.id)
        .ok_or(BackendError::Unavailable)
}

fn mail_error(error: &MailError) -> BackendError {
    if error.is_retryable() {
        BackendError::Unavailable
    } else {
        tracing::warn!(error = %error, "message download failed");
        BackendError::Internal("mail layer")
    }
}

/// Whether the message is cached: `Some(true)` yes, `Some(false)` no (go
/// to the server), `None` for an email that is gone (404).
async fn is_cached(mail: &MailAccount, email: EmailId) -> Result<Option<bool>, BackendError> {
    match mail.cached_message(email).await {
        Ok(cached) => Ok(Some(cached.is_some())),
        Err(MailError::NotFound(_)) => Ok(None),
        Err(error) => Err(mail_error(&error)),
    }
}

/// `bm…`: from the cache, whole if it fits the cache, else streamed.
async fn message(
    inner: &Inner,
    account: &AccountContext,
    email: EmailId,
) -> Result<Option<Blob>, BackendError> {
    let mail = mail_account(inner, account)?;
    let rfc822 = || media_type("message/rfc822");
    let Some(cached) = is_cached(&mail, email).await? else {
        return Ok(None);
    };
    if !cached {
        match mail.message_reader(email).await {
            Ok(Some(reader)) => {
                return Ok(Some(Blob {
                    media_type: rfc822(),
                    body: stream(reader),
                }));
            }
            Ok(None) => {}
            Err(MailError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(mail_error(&error)),
        }
    }
    Ok(load_whole(inner, account, email, Copies::One)
        .await?
        .map(|(raw, mut lease)| {
            // Loaded: a slow client must not keep the account's turn.
            lease.drop_turn();
            Blob {
                body: held(Bytes::from_owner(raw), lease),
                media_type: rfc822(),
            }
        }))
}

/// `bp…` (and the fallback of `bs…`): the part cut from the whole message.
async fn whole_part(
    inner: &Inner,
    account: &AccountContext,
    email: EmailId,
    part: u32,
) -> Result<Option<Blob>, BackendError> {
    let Some((raw, lease)) = load_whole(inner, account, email, Copies::Two).await? else {
        return Ok(None);
    };
    // Parsing a big message is CPU work: off the async workers. The lease
    // goes with it, so that a download cancelled meanwhile (its future
    // dropped) keeps the memory counted until the thread is done with
    // the message and the part it cut.
    let (content, mut lease) = tokio::task::spawn_blocking(move || {
        let content = body::part_content(&raw, part);
        drop(raw);
        (content, lease)
    })
    .await
    .map_err(|_| BackendError::Internal("part download"))?;
    let Some((bytes, kind)) = content else {
        return Ok(None);
    };
    // The message is gone (or back to the cache's): keep only the part,
    // and give back the turn.
    lease.keep_only(len_of(&bytes));
    Ok(Some(Blob {
        body: held(Bytes::from(bytes), lease),
        media_type: media_type(&kind),
    }))
}

/// `bs…`: from the cache if the message is there, else streamed from its
/// IMAP section; from the whole message if the server's structure no
/// longer matches.
async fn section_part(
    inner: &Inner,
    account: &AccountContext,
    email: EmailId,
    part: u32,
    section: &PartSection,
) -> Result<Option<Blob>, BackendError> {
    let mail = mail_account(inner, account)?;
    let Some(cached) = is_cached(&mail, email).await? else {
        return Ok(None);
    };
    if !cached {
        match mail.part_reader(email, section).await {
            Ok(Some(download)) => {
                return Ok(Some(Blob {
                    media_type: media_type(&download.media_type),
                    body: stream(download.reader),
                }));
            }
            Ok(None) => tracing::debug!("part section changed; downloading the whole message"),
            Err(MailError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(mail_error(&error)),
        }
    }
    whole_part(inner, account, email, part).await
}

/// The pieces of a reader as the HTTP body.
fn stream(reader: ChunkReader) -> BlobBody {
    let size = reader.size();
    let chunks = futures::stream::unfold(Some(reader), |reader| async move {
        let mut reader = reader?;
        match reader.next().await {
            Ok(Some(piece)) => Some((Ok(Bytes::from(piece)), Some(reader))),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(error = %error, "streamed download cut short");
                Some((Err(BackendError::Internal("mail layer")), None))
            }
        }
    })
    .boxed();
    BlobBody::Stream { size, chunks }
}

/// The chunks of a finished upload, read one at a time while they are sent.
/// If the upload disappears meanwhile (expired, or deleted to make room),
/// the stream fails and the response is cut short.
fn upload_chunks(
    store: Store,
    account: AccountId,
    id: UploadId,
    count: u64,
    now: UnixTime,
) -> futures::stream::BoxStream<'static, Result<Bytes, BackendError>> {
    futures::stream::unfold(0_u64, move |seq| {
        let store = store.clone();
        async move {
            if seq >= count {
                return None;
            }
            let chunk = match store.upload_chunk(account, id, seq, now).await {
                Ok(Some(chunk)) => Ok(Bytes::from(chunk)),
                Ok(None) => Err(BackendError::Internal("upload gone while sent")),
                Err(_) => Err(BackendError::Internal("store")),
            };
            // After an error, stop.
            let next = if chunk.is_ok() {
                seq.saturating_add(1)
            } else {
                count
            };
            Some((chunk, next))
        }
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use std::time::Duration;

    use nebula_mail::MemoryBudget;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[tokio::test]
    async fn a_held_body_keeps_its_lease_until_sent() {
        let budget = MemoryBudget::new(4 * MIB, Duration::from_secs(30), Duration::from_secs(30));
        let mut lease = budget.lease();
        lease.reserve(3 * MIB).await.unwrap();
        assert_eq!(budget.available(), MIB);
        let bytes = Bytes::from(vec![7_u8; PIECE * 2 + 5]);
        let BlobBody::Stream { size, mut chunks } = held(bytes, lease) else {
            panic!("not a stream")
        };
        assert_eq!(size, u64::try_from(PIECE * 2 + 5).unwrap());
        let mut sent = 0;
        for _ in 0..3 {
            sent += chunks.next().await.unwrap().unwrap().len();
            assert_eq!(budget.available(), MIB);
        }
        assert_eq!(sent, PIECE * 2 + 5);
        assert!(chunks.next().await.is_none());
        assert_eq!(budget.available(), 4 * MIB);
        // Dropped half way (connection closed): given back too.
        let mut lease = budget.lease();
        lease.reserve(MIB).await.unwrap();
        let BlobBody::Stream { mut chunks, .. } = held(Bytes::from(vec![0; PIECE * 3]), lease)
        else {
            panic!("not a stream")
        };
        chunks.next().await.unwrap().unwrap();
        assert_eq!(budget.available(), 3 * MIB);
        drop(chunks);
        assert_eq!(budget.available(), 4 * MIB);
    }
}
