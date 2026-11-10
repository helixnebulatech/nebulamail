// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Thread/get`.

use nebula_jmap_proto::method::{GetArguments, GetResponse};
use nebula_jmap_proto::object::{Thread, ThreadProperty};
use nebula_jmap_proto::{MethodError, MethodResponse, ThreadId as JmapThreadId};
use nebula_store::DataType;

use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::CallError;

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<JmapThreadId, ThreadProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let state = ctx.state(DataType::Thread).await?;
    let wanted: Vec<JmapThreadId> = match &args.ids {
        Some(wanted) => wanted.iter().filter_map(|id| ctx.resolve(id)).collect(),
        // `ids: null`: every thread, if there are no more than
        // `maxObjectsInGet` (RFC 8620 §5.1); else `requestTooLarge`.
        None => ctx
            .store()
            .thread_ids(ctx.id(), ctx.limits.max_objects_in_get)
            .await?
            .ok_or_else(|| MethodError::request_too_large("too many threads for ids: null"))?
            .into_iter()
            .map(ids::thread_id)
            .collect::<Result<_, _>>()?,
    };
    let rows: Vec<_> = wanted.iter().filter_map(ids::store_thread).collect();
    let found = ctx.store().thread_emails(ctx.id(), rows).await?;
    let wants_emails = args
        .properties
        .as_ref()
        .is_none_or(|list| list.contains(&ThreadProperty::EmailIds));
    let mut list = Vec::with_capacity(found.len());
    for (thread, emails) in &found {
        let email_ids = if wants_emails {
            Some(
                emails
                    .iter()
                    .map(|e| ids::email_id(*e))
                    .collect::<Result<_, _>>()?,
            )
        } else {
            None
        };
        list.push(Thread {
            id: Some(ids::thread_id(*thread)?),
            email_ids,
        });
    }
    let not_found = wanted
        .into_iter()
        .filter(|id| ids::store_thread(id).is_none_or(|row| !found.iter().any(|(t, _)| *t == row)))
        .collect();
    Ok(MethodResponse::ThreadGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}
