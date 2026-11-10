// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `GET /jmap/eventsource/` (RFC 8620 §7.3): `state` events with
//! `StateChange` objects and an id of every state (`event_id`; a client
//! that reconnects with `Last-Event-ID` gets what it missed at once), `calendarAlert` events with `CalendarAlert`
//! objects (draft-ietf-jmap-calendars; the event name is Stalwart's) and
//! `ping` events.
//!
//! An alert goes only to connections of its own account that asked for
//! the `CalendarAlert` type or `*` (checked against Stalwart: a connection
//! with `types=CalendarEvent` gets none, one with `*` gets them).
//! `closeafter=state` ends after the first `state` event; alerts before it
//! do not count.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::{Stream, StreamExt as _, future};
use http::{HeaderValue, StatusCode};
use nebula_jmap_proto::push::{
    CalendarAlert, CloseAfter, EventSourceParams, PingEvent, PushTypes, StateChange,
};
use nebula_jmap_proto::types::DataType;
use nebula_jmap_proto::{AccountId as JmapAccountId, UnsignedInt};

use crate::backend::{Backend, PushEvent, shared_account_key};
use crate::error::{HttpError, problem};
use crate::form::Form;
use crate::jmap::event_id::{MAX_LAST_EVENT_ID, Seen};
use crate::jmap::{authenticate, internal, limit_error};
use crate::state::Shared;

/// Ping intervals outside this range are moved into it (RFC 8620 §7.3
/// lets the server choose). Tests use 1 s to run fast.
const MIN_PING: u64 = if cfg!(test) { 1 } else { 10 };
const MAX_PING: u64 = 300;

/// The account itself, or one of its shared accounts.
fn is_of(account: &JmapAccountId, id: &JmapAccountId) -> bool {
    id == account || shared_account_key(account, id).is_some()
}

/// Keeps only the account's changes (and its shared accounts') of the
/// requested types.
fn filter_change(
    change: StateChange,
    account: &JmapAccountId,
    types: &PushTypes,
) -> Option<StateChange> {
    let changed: std::collections::BTreeMap<_, _> = change
        .changed
        .into_iter()
        .filter(|(id, _)| is_of(account, id))
        .filter_map(|(id, mut states)| {
            states.retain(|data_type, _| types.includes(data_type));
            (!states.is_empty()).then_some((id, states))
        })
        .collect();
    (!changed.is_empty()).then_some(StateChange { changed })
}

/// Keeps an alert of the account if `CalendarAlert` was asked for.
fn filter_alert(
    alert: CalendarAlert,
    account: &JmapAccountId,
    types: &PushTypes,
) -> Option<CalendarAlert> {
    (is_of(account, alert.account_id()) && types.includes(&DataType::CalendarAlert))
        .then_some(alert)
}

/// What is kept of a push event for a connection.
enum Kept {
    State(StateChange),
    Alert(CalendarAlert),
}

fn keep(event: PushEvent, account: &JmapAccountId, types: &PushTypes) -> Option<Kept> {
    match event {
        PushEvent::State(change) => filter_change(change, account, types).map(Kept::State),
        PushEvent::CalendarAlert(alert) => filter_alert(alert, account, types).map(Kept::Alert),
    }
}

/// A `state` event with the id of every state seen (RFC 8620 §7.3).
fn state_event(change: &StateChange, id: String) -> Result<Event, axum::Error> {
    Event::default().event("state").id(id).json_data(change)
}

/// A `calendarAlert` event. It sets no id: it changes no state, so the
/// last `state` event's id stays the one a reconnection sends.
fn alert_event(alert: &CalendarAlert) -> Result<Event, axum::Error> {
    Event::default()
        .event(CalendarAlert::EVENT_NAME)
        .json_data(alert)
}

/// The `Last-Event-ID` of a reconnection, if it has one we may read.
fn last_event_id(headers: &http::HeaderMap) -> Option<&str> {
    let value = headers.get("last-event-id")?;
    // Too long or not text: not one of ours, read as unreadable.
    Some(
        value
            .to_str()
            .ok()
            .filter(|text| text.len() <= MAX_LAST_EVENT_ID)
            .unwrap_or_default(),
    )
}

fn parse_params(query: &str) -> Option<EventSourceParams> {
    let form = Form::parse(query.as_bytes()).ok()?;
    EventSourceParams::parse(
        form.get("types")?,
        form.get("closeafter")?,
        form.get("ping")?,
    )
    .ok()
}

/// `GET /jmap/eventsource/?types=…&closeafter=…&ping=…`.
pub(crate) async fn eventsource<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    request: Request,
) -> Response {
    let account = match authenticate(&shared, request.headers()).await {
        Ok(account) => account,
        Err(response) => return *response,
    };
    let Some(params) = parse_params(request.uri().query().unwrap_or_default()) else {
        return problem(
            StatusCode::BAD_REQUEST,
            "types, closeafter and ping are required and must be valid.",
        );
    };
    let Some(slot) = shared.stream_slots.try_acquire(account.id) else {
        return limit_error(
            nebula_jmap_proto::error::LimitName::MaxConcurrentRequests,
            "Too many push connections for this account.",
        );
    };
    let alerts = params.types.includes(&DataType::CalendarAlert);
    let events = match shared.backend.events(&account, alerts).await {
        Ok(events) => events,
        Err(error) => return internal(&HttpError::from(error)),
    };

    // Read after subscribing: a change in between comes as an event too.
    let current = match shared.backend.push_states(&account).await {
        Ok(current) => current,
        Err(error) => return internal(&HttpError::from(error)),
    };
    let jmap_id = account.jmap_id.clone();
    let types = params.types.clone();
    let mut seen = Seen::new(filter_change(current, &jmap_id, &types).unwrap_or_default());
    // A reconnection gets at once what it missed (RFC 8620 §7.3).
    let missed = last_event_id(request.headers())
        .and_then(|last| seen.missed(&jmap_id, last))
        .map(|change| (true, state_event(&change, seen.id(&jmap_id))));
    let own = jmap_id.clone();
    let changes = events
        .filter_map(move |event| future::ready(keep(event, &jmap_id, &types)))
        .map(move |kept| match kept {
            Kept::State(change) => {
                seen.merge(&change);
                (true, state_event(&change, seen.id(&own)))
            }
            Kept::Alert(alert) => (false, alert_event(&alert)),
        });
    let changes = futures::stream::iter(missed)
        .chain(changes)
        .map(move |event| {
            // The slot is released when the stream ends.
            let _held = &slot;
            event
        });
    let changes: std::pin::Pin<Box<dyn Stream<Item = _> + Send>> = match params.close_after {
        // Up to and including the first `state` event.
        CloseAfter::State => Box::pin(futures::stream::unfold(
            Some(Box::pin(changes)),
            |inner| async move {
                let mut inner = inner?;
                let (state, event) = inner.next().await?;
                Some((event, (!state).then_some(inner)))
            },
        )),
        // `no`, and any value a newer jmap-proto may add: keep it open.
        _ => Box::pin(changes.map(|(_, event)| event)),
    };
    // End on shutdown, and after an access token's lifetime: the client
    // reconnects with a fresh token, so a revoked session stops getting
    // events within that time.
    let max_age = shared.config.lifetimes.access_token();
    let shutdown = shared.shutdown.clone().cancelled_owned();
    let stream = changes
        .take_until(shutdown)
        .take_until(tokio::time::sleep(max_age));

    let ping = u64::from(params.ping);
    let mut response = if ping == 0 {
        Sse::new(stream).into_response()
    } else {
        let interval = ping.clamp(MIN_PING, MAX_PING);
        let data = PingEvent {
            interval: UnsignedInt::from(u32::try_from(interval).unwrap_or(u32::MAX)),
        };
        let event = Event::default()
            .event("ping")
            .json_data(data)
            .unwrap_or_else(|_| Event::default().event("ping"));
        Sse::new(stream)
            .keep_alive(
                KeepAlive::new()
                    .interval(Duration::from_secs(interval))
                    .event(event),
            )
            .into_response()
    };
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    // Reverse proxies (nginx) must not buffer the stream.
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use nebula_jmap_proto::State as JmapState;
    use nebula_jmap_proto::types::DataType;

    use super::*;

    #[test]
    fn filters_other_accounts_and_types() {
        let me: JmapAccountId = "a1".parse().unwrap();
        let other: JmapAccountId = "a2".parse().unwrap();
        let change = StateChange {
            changed: [
                (
                    me.clone(),
                    [
                        (DataType::Email, JmapState::new("e1")),
                        (DataType::Mailbox, JmapState::new("m1")),
                    ]
                    .into(),
                ),
                (other, [(DataType::Email, JmapState::new("x"))].into()),
            ]
            .into(),
        };
        let only_email = PushTypes::Only(BTreeSet::from([DataType::Email]));
        let filtered = filter_change(change.clone(), &me, &only_email).unwrap();
        assert_eq!(filtered.changed.len(), 1);
        assert_eq!(filtered.changed[&me].len(), 1);
        let only_quota = PushTypes::Only(BTreeSet::from([DataType::Quota]));
        assert!(filter_change(change, &me, &only_quota).is_none());
    }

    /// The accounts others share with the user come along; another
    /// account's never (`a1-…` is a1's, `a10` and `a10-…` are not).
    #[test]
    fn keeps_shared_accounts() {
        let me: JmapAccountId = "a1".parse().unwrap();
        let shared = crate::backend::shared_account_id(&me, "r42").unwrap();
        assert_eq!(shared.as_str(), "a1-r42");
        let calendars = [(DataType::Calendar, JmapState::new("c1"))];
        let change = StateChange {
            changed: [
                (shared.clone(), calendars.clone().into()),
                ("a10".parse().unwrap(), calendars.clone().into()),
                ("a10-r42".parse().unwrap(), calendars.into()),
            ]
            .into(),
        };
        let filtered = filter_change(change, &me, &PushTypes::All).unwrap();
        assert_eq!(filtered.changed.keys().collect::<Vec<_>>(), [&shared]);
        assert_eq!(shared_account_key(&me, &shared), Some("r42"));
        assert_eq!(shared_account_key(&me, &"a10-r42".parse().unwrap()), None);
        assert_eq!(crate::backend::shared_account_id(&me, "r-1"), None);
        assert_eq!(crate::backend::shared_account_id(&me, ""), None);
    }
}
