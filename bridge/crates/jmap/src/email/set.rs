// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/set` (RFC 8621 §4.6): create (build the message and `APPEND` it),
//! update keywords and mailboxes (`STORE`, `MOVE`, `COPY`, `EXPUNGE`),
//! destroy (expunge every copy). Objects are done one by one: a failure
//! only fails that object.

use std::collections::{BTreeMap, BTreeSet};

use nebula_jmap_proto::method::{PatchObject, SetArguments, SetResponse};
use nebula_jmap_proto::object::Email;
use nebula_jmap_proto::object::email::{EmailCreate, EmailPatch, SetPatch};
use nebula_jmap_proto::{
    CreationId, EmailId as JmapEmailId, IdOrRef, MailboxId as JmapMailboxId, MethodResponse,
    SetError, SetErrorType, UnsignedInt,
};
use nebula_mail::{Appended, Discarded, KeywordChange, MailAccount};
use nebula_store::{DataType, EmailId, MailboxId, UnixTime};

use super::build::{self, Blobs};
use super::{checked_mailboxes, mailbox_rows};
use crate::blob::BlobRef;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{CallError, mail_set_error, store_set_error};

fn mailboxes_error(unknown: &str) -> SetError {
    SetError::invalid_properties(["mailboxIds"])
        .with_description(format!("unknown mailbox {unknown}"))
}

/// The server-set properties of a new email: `id`, `blobId`, `threadId`,
/// `size`.
pub(crate) async fn created_properties(ctx: &Ctx<'_>, id: EmailId) -> Result<Email, SetError> {
    let email = ctx
        .store()
        .emails(ctx.id(), vec![id])
        .await
        .map_err(|e| store_set_error(&e))?
        .pop()
        .ok_or_else(SetError::not_found)?;
    let bug = |_| SetError::new(SetErrorType::Forbidden).with_description("internal error");
    Ok(Email {
        id: Some(ids::email_id(id).map_err(bug)?),
        blob_id: Some(BlobRef::Message(id).to_blob_id().map_err(bug)?),
        thread_id: Some(ids::thread_id(email.thread).map_err(bug)?),
        size: Some(UnsignedInt::saturating(email.meta.size)),
        ..Email::default()
    })
}

/// Appends `raw` to the first mailbox and puts the email in the others.
pub(crate) async fn store_message(
    mail: &MailAccount,
    raw: &[u8],
    mailboxes: &BTreeSet<MailboxId>,
    keywords: &BTreeSet<nebula_jmap_proto::Keyword>,
    received_at: Option<UnixTime>,
) -> Result<EmailId, SetError> {
    let first = *mailboxes
        .first()
        .ok_or_else(|| SetError::invalid_properties(["mailboxIds"]))?;
    let appended = mail
        .append(first, raw, keywords, received_at)
        .await
        .map_err(|e| mail_set_error(&e))?;
    if mailboxes.len() > 1 {
        put_in_all(mail, appended, mailboxes).await?;
    }
    Ok(appended.email)
}

/// Puts a just-appended email in all its mailboxes. If that fails, the
/// email is taken out again (best effort), so a create is whole or not
/// at all; but only if the APPEND surely made it: a server that
/// deduplicates (Gmail) may have answered with an email the user already
/// had, and removing that would lose it ([`MailAccount::discard_appended`]).
pub(crate) async fn put_in_all(
    mail: &MailAccount,
    appended: Appended,
    mailboxes: &BTreeSet<MailboxId>,
) -> Result<(), SetError> {
    let Err(error) = mail.set_mailboxes(appended.email, mailboxes.clone()).await else {
        return Ok(());
    };
    match mail.discard_appended(appended, mailboxes).await {
        Ok(Discarded::Removed) => {}
        Ok(Discarded::Kept) => tracing::warn!(
            email = %appended.email,
            "a half-created email was kept: it may be one the user already had"
        ),
        Err(undo) => tracing::warn!(error = %undo, "a half-created email could not be removed"),
    }
    Err(mail_set_error(&error))
}

async fn create_one(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<(EmailId, Email), SetError> {
    let create = EmailCreate::parse(object)?;
    let mailboxes = checked_mailboxes(ctx, &create.mailbox_ids).await?;
    // One lease for the create: its blobs, the message being built, and
    // then the message and its copy in the APPEND.
    let mut lease = mail.lease();
    let blobs: Blobs = build::load_blobs(ctx, &create, &mut lease).await?;
    let now = Ctx::now().map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    // The output of the build lives next to the blobs while it is built:
    // counted before (cached blobs were not counted at all).
    let estimate = build::estimated_size(&create, &blobs);
    lease
        .reserve(estimate)
        .await
        .map_err(|e| mail_set_error(&e))?;
    let keywords = create.keywords.clone();
    let received_at = create
        .received_at
        .map(|date| UnixTime::from_secs(date.unix_timestamp()));
    // mail-builder writes up to ~70 MB of base64: off the async workers.
    let raw = tokio::task::spawn_blocking(move || build::build(&create, &blobs, now.secs()))
        .await
        .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("internal error"))??;
    let size = u64::try_from(raw.len()).unwrap_or(u64::MAX);
    if size > ctx.config().max_size_attachments {
        return Err(SetError::new(SetErrorType::TooLarge));
    }
    // The blobs are gone: the message and the copy of the APPEND literal.
    lease
        .resize(size.saturating_mul(2))
        .await
        .map_err(|e| mail_set_error(&e))?;
    lease.drop_turn();
    let id = store_message(mail, &raw, &mailboxes, &keywords, received_at).await?;
    drop(lease);
    Ok((id, created_properties(ctx, id).await?))
}

fn resolve_rows(
    ctx: &Ctx<'_>,
    list: &BTreeSet<IdOrRef<JmapMailboxId>>,
) -> Result<BTreeSet<MailboxId>, SetError> {
    mailbox_rows(ctx, list).map_err(|id| mailboxes_error(&id))
}

/// Applies a patch: keywords first, then mailboxes (a move changes the
/// copies the keywords are stored on, so both orders work; this one keeps
/// the keyword write on the copy that is already there).
pub(crate) async fn update_one(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    id: EmailId,
    patch: &PatchObject,
) -> Result<(), SetError> {
    let patch = EmailPatch::parse(patch)?;
    let current = ctx
        .store()
        .emails(ctx.id(), vec![id])
        .await
        .map_err(|e| store_set_error(&e))?
        .pop()
        .ok_or_else(SetError::not_found)?;
    let target_mailboxes = match &patch.mailbox_ids {
        None => None,
        Some(SetPatch::Replace(set)) => Some(resolve_rows(ctx, set)?),
        Some(SetPatch::Change { add, remove }) => {
            let add = resolve_rows(ctx, add)?;
            let remove = resolve_rows(ctx, remove)?;
            let mut target = current.mailboxes.clone();
            target.retain(|m| !remove.contains(m));
            target.extend(add);
            Some(target)
        }
        Some(_) => return Err(SetError::invalid_patch("unknown patch")),
    };
    if target_mailboxes.as_ref().is_some_and(BTreeSet::is_empty) {
        return Err(SetError::invalid_properties(["mailboxIds"])
            .with_description("an email must stay in at least one mailbox"));
    }
    if let Some(keywords) = patch.keywords {
        let change = match keywords {
            SetPatch::Replace(set) => KeywordChange::Set(set),
            SetPatch::Change { add, remove } => KeywordChange::Patch { add, remove },
            _ => return Err(SetError::invalid_patch("unknown patch")),
        };
        mail.update_keywords(id, change)
            .await
            .map_err(|e| mail_set_error(&e))?;
    }
    if let Some(target) = target_mailboxes
        && target != current.mailboxes
    {
        mail.set_mailboxes(id, target)
            .await
            .map_err(|e| mail_set_error(&e))?;
    }
    Ok(())
}

pub(crate) async fn destroy_one(mail: &MailAccount, id: EmailId) -> Result<(), SetError> {
    mail.destroy(id)
        .await
        .map(|_| ())
        .map_err(|e| mail_set_error(&e))
}

/// The results of the updates and destroys of one call.
#[derive(Default)]
pub(crate) struct Changed {
    pub(crate) updated: BTreeMap<JmapEmailId, Option<Email>>,
    pub(crate) not_updated: BTreeMap<IdOrRef<JmapEmailId>, SetError>,
    pub(crate) destroyed: Vec<JmapEmailId>,
    pub(crate) not_destroyed: BTreeMap<IdOrRef<JmapEmailId>, SetError>,
}

/// Runs updates and destroys (also used by `EmailSubmission/set`'s
/// `onSuccessUpdateEmail` / `onSuccessDestroyEmail`).
pub(crate) async fn update_and_destroy(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    update: &BTreeMap<IdOrRef<JmapEmailId>, PatchObject>,
    destroy: &[IdOrRef<JmapEmailId>],
) -> Changed {
    let mut changed = Changed::default();
    for (key, patch) in update {
        let id = ctx.resolve(key);
        let result = match id.as_ref().and_then(ids::store_email) {
            Some(row) => update_one(ctx, mail, row, patch).await,
            None => Err(SetError::not_found()),
        };
        match (result, id) {
            (Ok(()), Some(id)) => {
                changed.updated.insert(id, None);
            }
            (Ok(()), None) => {}
            (Err(error), _) => {
                changed.not_updated.insert(key.clone(), error);
            }
        }
    }
    for key in destroy {
        let id = ctx.resolve(key);
        let result = match id.as_ref().and_then(ids::store_email) {
            Some(row) => destroy_one(mail, row).await,
            None => Err(SetError::not_found()),
        };
        match (result, id) {
            (Ok(()), Some(id)) => changed.destroyed.push(id),
            (Ok(()), None) => {}
            (Err(error), _) => {
                changed.not_destroyed.insert(key.clone(), error);
            }
        }
    }
    changed
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: SetArguments<JmapEmailId>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let old_state = ctx
        .check_state(DataType::Email, args.if_in_state.as_ref())
        .await?;
    let mail = ctx.mail()?;
    let mut created: BTreeMap<CreationId, Email> = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    for (creation_id, object) in args.create {
        match create_one(ctx, &mail, object).await {
            Ok((id, object)) => {
                ctx.created.insert(creation_id.clone(), ids::email_id(id)?);
                created.insert(creation_id, object);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }
    let changed = update_and_destroy(ctx, &mail, &args.update, &args.destroy).await;
    let new_state = ctx.state(DataType::Email).await?;
    let mut response = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    response.created = created;
    response.not_created = not_created;
    response.updated = changed.updated;
    response.not_updated = changed.not_updated;
    response.destroyed = changed.destroyed;
    response.not_destroyed = changed.not_destroyed;
    Ok(MethodResponse::EmailSet(response))
}
