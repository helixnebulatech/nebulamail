// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Mailbox/set`: IMAP `CREATE`, `RENAME`, `DELETE` and `(UN)SUBSCRIBE`.
//!
//! - Roles come from the server (SPECIAL-USE or the name), so a role asked
//!   for on create is only a wish: the response says which role the new
//!   mailbox really has. Changing a role later is refused.
//! - `sortOrder` is computed by the bridge; changing it is refused.

use std::collections::BTreeMap;

use nebula_jmap_proto::method::SetResponse;
use nebula_jmap_proto::object::{Mailbox, MailboxCreate, MailboxPatch, MailboxSetArguments};
use nebula_jmap_proto::{
    CreationId, IdOrRef, MailboxId as JmapMailboxId, MethodResponse, SetError, SetErrorType,
};
use nebula_mail::{MailAccount, OnDestroy};
use nebula_store::{DataType, Mailbox as StoredMailbox, MailboxId};

use super::get::to_jmap;
use super::jmap_role;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{CallError, mail_set_error, store_set_error};

/// A `parentId` that may point at a mailbox created earlier.
fn parent_row(
    ctx: &Ctx<'_>,
    parent: Option<&IdOrRef<JmapMailboxId>>,
) -> Result<Option<MailboxId>, SetError> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    ctx.resolve(parent)
        .as_ref()
        .and_then(ids::store_mailbox)
        .map(Some)
        .ok_or_else(|| {
            SetError::invalid_properties(["parentId"]).with_description("unknown parent mailbox")
        })
}

async fn stored(ctx: &Ctx<'_>, id: MailboxId) -> Result<StoredMailbox, SetError> {
    ctx.store()
        .mailbox(ctx.id(), id)
        .await
        .map_err(|e| store_set_error(&e))
}

async fn create_one(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<(MailboxId, Mailbox), SetError> {
    let create = MailboxCreate::parse(object)?;
    let parent = parent_row(ctx, create.parent_id.as_ref())?;
    let id = mail
        .create_mailbox(&create.name, parent)
        .await
        .map_err(|e| mail_set_error(&e))?;
    if create.is_subscribed != Some(false)
        && let Err(error) = mail.set_subscribed(id, true).await
    {
        tracing::debug!(error = %error, "could not subscribe to the new mailbox");
    }
    let mailbox = stored(ctx, id).await?;
    if let Some(role) = create.role
        && mailbox.role.and_then(jmap_role) != Some(role)
    {
        tracing::debug!(%role, "role asked for on create not given by the server");
    }
    let object = to_jmap(&mailbox, nebula_store::MailboxCounts::default(), None)
        .map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    Ok((id, object))
}

async fn update_one(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    id: MailboxId,
    patch: &nebula_jmap_proto::method::PatchObject,
) -> Result<(), SetError> {
    let patch = MailboxPatch::parse(patch)?;
    let current = stored(ctx, id).await?;
    if let Some(role) = &patch.role
        && *role != current.role.and_then(jmap_role)
    {
        return Err(SetError::new(SetErrorType::Forbidden)
            .with_description("roles come from the mail server and can't be changed"));
    }
    if let Some(order) = patch.sort_order
        && order.get() != u64::from(current.sort_order)
    {
        return Err(SetError::new(SetErrorType::Forbidden)
            .with_description("sortOrder can't be changed on this server"));
    }
    let new_parent = match &patch.parent_id {
        Some(parent) => parent_row(ctx, parent.as_ref())?,
        None => current.parent,
    };
    let new_name = patch.name.clone().unwrap_or_else(|| current.name.clone());
    if new_parent != current.parent || new_name != current.name {
        mail.rename_mailbox(id, &new_name, new_parent)
            .await
            .map_err(|e| mail_set_error(&e))?;
    }
    if let Some(subscribed) = patch.is_subscribed
        && subscribed != current.subscribed
    {
        mail.set_subscribed(id, subscribed)
            .await
            .map_err(|e| mail_set_error(&e))?;
    }
    Ok(())
}

async fn destroy_one(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    id: MailboxId,
    remove_emails: bool,
) -> Result<(), SetError> {
    let current = stored(ctx, id).await?;
    let mailboxes = ctx
        .store()
        .mailboxes(ctx.id())
        .await
        .map_err(|e| store_set_error(&e))?;
    if mailboxes.iter().any(|m| m.parent == Some(current.id)) {
        return Err(SetError::new(SetErrorType::MailboxHasChild));
    }
    let counts = ctx
        .store()
        .mailbox_counts(ctx.id())
        .await
        .map_err(|e| store_set_error(&e))?;
    let has_emails = counts.get(&id).is_some_and(|c| c.total_emails > 0);
    if has_emails && !remove_emails {
        return Err(SetError::new(SetErrorType::MailboxHasEmail));
    }
    if current.imap_name.is_inbox() {
        return Err(
            SetError::new(SetErrorType::Forbidden).with_description("INBOX can't be deleted")
        );
    }
    // The local count is only a shortcut: the mail layer asks the server,
    // which may hold messages not synced yet.
    let emails = if remove_emails {
        OnDestroy::RemoveEmails
    } else {
        OnDestroy::KeepEmails
    };
    mail.delete_mailbox(id, emails)
        .await
        .map(|_| ())
        .map_err(|e| mail_set_error(&e))
}

/// The creation id a create's `parentId` references (`"#p"` → `p`).
fn parent_reference(object: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    object
        .get("parentId")
        .and_then(serde_json::Value::as_str)
        .and_then(|parent| parent.strip_prefix('#'))
}

/// The creates in an order where a mailbox created in this same call comes
/// before the ones whose `parentId` references it: "the server MUST order
/// the creates and updates within a single method call so that creates
/// happen before their creation ids are referenced by another
/// create/update/destroy in the same call" (RFC 8620 §5.3). Otherwise by
/// creation id. A cycle, or a reference to a creation id that isn't here,
/// keeps its place and fails later as an unknown parent.
fn parents_first(
    mut pending: BTreeMap<CreationId, serde_json::Map<String, serde_json::Value>>,
) -> Vec<(CreationId, serde_json::Map<String, serde_json::Value>)> {
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let ready: Vec<CreationId> = pending
            .iter()
            .filter(|(_, object)| {
                parent_reference(object)
                    .is_none_or(|parent| !pending.keys().any(|other| other.as_str() == parent))
            })
            .map(|(id, _)| id.clone())
            .collect();
        if ready.is_empty() {
            // A cycle: the rest as they are.
            ordered.extend(pending);
            break;
        }
        for id in ready {
            if let Some(object) = pending.remove(&id) {
                ordered.push((id, object));
            }
        }
    }
    ordered
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: MailboxSetArguments,
) -> Result<MethodResponse, CallError> {
    let set = args.set;
    ctx.check_account(&set.account_id)?;
    let old_state = ctx
        .check_state(DataType::Mailbox, set.if_in_state.as_ref())
        .await?;
    let mail = ctx.mail()?;
    let mut created: BTreeMap<CreationId, Mailbox> = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    for (creation_id, object) in parents_first(set.create) {
        match create_one(ctx, &mail, object).await {
            Ok((id, object)) => {
                ctx.created
                    .insert(creation_id.clone(), ids::mailbox_id(id)?);
                created.insert(creation_id, object);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }
    let mut updated = BTreeMap::new();
    let mut not_updated = BTreeMap::new();
    for (key, patch) in &set.update {
        let row = ctx.resolve(key).as_ref().and_then(ids::store_mailbox);
        let result = match row {
            Some(row) => update_one(ctx, &mail, row, patch).await,
            None => Err(SetError::not_found()),
        };
        match (result, ctx.resolve(key)) {
            (Ok(()), Some(id)) => {
                updated.insert(id, None);
            }
            (Ok(()), None) => {}
            (Err(error), _) => {
                not_updated.insert(key.clone(), error);
            }
        }
    }
    let mut destroyed = Vec::new();
    let mut not_destroyed = BTreeMap::new();
    for key in &set.destroy {
        let id = ctx.resolve(key);
        let row = id.as_ref().and_then(ids::store_mailbox);
        let result = match row {
            Some(row) => destroy_one(ctx, &mail, row, args.on_destroy_remove_emails).await,
            None => Err(SetError::not_found()),
        };
        match (result, id) {
            (Ok(()), Some(id)) => destroyed.push(id),
            (Ok(()), None) => {}
            (Err(error), _) => {
                not_destroyed.insert(key.clone(), error);
            }
        }
    }
    let new_state = ctx.state(DataType::Mailbox).await?;
    let mut response = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    response.created = created;
    response.not_created = not_created;
    response.updated = updated;
    response.not_updated = not_updated;
    response.destroyed = destroyed;
    response.not_destroyed = not_destroyed;
    Ok(MethodResponse::MailboxSet(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creates(
        items: &[(&str, Option<&str>)],
    ) -> BTreeMap<CreationId, serde_json::Map<String, serde_json::Value>> {
        items
            .iter()
            .map(|(id, parent)| {
                let mut object = serde_json::Map::new();
                object.insert("name".to_owned(), serde_json::Value::from(*id));
                if let Some(parent) = parent {
                    object.insert("parentId".to_owned(), serde_json::Value::from(*parent));
                }
                (id.parse().unwrap(), object)
            })
            .collect()
    }

    fn order(items: &[(&str, Option<&str>)]) -> Vec<String> {
        parents_first(creates(items))
            .into_iter()
            .map(|(id, _)| id.as_str().to_owned())
            .collect()
    }

    /// RFC 8620 §5.3: creates before their creation ids are referenced.
    #[test]
    fn parents_are_created_first() {
        // "a" sorts before "p", but its parent is "#p"; "z" under "a".
        assert_eq!(
            order(&[("a", Some("#p")), ("p", None), ("z", Some("#a"))]),
            ["p", "a", "z"]
        );
        // A real id, or a creation id not in this call: as it comes.
        assert_eq!(
            order(&[("b", Some("Mx1")), ("a", Some("#elsewhere"))]),
            ["a", "b"]
        );
        // A cycle doesn't loop: both stay, and fail as unknown parents.
        assert_eq!(
            order(&[("a", Some("#b")), ("b", Some("#a")), ("c", None)]),
            ["c", "a", "b"]
        );
    }
}
