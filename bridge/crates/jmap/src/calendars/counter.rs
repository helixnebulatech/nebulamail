// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `CalendarEvent/counter` and `CalendarEvent/declineCounter`, the
//! bridge's own methods (capability `https://helixnebula.tech/jmap/bridge`)
//! a proposal of another time (iTIP
//! `COUNTER`) and the organizer's refusal (`DECLINECOUNTER`), built from
//! the stored event or the email (`nebula_groupware::model::counter`)
//! and mailed like the bridge's invitations (`set_itip`: from the
//! identity with the sender's address, in the user's language, a copy in
//! Sent as the web kept it). Nothing is written to the calendar.
//!
//! The web uses them only with the bridge; with Stalwart it builds and
//! mails the same messages itself (Stalwart does not process `COUNTER`).
//! Errors carry no content (titles, addresses).

use std::sync::Arc;

use nebula_groupware::model::counter::{
    CounterError, CounterSource, Proposal, counter as build_counter,
    decline_counter as build_decline,
};
use nebula_groupware::model::itip::ItipMessage;
use nebula_jmap_proto::object::{
    CalendarEventCounterArguments, CalendarEventDeclineCounterArguments, CalendarItipSentResponse,
    CounterSubject,
};
use nebula_jmap_proto::{BlobId, CalendarEventId, MethodError, MethodErrorType, MethodResponse};
use nebula_store::CollectionKind;

use super::ids::{self, EventRef};
use super::set_event::{Occurrence, event_text, occurrence_kind};
use super::set_itip::{Occasion, send_all, sender};
use crate::core::context::Ctx;
use crate::email::build::blob_bytes;
use crate::error::CallError;

fn refused(error: &CounterError) -> CallError {
    let description = match error {
        CounterError::Convert(_) => "the event can't be read",
        CounterError::NotInvited => "the user is not invited to this event",
        CounterError::NoOccurrence => "the event has no such occurrence",
        CounterError::NotOrganizer => "the user does not organize this event",
        CounterError::NotACounter => "the email is not a proposal for this event",
        CounterError::NotAGuest => "the proposal is not from a guest of this event",
        _ => "the proposal can't be made",
    };
    MethodError::invalid_arguments(description).into()
}

fn not_found(what: &str) -> CallError {
    MethodError::new(MethodErrorType::NotFound)
        .with_description(format!("{what} not found"))
        .into()
}

/// The stored text of `id`, and its occurrence (`None`: the whole event,
/// also for an occurrence id of an event without repetition).
async fn stored(
    ctx: &Ctx<'_>,
    id: &CalendarEventId,
) -> Result<
    (
        String,
        Option<nebula_groupware::model::calendar::RecurrenceKey>,
    ),
    CallError,
> {
    let which = ids::parse_event(id).ok_or_else(|| not_found("eventId"))?;
    let dav = super::account(ctx)?;
    let found = ctx
        .store()
        .visible_resource(ctx.id(), CollectionKind::Calendar, which.resource())
        .await
        .map_err(|_| MethodError::server_fail("database error"))?;
    // Only an event of the call's scope (own or the shared account named).
    let event = crate::sharing::scope::resource(ctx, CollectionKind::Calendar, found)
        .await?
        .ok_or_else(|| not_found("eventId"))?;
    let text = event_text(&dav, &event)
        .await
        .map_err(|_| not_found("eventId"))?
        .text;
    let occurrence = match which {
        EventRef::Base(_) => None,
        EventRef::Occurrence(_, key) => {
            match occurrence_kind(text.clone(), key)
                .await?
                .map_err(|e| refused(&CounterError::Convert(e)))?
            {
                Occurrence::Whole => None,
                Occurrence::One(key) => Some(key),
                Occurrence::Missing => return Err(not_found("eventId")),
            }
        }
    };
    Ok((text, occurrence))
}

async fn blob(ctx: &Ctx<'_>, id: &BlobId) -> Result<Vec<u8>, CallError> {
    // Copied out right away: the lease only covers the load.
    let mut lease = crate::email::build::lease(ctx)
        .map_err(|_| MethodError::new(MethodErrorType::ServerUnavailable))?;
    match blob_bytes(ctx, id, &mut lease).await {
        Ok(Some(bytes)) => Ok(bytes.to_vec()),
        Ok(None) => Err(not_found("blobId")),
        Err(_) => Err(MethodError::new(MethodErrorType::ServerUnavailable).into()),
    }
}

/// Mails the message from the identity with its sender's address.
async fn send(ctx: &Ctx<'_>, message: ItipMessage) -> Result<MethodResponse, CallError> {
    let from = sender(ctx, &message.organizer).await.map_err(|_| {
        MethodError::new(MethodErrorType::Forbidden)
            .with_description("no identity has the sender's address")
    })?;
    let method = message.method;
    if send_all(
        ctx,
        &from,
        std::slice::from_ref(&message),
        Occasion::Changed,
    )
    .await
        == 0
    {
        return Err(MethodError::server_fail("the email was not sent").into());
    }
    let response = CalendarItipSentResponse {
        account_id: ctx.jmap_id(),
    };
    Ok(match method {
        nebula_groupware::model::itip::Method::Counter => {
            MethodResponse::CalendarEventCounter(response)
        }
        _ => MethodResponse::CalendarEventDeclineCounter(response),
    })
}

fn now_secs() -> Result<i64, CallError> {
    Ok(Ctx::now()?.secs())
}

/// `CalendarEvent/counter`.
pub(crate) async fn counter(
    ctx: &Ctx<'_>,
    args: CalendarEventCounterArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let dav = super::account(ctx)?;
    let own = Arc::new(super::own_addresses(ctx, &dav));
    let proposal = Proposal::new(
        args.utc_start.unix_timestamp(),
        args.utc_end.unix_timestamp(),
    )
    .ok_or_else(|| MethodError::invalid_arguments("utcEnd is before utcStart"))?;
    let (text, occurrence, invitation) = match &args.subject {
        CounterSubject::Event(id) => {
            let (text, occurrence) = stored(ctx, id).await?;
            (text.into_bytes(), occurrence, false)
        }
        CounterSubject::Invitation(id) => (blob(ctx, id).await?, None, true),
    };
    let now = now_secs()?;
    let comment = args.comment;
    let message = super::blocking(move || {
        let source = if invitation {
            CounterSource::Invitation(&text)
        } else {
            CounterSource::Stored {
                text: &text,
                occurrence,
            }
        };
        build_counter(source, &own, proposal, comment.as_deref(), now)
    })
    .await?
    .map_err(|e| refused(&e))?;
    send(ctx, message).await
}

/// `CalendarEvent/declineCounter`.
pub(crate) async fn decline_counter(
    ctx: &Ctx<'_>,
    args: CalendarEventDeclineCounterArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let dav = super::account(ctx)?;
    let own = Arc::new(super::own_addresses(ctx, &dav));
    let (text, _) = stored(ctx, &args.event_id).await?;
    let proposal = blob(ctx, &args.blob_id).await?;
    let now = now_secs()?;
    let comment = args.comment;
    let message = super::blocking(move || {
        build_decline(text.as_bytes(), &proposal, &own, comment.as_deref(), now)
    })
    .await?
    .map_err(|e| refused(&e))?;
    send(ctx, message).await
}
