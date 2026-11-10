// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/import` (RFC 8621 §4.8): a blob (usually an upload) appended to a
//! mailbox as it is.
//!
//! Every email is checked (its mailboxes too, before anything is
//! appended); then the ones going to the same (first) mailbox are
//! appended together, in one command when the IMAP server has MULTIAPPEND
//! (the mail layer goes one by one if the server refuses the batch, so
//! each email still gets its own error).
//!
//! Memory: the same blob imported several times is read once and shared,
//! and the checked emails are appended in batches of about
//! [`MAX_HELD_BYTES`], not all at the end: 500 imports of a 50 MB blob
//! don't hold 500 copies.
//!
//! A call imports at most `max_import_bytes` in all (each blob counted
//! once, however many imports name it); the imports past that fail with
//! `tooLarge`, before their blob is loaded when its size is known.
//!
//! The held blobs are counted in the mail layer's budget for whole
//! messages (a [`Lease`]), twice: the blob and its copy in the `APPEND`
//! literal. A batch that would grow past [`MAX_HELD_BYTES`] is appended
//! before the next blob is read when its size is known (after, when not).
//! An import that doesn't get its memory in time fails with "try again
//! later"; the others go on.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use nebula_jmap_proto::method::{EmailImport, EmailImportArguments, EmailImportResponse};
use nebula_jmap_proto::object::Email;
use nebula_jmap_proto::{BlobId, CreationId, Keyword, MethodResponse, SetError, SetErrorType};
use nebula_mail::{AppendItem, Lease, MailAccount};
use nebula_store::{DataType, EmailId, MailboxId, UnixTime};

use super::build::blob_bytes;
use super::checked_mailboxes;
use super::set::{created_properties, put_in_all, store_message};
use crate::blob::BlobRef;
use crate::core::context::Ctx;
use crate::error::{CallError, mail_set_error};

/// Bytes of checked emails held before they are appended. One bigger
/// email is a batch of its own.
const MAX_HELD_BYTES: usize = 32 * 1024 * 1024;

/// An import that passed the checks.
struct Checked {
    raw: Arc<[u8]>,
    mailboxes: BTreeSet<MailboxId>,
    keywords: BTreeSet<Keyword>,
    received_at: Option<UnixTime>,
}

/// The blobs read so far in this batch, shared by the imports that use
/// the same one.
#[derive(Default)]
struct Held {
    blobs: HashMap<BlobId, Arc<[u8]>>,
    bytes: usize,
    /// What the blobs take of the call's lease.
    reserved: u64,
}

impl Held {
    fn add(&mut self, id: BlobId, blob: Arc<[u8]>, reserved: u64) {
        self.bytes = self.bytes.saturating_add(blob.len());
        self.reserved = self.reserved.saturating_add(reserved);
        self.blobs.insert(id, blob);
    }

    /// Empties it (once appended), giving its memory back.
    fn clear(&mut self, lease: &mut Lease) {
        lease.release(self.reserved);
        *self = Self::default();
    }
}

/// Whether the held batch must be appended before one more blob of
/// `adding` bytes is held.
fn needs_flush(held: usize, adding: usize) -> bool {
    held > 0 && held.saturating_add(adding) > MAX_HELD_BYTES
}

/// The bytes a call may still import, each blob counted once.
struct Budget {
    remaining: u64,
    counted: HashSet<BlobId>,
}

fn too_large() -> SetError {
    SetError::new(SetErrorType::TooLarge).with_description("import too large for one call")
}

impl Budget {
    fn new(max: u64) -> Self {
        Self {
            remaining: max,
            counted: HashSet::new(),
        }
    }

    /// Whether `id` (of `size` bytes) fits: a blob already counted always
    /// does.
    fn fits(&self, id: &BlobId, size: u64) -> bool {
        self.counted.contains(id) || size <= self.remaining
    }

    /// Counts a loaded blob (once).
    fn take(&mut self, id: &BlobId, size: usize) -> Result<(), SetError> {
        let size = u64::try_from(size).unwrap_or(u64::MAX);
        if !self.fits(id, size) {
            return Err(too_large());
        }
        if self.counted.insert(id.clone()) {
            self.remaining = self.remaining.saturating_sub(size);
        }
        Ok(())
    }
}

/// The size of a blob without loading it, when the store knows it.
async fn known_size(ctx: &Ctx<'_>, id: &BlobId) -> Option<u64> {
    match BlobRef::parse(id)? {
        BlobRef::Upload(upload) => {
            let now = Ctx::now().ok()?;
            let info = ctx
                .store()
                .upload_info(ctx.id(), upload, now)
                .await
                .ok()??;
            Some(info.size)
        }
        BlobRef::Message(email) => {
            let stored = ctx.store().emails(ctx.id(), vec![email]).await.ok()?;
            stored.first().map(|email| email.meta.size)
        }
        _ => None,
    }
}

async fn check_one(
    ctx: &Ctx<'_>,
    import: &EmailImport,
    blob: Arc<[u8]>,
) -> Result<Checked, SetError> {
    let mailboxes = checked_mailboxes(ctx, &import.mailbox_ids).await?;
    // Parsing a big message is CPU work: off the async workers.
    let parsed = Arc::clone(&blob);
    let is_message = tokio::task::spawn_blocking(move || super::body::parse(&parsed).is_some())
        .await
        .unwrap_or(false);
    if !is_message {
        return Err(SetError::new(SetErrorType::InvalidEmail)
            .with_description("the blob is not an RFC 5322 message"));
    }
    let received_at = import
        .received_at
        .map(|date| UnixTime::from_secs(date.unix_timestamp()));
    Ok(Checked {
        raw: blob,
        mailboxes,
        keywords: import.keywords.clone(),
        received_at,
    })
}

/// Appends a group of checked imports with the same first mailbox.
async fn append_group(
    mail: &MailAccount,
    first: MailboxId,
    group: &[(CreationId, Checked)],
) -> Vec<Result<EmailId, SetError>> {
    if let [(_, one)] = group {
        return vec![
            store_message(
                mail,
                &one.raw,
                &one.mailboxes,
                &one.keywords,
                one.received_at,
            )
            .await,
        ];
    }
    let items: Vec<AppendItem<'_>> = group
        .iter()
        .map(|(_, checked)| AppendItem {
            raw: &checked.raw,
            keywords: &checked.keywords,
            received_at: checked.received_at,
        })
        .collect();
    let appended = match mail.append_many(first, &items).await {
        Ok(list) => list,
        Err(error) => {
            let error = mail_set_error(&error);
            return group.iter().map(|_| Err(error.clone())).collect();
        }
    };
    let mut results = Vec::with_capacity(group.len());
    for ((_, checked), result) in group.iter().zip(appended) {
        let result = match result {
            Ok(appended) if checked.mailboxes.len() > 1 => {
                put_in_all(mail, appended, &checked.mailboxes)
                    .await
                    .map(|()| appended.email)
            }
            Ok(appended) => Ok(appended.email),
            Err(error) => Err(mail_set_error(&error)),
        };
        results.push(result);
    }
    results
}

/// What an import call has done so far.
#[derive(Default)]
struct Outcome {
    created: BTreeMap<CreationId, Email>,
    not_created: BTreeMap<CreationId, SetError>,
}

/// Appends the held groups and empties them (and the shared blobs, whose
/// memory goes back to `lease`).
async fn flush(
    ctx: &mut Ctx<'_>,
    mail: &MailAccount,
    groups: &mut BTreeMap<MailboxId, Vec<(CreationId, Checked)>>,
    held: &mut Held,
    lease: &mut Lease,
    outcome: &mut Outcome,
) {
    for (first, group) in std::mem::take(groups) {
        let results = append_group(mail, first, &group).await;
        for ((creation_id, _), result) in group.into_iter().zip(results) {
            let email = match result {
                Ok(id) => created_properties(ctx, id).await,
                Err(error) => Err(error),
            };
            match email {
                Ok(email) => {
                    // Record it before anything else can fail: the email
                    // is in the mailbox now.
                    if let Some(id) = &email.id {
                        ctx.created.insert(creation_id.clone(), id.clone());
                    }
                    outcome.created.insert(creation_id, email);
                }
                Err(error) => {
                    outcome.not_created.insert(creation_id, error);
                }
            }
        }
    }
    held.clear(lease);
}

/// Loads one blob for an import, counted twice in `lease` (the blob and
/// the copy of the `APPEND` literal). Returns it with what it took.
async fn load_blob(
    ctx: &Ctx<'_>,
    id: &BlobId,
    lease: &mut Lease,
) -> Result<Option<(Arc<[u8]>, u64)>, SetError> {
    let before = lease.bytes();
    let blob = match blob_bytes(ctx, id, lease).await {
        Ok(Some(blob)) => blob,
        other => {
            lease.release(lease.bytes().saturating_sub(before));
            return other.map(|_| None);
        }
    };
    let copy = u64::try_from(blob.len()).unwrap_or(u64::MAX);
    if let Err(error) = lease.reserve(copy).await {
        lease.release(lease.bytes().saturating_sub(before));
        return Err(mail_set_error(&error));
    }
    Ok(Some((blob, lease.bytes().saturating_sub(before))))
}

pub(crate) async fn import(
    ctx: &mut Ctx<'_>,
    args: EmailImportArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let old_state = ctx
        .check_state(DataType::Email, args.if_in_state.as_ref())
        .await?;
    let mail = ctx.mail()?;
    let mut outcome = Outcome::default();
    let mut groups: BTreeMap<MailboxId, Vec<(CreationId, Checked)>> = BTreeMap::new();
    let mut held = Held::default();
    let mut lease = mail.lease();
    let mut budget = Budget::new(ctx.inner.config.max_import_bytes);
    for (creation_id, object) in args.emails {
        let import = match EmailImport::parse(object) {
            Ok(import) => import,
            Err(error) => {
                outcome.not_created.insert(creation_id, error);
                continue;
            }
        };
        let shared = held.blobs.get(&import.blob_id).map(Arc::clone);
        let blob = if let Some(blob) = shared {
            Ok(blob)
        } else {
            let size = known_size(ctx, &import.blob_id).await;
            if size.is_some_and(|size| !budget.fits(&import.blob_id, size)) {
                Err(too_large())
            } else {
                // Known size: append the batch before reading, not after
                // (else both would be held at once).
                let size = size.map(|size| usize::try_from(size).unwrap_or(usize::MAX));
                if size.is_some_and(|size| needs_flush(held.bytes, size)) {
                    flush(ctx, &mail, &mut groups, &mut held, &mut lease, &mut outcome).await;
                }
                match load_blob(ctx, &import.blob_id, &mut lease).await {
                    Ok(Some((blob, reserved)))
                        if budget.take(&import.blob_id, blob.len()).is_err() =>
                    {
                        lease.release(reserved);
                        Err(too_large())
                    }
                    Ok(Some((blob, reserved))) => {
                        if needs_flush(held.bytes, blob.len()) {
                            flush(ctx, &mail, &mut groups, &mut held, &mut lease, &mut outcome)
                                .await;
                        }
                        held.add(import.blob_id.clone(), Arc::clone(&blob), reserved);
                        Ok(blob)
                    }
                    Ok(None) => Err(SetError::blob_not_found(vec![import.blob_id.clone()])),
                    Err(error) => Err(error),
                }
            }
        };
        let checked = match blob {
            Ok(blob) => check_one(ctx, &import, blob).await,
            Err(error) => Err(error),
        };
        match checked {
            Ok(checked) => {
                if let Some(first) = checked.mailboxes.first().copied() {
                    groups
                        .entry(first)
                        .or_default()
                        .push((creation_id, checked));
                }
            }
            Err(error) => {
                outcome.not_created.insert(creation_id, error);
            }
        }
    }
    flush(ctx, &mail, &mut groups, &mut held, &mut lease, &mut outcome).await;
    drop(lease);
    // The emails are in now: a failure here must not hide them. With the
    // old state as the new one, the next /changes gives them again.
    let new_state = match ctx.state(DataType::Email).await {
        Ok(state) => state,
        Err(error) => {
            tracing::error!(error = %error, "Email/import: the new state could not be read");
            old_state.clone()
        }
    };
    Ok(MethodResponse::EmailImport(EmailImportResponse {
        account_id: ctx.jmap_id(),
        old_state: Some(old_state),
        new_state,
        created: outcome.created,
        not_created: outcome.not_created,
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use super::*;

    /// The batches `import` makes for blobs of these sizes (all
    /// different blobs).
    fn batches(sizes: &[usize]) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = Vec::new();
        let mut held = 0_usize;
        for (index, size) in sizes.iter().enumerate() {
            if out.is_empty() || needs_flush(held, *size) {
                out.push(Vec::new());
                held = 0;
            }
            held = held.saturating_add(*size);
            if let Some(last) = out.last_mut() {
                last.push(index);
            }
        }
        out
    }

    #[test]
    fn imports_go_in_batches() {
        const MB: usize = 1024 * 1024;
        assert_eq!(batches(&[50 * MB; 500]).len(), 500);
        assert_eq!(batches(&[MB; 10]), vec![(0..10).collect::<Vec<_>>()]);
        assert_eq!(
            batches(&[MB, MB, 40 * MB, MB]),
            vec![vec![0, 1], vec![2], vec![3]]
        );
        assert_eq!(batches(&[MAX_HELD_BYTES, 1]), vec![vec![0], vec![1]]);
        assert!(!needs_flush(0, usize::MAX));
    }

    /// With 30 MiB held and a 4 MiB blob of known size next, the batch is
    /// appended (and its memory given back) before the blob is read: never
    /// more than `MAX_HELD_BYTES` × 2 + the blob × 2 reserved.
    #[tokio::test]
    async fn flush_before_load() {
        use std::time::Duration;

        const MIB: usize = 1024 * 1024;
        let budget = nebula_mail::MemoryBudget::new(
            256 * 1024 * 1024,
            Duration::from_secs(30),
            Duration::from_secs(30),
        );
        let mut lease = budget.lease();
        let mut held = Held::default();
        let id = |text: &str| text.parse::<BlobId>().unwrap();
        let mut most = 0;
        for (name, size) in [("a", 30 * MIB), ("b", 4 * MIB), ("c", 20 * MIB)] {
            if needs_flush(held.bytes, size) {
                held.clear(&mut lease);
            }
            let reserved = u64::try_from(size * 2).unwrap();
            lease.reserve(reserved).await.unwrap();
            most = most.max(lease.bytes());
            held.add(id(name), Arc::from(vec![0_u8; size]), reserved);
        }
        let bound = u64::try_from(MAX_HELD_BYTES * 2 + 4 * MIB * 2).unwrap();
        assert!(most <= bound, "{most} > {bound}");
        // b and c are held now (24 MiB); a went before b was read.
        assert_eq!(held.bytes, 24 * MIB);
        assert_eq!(lease.bytes(), u64::try_from(48 * MIB).unwrap());
        held.clear(&mut lease);
        assert_eq!(lease.bytes(), 0);
        drop(lease);
        assert_eq!(budget.available(), 256 * 1024 * 1024);
    }

    #[test]
    fn the_budget_counts_each_blob_once() {
        let mut budget = Budget::new(10);
        let id = |text: &str| text.parse::<BlobId>().unwrap();
        budget.take(&id("a"), 6).unwrap();
        assert_eq!(budget.remaining, 4);
        // The same blob again (read again after a batch) is not counted again.
        assert!(budget.fits(&id("a"), 6));
        budget.take(&id("a"), 6).unwrap();
        assert_eq!(budget.remaining, 4);
        assert!(budget.fits(&id("b"), 4) && !budget.fits(&id("b"), 5));
        let error = budget.take(&id("b"), 5).unwrap_err();
        assert_eq!(error.kind(), SetErrorType::TooLarge);
        assert!(budget.take(&id("c"), 4).is_ok());
        assert_eq!(budget.remaining, 0);
    }
}
