// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Identity/get`.

use nebula_jmap_proto::method::{GetArguments, GetResponse};
use nebula_jmap_proto::object::{Identity, IdentityProperty};
use nebula_jmap_proto::{IdentityId as JmapIdentityId, MethodResponse};
use nebula_store::{DataType, Identity as StoredIdentity};

use super::identities;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

fn to_jmap(
    identity: &StoredIdentity,
    properties: Option<&[IdentityProperty]>,
) -> Result<Identity, Bug> {
    let wants = |p| properties.is_none_or(|list| list.contains(&p));
    let mut out = Identity {
        id: Some(ids::identity_id(identity.id)?),
        ..Identity::default()
    };
    if wants(IdentityProperty::Name) {
        out.name = Some(identity.name.clone());
    }
    if wants(IdentityProperty::Email) {
        out.email = Some(identity.email.clone());
    }
    if wants(IdentityProperty::ReplyTo) {
        out.reply_to = Some(identity.reply_to.clone());
    }
    if wants(IdentityProperty::Bcc) {
        out.bcc = Some(identity.bcc.clone());
    }
    if wants(IdentityProperty::TextSignature) {
        out.text_signature = Some(identity.text_signature.clone());
    }
    if wants(IdentityProperty::HtmlSignature) {
        out.html_signature = Some(identity.html_signature.clone());
    }
    if wants(IdentityProperty::MayDelete) {
        out.may_delete = Some(identity.may_delete);
    }
    Ok(out)
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<JmapIdentityId, IdentityProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    // The default identity may be created here, which moves the state:
    // make sure of it first, then read the state, then the list. A change
    // in between makes the list newer than the state, and the next
    // /changes gives it again (safe); the other order could lose it.
    identities(ctx).await?;
    let state = ctx.state(DataType::Identity).await?;
    let all = identities(ctx).await?;
    let properties = args.properties.as_deref();
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &args.ids {
        None => {
            for identity in &all {
                list.push(to_jmap(identity, properties)?);
            }
        }
        Some(wanted) => {
            for id in wanted.iter().filter_map(|id| ctx.resolve(id)) {
                let found =
                    ids::store_identity(&id).and_then(|row| all.iter().find(|i| i.id == row));
                match found {
                    Some(identity) => list.push(to_jmap(identity, properties)?),
                    None => not_found.push(id),
                }
            }
        }
    }
    Ok(MethodResponse::IdentityGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}
