// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `EmailSubmission/get`: the submissions of the last day.

use nebula_jmap_proto::method::{GetArguments, GetResponse};
use nebula_jmap_proto::object::EmailSubmissionProperty;
use nebula_jmap_proto::{EmailSubmissionId, MethodResponse};
use nebula_store::DataType;

use super::to_jmap;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::CallError;

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<EmailSubmissionId, EmailSubmissionProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let state = ctx.state(DataType::EmailSubmission).await?;
    let wanted: Option<Vec<EmailSubmissionId>> = args
        .ids
        .as_ref()
        .map(|list| list.iter().filter_map(|id| ctx.resolve(id)).collect());
    let rows = wanted
        .as_ref()
        .map(|list| list.iter().filter_map(ids::store_submission).collect());
    let found = ctx.store().submissions(ctx.id(), rows, Ctx::now()?).await?;
    let properties = args.properties.as_deref();
    let signed_out = ctx.config().hold.sends_while_signed_out();
    let list = found
        .iter()
        .map(|s| to_jmap(s, properties, signed_out))
        .collect::<Result<Vec<_>, _>>()?;
    let not_found = wanted
        .unwrap_or_default()
        .into_iter()
        .filter(|id| ids::store_submission(id).is_none_or(|row| !found.iter().any(|s| s.id == row)))
        .collect();
    Ok(MethodResponse::EmailSubmissionGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}
