// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Identity/set`: names, signatures, `replyTo` and `bcc` can change. New
//! identities must use the account's own address (no aliases:
//! the SMTP server would refuse other senders anyway).

use std::collections::BTreeMap;

use nebula_jmap_proto::method::{SetArguments, SetResponse};
use nebula_jmap_proto::object::{Identity, IdentityCreate, IdentityFields, IdentityPatch};
use nebula_jmap_proto::{IdentityId as JmapIdentityId, MethodResponse, SetError, SetErrorType};
use nebula_store::{DataType, IdentityUpdate, NewIdentity};

use super::{identities, may_use};
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{CallError, store_set_error};

/// Most identities of one account (every one uses the account's own
/// address, so more only cost memory and `Identity/get` time): the
/// store's own cap, checked here first for a clear error.
const MAX_IDENTITIES: usize = 100;

/// `overQuota` once the account has [`MAX_IDENTITIES`].
fn room_for_one(count: usize) -> Result<(), SetError> {
    if count < MAX_IDENTITIES {
        Ok(())
    } else {
        Err(SetError::new(SetErrorType::OverQuota)
            .with_description("too many identities; delete some first"))
    }
}

fn update_of(fields: IdentityFields) -> IdentityUpdate {
    IdentityUpdate {
        name: fields.name,
        reply_to: fields.reply_to,
        bcc: fields.bcc,
        text_signature: fields.text_signature,
        html_signature: fields.html_signature,
    }
}

async fn create_one(
    ctx: &Ctx<'_>,
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<Identity, SetError> {
    let create = IdentityCreate::parse(object)?;
    if !may_use(ctx, &create.email) {
        return Err(SetError::new(SetErrorType::ForbiddenFrom)
            .with_description("only the account's own address can be used"));
    }
    let fields = create.fields;
    let new = NewIdentity {
        name: fields.name.unwrap_or_default(),
        email: create.email.trim().to_owned(),
        reply_to: fields.reply_to.flatten(),
        bcc: fields.bcc.flatten(),
        text_signature: fields.text_signature.unwrap_or_default(),
        html_signature: fields.html_signature.unwrap_or_default(),
        may_delete: true,
    };
    let (id, _) = ctx
        .store()
        .create_identity(ctx.id(), new)
        .await
        .map_err(|e| store_set_error(&e))?;
    Ok(Identity {
        id: Some(ids::identity_id(id).map_err(|_| SetError::new(SetErrorType::Forbidden))?),
        may_delete: Some(true),
        ..Identity::default()
    })
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: SetArguments<JmapIdentityId>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    // Makes sure the default identity exists before comparing states.
    let existing = identities(ctx).await?;
    let old_state = ctx
        .check_state(DataType::Identity, args.if_in_state.as_ref())
        .await?;
    let mut created = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    let mut count = existing.len();
    for (creation_id, object) in args.create {
        let result = match room_for_one(count) {
            Ok(()) => create_one(ctx, object).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(identity) => {
                count = count.saturating_add(1);
                if let Some(id) = &identity.id {
                    ctx.created.insert(creation_id.clone(), id.clone());
                }
                created.insert(creation_id, identity);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }
    let mut updated = BTreeMap::new();
    let mut not_updated = BTreeMap::new();
    for (key, patch) in &args.update {
        let id = ctx.resolve(key);
        let result = match (
            id.as_ref().and_then(ids::store_identity),
            IdentityPatch::parse(patch),
        ) {
            (_, Err(error)) => Err(error),
            (None, Ok(_)) => Err(SetError::not_found()),
            (Some(row), Ok(patch)) => ctx
                .store()
                .update_identity(ctx.id(), row, update_of(patch.fields))
                .await
                .map(|_| ())
                .map_err(|e| store_set_error(&e)),
        };
        match (result, id) {
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
    for key in &args.destroy {
        let id = ctx.resolve(key);
        let row = id.as_ref().and_then(ids::store_identity);
        let deletable =
            row.and_then(|row| existing.iter().find(|i| i.id == row).map(|i| i.may_delete));
        let result = match (row, deletable) {
            (Some(_), Some(false)) => Err(SetError::new(SetErrorType::Forbidden)
                .with_description("the account's main identity can't be deleted")),
            // Created in this same call: deletable.
            (Some(row), Some(true) | None) => ctx
                .store()
                .destroy_identity(ctx.id(), row)
                .await
                .map(|_| ())
                .map_err(|e| store_set_error(&e)),
            (None, _) => Err(SetError::not_found()),
        };
        match (result, id) {
            (Ok(()), Some(id)) => destroyed.push(id),
            (Ok(()), None) => {}
            (Err(error), _) => {
                not_destroyed.insert(key.clone(), error);
            }
        }
    }
    let new_state = ctx.state(DataType::Identity).await?;
    let mut response = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    response.created = created;
    response.not_created = not_created;
    response.updated = updated;
    response.not_updated = not_updated;
    response.destroyed = destroyed;
    response.not_destroyed = not_destroyed;
    Ok(MethodResponse::IdentitySet(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_capped() {
        assert!(room_for_one(0).is_ok());
        // The same caps as the store's.
        assert_eq!(
            i64::try_from(MAX_IDENTITIES).unwrap(),
            nebula_store::MAX_IDENTITIES
        );
        assert_eq!(
            nebula_jmap_proto::object::MAX_IDENTITY_NAME,
            nebula_store::MAX_IDENTITY_NAME
        );
        assert_eq!(
            nebula_jmap_proto::object::MAX_IDENTITY_SIGNATURE,
            nebula_store::MAX_SIGNATURE
        );
        assert_eq!(
            nebula_jmap_proto::object::MAX_IDENTITY_ADDRESSES,
            nebula_store::MAX_IDENTITY_ADDRESSES
        );
        assert!(room_for_one(MAX_IDENTITIES - 1).is_ok());
        assert_eq!(
            room_for_one(MAX_IDENTITIES).unwrap_err().kind(),
            SetErrorType::OverQuota
        );
    }
}
