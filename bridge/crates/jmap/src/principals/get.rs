// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Principal/get` (RFC 9670 §3.1). Without `ids`, every principal the
//! server shows (the user's own first); with `ids`, the ones seen before
//! (in this login: discovery, listings and searches), else the server's
//! listing is read once more; the rest are `notFound`. An address given
//! an id of its own (`super::Who::Address`, servers that share by
//! address) is found by its id while the bridge remembers it.
//!
//! `accounts` names the user's own account for their principal and, for
//! someone who shares calendars or address books with the user, their
//! shared account (`a{n}-r{m}`, also the calendars' `accountId`).
//! `mayShareWith`: one of the user's calendars can be shared with them.

use std::collections::HashSet;

use nebula_jmap_proto::method::{GetArguments, GetResponse};
use nebula_jmap_proto::object::PrincipalProperty;
use nebula_jmap_proto::{MethodResponse, PrincipalId};

use super::{Who, ids};
use crate::core::context::Ctx;
use crate::error::CallError;

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<PrincipalId, PrincipalProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let running = super::account(ctx)?;
    let properties = args.properties.as_deref();
    let wants_extras = properties.is_none_or(|list| {
        list.contains(&PrincipalProperty::Accounts)
            || list.contains(&PrincipalProperty::Capabilities)
            || list.contains(&PrincipalProperty::TimeZone)
    });
    let extras = if wants_extras {
        super::Extras::load(ctx, &running).await?
    } else {
        super::Extras::default()
    };
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &args.ids {
        None => {
            let all = running
                .principals()
                .await
                .map_err(|e| super::groupware_error(&e))?;
            for principal in all {
                let who = Who::Dav(principal);
                list.push(super::to_jmap(ctx, &running, &extras, &who, properties)?);
            }
        }
        Some(wanted) => {
            let mut listed = false;
            let mut done: HashSet<PrincipalId> = HashSet::new();
            for id in wanted.iter().filter_map(|id| ctx.resolve(id)) {
                if !done.insert(id.clone()) {
                    continue;
                }
                let Some(number) = ids::principal_number(&id) else {
                    not_found.push(id);
                    continue;
                };
                // One lookup per id (an index, not a scan), and a listing
                // at most once per call for the ids not seen yet.
                let mut found = running.known_principal(number);
                if found.is_none()
                    && let Some(address) = running.known_address(number)
                {
                    let who = Who::Address(address);
                    list.push(super::to_jmap(ctx, &running, &extras, &who, properties)?);
                    continue;
                }
                if found.is_none() && !listed {
                    listed = true;
                    running
                        .principals()
                        .await
                        .map_err(|e| super::groupware_error(&e))?;
                    found = running.known_principal(number);
                }
                match found {
                    Some(principal) => {
                        let who = Who::Dav(principal);
                        list.push(super::to_jmap(ctx, &running, &extras, &who, properties)?);
                    }
                    None => not_found.push(id),
                }
            }
        }
    }
    let state = super::state_of(ctx, &running.known_principals());
    Ok(MethodResponse::PrincipalGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}
