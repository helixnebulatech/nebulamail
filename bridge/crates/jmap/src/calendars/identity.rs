// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `ParticipantIdentity/get`: who the user is in events. One identity per
//! address, as Stalwart: the account's own address first (the default),
//! then the `mailto:` addresses of the DAV server's
//! `calendar-user-address-set`. The name is that of the mail identity with
//! the same address, if it has one. Unlike Stalwart, the answer has a
//! `state` (its changes are recorded at each discovery).

use nebula_groupware::model::calendar::participant_number;
use nebula_jmap_proto::method::{GetArguments, GetResponse};
use nebula_jmap_proto::object::{ParticipantIdentity, ParticipantIdentityProperty};
use nebula_jmap_proto::{MethodResponse, ParticipantIdentityId};
use nebula_store::DataType;

use super::ids;
use crate::core::context::Ctx;
use crate::error::CallError;
use crate::identity::identities;

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<ParticipantIdentityId, ParticipantIdentityProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let running = super::account(ctx)?;
    let state = ctx.state(DataType::ParticipantIdentity).await?;
    let mail_identities = identities(ctx).await?;
    let own = super::own_addresses(ctx, &running);
    let wants = |p| {
        args.properties
            .as_ref()
            .is_none_or(|list: &Vec<ParticipantIdentityProperty>| list.contains(&p))
    };
    let mut all = Vec::with_capacity(own.len());
    for (index, address) in own.iter().enumerate() {
        let uri = format!("mailto:{address}");
        let id = ids::identity_id(participant_number(&uri))?;
        let mut out = ParticipantIdentity {
            id: Some(id),
            ..ParticipantIdentity::default()
        };
        if wants(ParticipantIdentityProperty::Name) {
            let name = mail_identities
                .iter()
                .find(|identity| identity.email.eq_ignore_ascii_case(address))
                .map(|identity| identity.name.clone())
                .unwrap_or_default();
            out.name = Some(name);
        }
        if wants(ParticipantIdentityProperty::CalendarAddress) {
            out.calendar_address = Some(uri);
        }
        if wants(ParticipantIdentityProperty::IsDefault) {
            out.is_default = Some(index == 0);
        }
        all.push(out);
    }
    let (list, not_found) = match &args.ids {
        None => (all, Vec::new()),
        Some(wanted) => {
            let mut list = Vec::new();
            let mut not_found = Vec::new();
            for id in wanted.iter().filter_map(|id| ctx.resolve(id)) {
                let found = ids::identity_number(&id)
                    .and_then(|_| all.iter().find(|i| i.id.as_ref() == Some(&id)));
                match found {
                    Some(identity) if !list.contains(identity) => list.push(identity.clone()),
                    None if !not_found.contains(&id) => not_found.push(id),
                    Some(_) | None => {}
                }
            }
            (list, not_found)
        }
    };
    Ok(MethodResponse::ParticipantIdentityGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}
