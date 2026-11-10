// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Runs the method calls of a request in order (RFC 8620 §3.3): result
//! references are resolved against the responses so far, creation ids are
//! shared across calls, and a failing call only answers itself with an
//! error.

use std::sync::Arc;

use nebula_http::{AccountContext, ApiCall};
use nebula_jmap_proto::{
    CallId, Invocation, MethodCall, MethodError, MethodResponse, ReferenceBudget, Response,
    ResponseInvocation, resolve_result_references,
};

use super::backend::Inner;
use super::context::Ctx;
use crate::error::CallError;
use crate::{
    calendars, contacts, email, identity, mailbox, mdn, principals, push, quota, sieve, submission,
    thread,
};

/// The response to a whole request. Never fails: problems are per call.
pub(crate) async fn run(inner: &Arc<Inner>, account: &AccountContext, call: ApiCall) -> Response {
    let ApiCall {
        request,
        session_state,
        limits,
        languages,
        session,
    } = call;
    let echo_created = request.created_ids.is_some();
    let mut ctx = Ctx {
        inner,
        account,
        created: request.created_ids.clone().unwrap_or_default(),
        limits,
        uses_bridge: request
            .using
            .contains(&nebula_jmap_proto::Capability::Bridge),
        using: request.using.clone(),
        languages,
        scope: crate::sharing::Scope::Own,
        session,
    };
    let mut responses: Vec<ResponseInvocation> = Vec::new();
    // What result references may copy, for the whole request: a request
    // can't make more data than it may send.
    let mut budget = ReferenceBudget::new(limits.max_size_request);
    for invocation in &request.method_calls {
        let call_id = invocation.call_id.clone();
        let answers = if request.is_method_enabled(&invocation.name) {
            one_call(&mut ctx, invocation, &responses, &mut budget).await
        } else {
            Err(MethodError::unknown_method())
        };
        match answers {
            Ok(list) => {
                for answer in list {
                    responses.push(to_invocation(answer, call_id.clone()));
                }
            }
            Err(error) => responses.push(ResponseInvocation::error(&error, call_id)),
        }
    }
    Response {
        method_responses: responses,
        created_ids: echo_created.then_some(ctx.created),
        session_state,
    }
}

fn to_invocation(answer: MethodResponse, call_id: CallId) -> ResponseInvocation {
    let fallback = call_id.clone();
    answer.into_invocation(call_id).unwrap_or_else(|_| {
        tracing::error!("a method response could not be serialized");
        ResponseInvocation::error(&MethodError::server_fail("internal error"), fallback)
    })
}

async fn one_call(
    ctx: &mut Ctx<'_>,
    invocation: &Invocation,
    responses: &[ResponseInvocation],
    budget: &mut ReferenceBudget,
) -> Result<Vec<MethodResponse>, MethodError> {
    let mut arguments = invocation.arguments.clone();
    resolve_result_references(&mut arguments, responses, budget)?;
    let requested = arguments
        .get("accountId")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| id.parse::<nebula_jmap_proto::AccountId>().ok());
    let mut call = MethodCall::parse(&invocation.name, arguments, &ctx.limits)?;
    call.resolve_get_ids(&ctx.created);
    // A groupware call on someone else's shared account runs in its
    // owner's scope (sharing::scope).
    match crate::sharing::scope::handle(ctx, call).await {
        Ok(answers) => Ok(answers),
        Err(error) => {
            Err(
                crate::sharing::scope::unsupported_account(ctx, requested.as_ref(), error)
                    .await
                    .into_method_error(),
            )
        }
    }
}

pub(crate) async fn handle(
    ctx: &mut Ctx<'_>,
    call: MethodCall,
) -> Result<Vec<MethodResponse>, CallError> {
    let one = |response: MethodResponse| vec![response];
    Ok(match call {
        MethodCall::CoreEcho(arguments) => one(MethodResponse::CoreEcho(arguments)),
        MethodCall::MailboxGet(args) => one(mailbox::get::get(ctx, args).await?),
        MethodCall::MailboxChanges(args) => one(mailbox::changes::changes(ctx, args).await?),
        MethodCall::MailboxQuery(args) => one(mailbox::query::query(ctx, args).await?),
        MethodCall::MailboxQueryChanges(args) => {
            one(mailbox::query::query_changes(ctx, args).await?)
        }
        MethodCall::MailboxSet(args) => one(mailbox::set::set(ctx, args).await?),
        MethodCall::EmailGet(args) => one(email::get::get(ctx, args).await?),
        MethodCall::EmailChanges(args) => one(email::changes::changes(ctx, args).await?),
        MethodCall::EmailQuery(args) => one(email::query::query(ctx, args).await?),
        MethodCall::EmailQueryChanges(args) => one(email::query::query_changes(ctx, args).await?),
        MethodCall::EmailSet(args) => one(email::set::set(ctx, args).await?),
        MethodCall::EmailImport(args) => one(email::import::import(ctx, args).await?),
        MethodCall::EmailCopy(args) => one(email::copy::copy(ctx, args).await?),
        MethodCall::EmailParse(args) => one(email::parse::parse(ctx, args).await?),
        MethodCall::BlobCopy(args) => one(crate::blob::copy::copy(ctx, args).await?),
        MethodCall::ThreadGet(args) => one(thread::get::get(ctx, args).await?),
        MethodCall::ThreadChanges(args) => one(thread::changes::changes(ctx, args).await?),
        MethodCall::SearchSnippetGet(args) => one(email::snippet::get(ctx, args).await?),
        MethodCall::IdentityGet(args) => one(identity::get::get(ctx, args).await?),
        MethodCall::IdentityChanges(args) => one(identity::changes::changes(ctx, args).await?),
        MethodCall::IdentitySet(args) => one(identity::set::set(ctx, args).await?),
        MethodCall::EmailSubmissionGet(args) => one(submission::get::get(ctx, args).await?),
        MethodCall::EmailSubmissionQuery(args) => one(submission::query::query(ctx, args).await?),
        MethodCall::EmailSubmissionQueryChanges(args) => {
            one(submission::query::query_changes(ctx, args).await?)
        }
        MethodCall::EmailSubmissionSet(args) => submission::set::set(ctx, args).await?,
        MethodCall::QuotaGet(args) => one(quota::get(ctx, args).await?),
        MethodCall::QuotaChanges(args) => one(quota::changes(ctx, args).await?),
        MethodCall::QuotaQuery(args) => one(quota::query(ctx, args).await?),
        MethodCall::QuotaQueryChanges(args) => one(quota::query_changes(ctx, args).await?),
        MethodCall::MdnSend(args) => mdn::send(ctx, args).await?,
        MethodCall::MdnParse(args) => one(mdn::parse(ctx, args).await?),
        // Contacts over CardDAV.
        call @ (MethodCall::AddressBookGet(_)
        | MethodCall::AddressBookChanges(_)
        | MethodCall::AddressBookSet(_)
        | MethodCall::ContactCardGet(_)
        | MethodCall::ContactCardChanges(_)
        | MethodCall::ContactCardQuery(_)
        | MethodCall::ContactCardQueryChanges(_)
        | MethodCall::ContactCardSet(_)
        | MethodCall::ContactCardParse(_)) => one(contacts::handle(ctx, call).await?),
        MethodCall::CalendarGet(args) => one(calendars::calendar::get(ctx, args).await?),
        MethodCall::CalendarChanges(args) => one(calendars::calendar::changes(ctx, args).await?),
        MethodCall::CalendarSet(args) => one(calendars::set_calendar::set(ctx, args).await?),
        MethodCall::CalendarEventSet(args) => one(calendars::set_event::set(ctx, args).await?),
        MethodCall::CalendarEventGet(args) => one(calendars::event::get(ctx, args).await?),
        MethodCall::CalendarEventChanges(args) => one(calendars::event::changes(ctx, args).await?),
        MethodCall::CalendarEventQuery(args) => one(calendars::query::query(ctx, args).await?),
        MethodCall::CalendarEventQueryChanges(args) => {
            one(calendars::query::query_changes(ctx, args).await?)
        }
        MethodCall::CalendarEventParse(args) => one(calendars::parse::parse(ctx, args).await?),
        MethodCall::CalendarEventCounter(args) => {
            one(calendars::counter::counter(ctx, args).await?)
        }
        MethodCall::CalendarEventDeclineCounter(args) => {
            one(calendars::counter::decline_counter(ctx, args).await?)
        }
        MethodCall::CalendarTrashGet(args) => one(calendars::trash::get(ctx, args).await?),
        MethodCall::CalendarTrashSet(args) => one(calendars::trash::set(ctx, args).await?),
        MethodCall::ParticipantIdentityGet(args) => one(calendars::identity::get(ctx, args).await?),
        // Principals and free-busy over DAV.
        MethodCall::PrincipalGet(args) => one(principals::get::get(ctx, args).await?),
        MethodCall::PrincipalQuery(args) => one(principals::query::query(ctx, args).await?),
        MethodCall::PrincipalChanges(args) => one(MethodResponse::PrincipalChanges(
            principals::changes(ctx, args)?,
        )),
        MethodCall::PrincipalGetAvailability(args) => {
            one(principals::availability::get_availability(ctx, args).await?)
        }
        // Sieve scripts and the vacation response over ManageSieve.
        MethodCall::SieveScriptGet(args) => one(sieve::script::get(ctx, args).await?),
        MethodCall::SieveScriptChanges(args) => one(sieve::script::changes(ctx, args).await?),
        MethodCall::SieveScriptQuery(args) => one(sieve::script::query(ctx, args).await?),
        MethodCall::SieveScriptSet(args) => one(sieve::script::set(ctx, args).await?),
        MethodCall::SieveScriptValidate(args) => one(sieve::script::validate(ctx, args).await?),
        MethodCall::VacationResponseGet(args) => one(sieve::vacation::get(ctx, args).await?),
        MethodCall::VacationResponseSet(args) => one(sieve::vacation::set(ctx, args).await?),
        // Web Push (RFC 8620 §7.2).
        MethodCall::PushSubscriptionGet(args) => one(push::methods::get(ctx, args).await?),
        MethodCall::PushSubscriptionSet(args) => one(push::methods::set(ctx, args).await?),
        // Any method this crate doesn't know.
        _ => return Err(MethodError::unknown_method().into()),
    })
}
