// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Principal/getAvailability` (draft-ietf-jmap-calendars) through the
//! DAV server's scheduling outbox (RFC 6638 §5): one `VFREEBUSY` request
//! for the principal's address, asked as the user (the `ORGANIZER` is
//! their first calendar address).
//!
//! **The user's own principal** is answered from the local copy instead
//! (`GroupwareAccount::own_free_busy`): every calendar of
//! theirs counts, not only the default one (Stalwart's outbox only
//! counts that one), with transparency, cancelled events, their answer to
//! invitations, recurrences and time zones.
//!
//! As Stalwart 0.16.27: an end before the start gives an empty list, and
//! someone whose busy times the server does not give (nothing shared, not
//! a calendar user) too, the same as "free" (the web only marks busy
//! guests). Unlike Stalwart: an unknown id is `notFound` (Stalwart says
//! `serverUnavailable`) and a range over `maxAvailabilityDuration` is
//! `tooLarge`, as the draft says.
//!
//! `showDetails` (draft-ietf-jmap-calendars: the event "if the user has
//! permission to view" it; `null` if it is `private`): for the user's own
//! principal each period carries its occurrence, as `CalendarEvent/get`
//! gives it for the occurrence id (`id`, `baseEventId`, `calendarIds` and
//! the JSCalendar; an event that doesn't repeat, as itself, with
//! `baseEventId: null`), unless the event is `private` or `secret` (iCalendar
//! `CLASS:PRIVATE`/`CONFIDENTIAL`), and only the first
//! [`nebula_groupware::MAX_DETAILED`]. `eventProperties` keeps only
//! those members. For anyone else `event` is always `null`: a DAV
//! free-busy reply (`VFREEBUSY`) has only times, and the bridge does not
//! read others' calendars for this, even shared ones (their privacy and
//! permissions are the server's to apply).

use nebula_dav::proto::CalendarAddress;
use nebula_groupware::OwnEvent;
use nebula_groupware::model::freebusy::{BusyPeriod as DavBusyPeriod, BusyStatus as DavBusyStatus};
use nebula_jmap_proto::object::{
    BusyPeriod, BusyStatus, PrincipalGetAvailabilityArguments, PrincipalGetAvailabilityResponse,
};
use nebula_jmap_proto::types::Duration;
use nebula_jmap_proto::{MethodError, MethodErrorType, MethodResponse, UtcDate};
use serde_json::{Map, Value};

use super::ids;
use crate::calendars::event::body;
use crate::calendars::ids as calendar_ids;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// The event of a busy period as JMAP (`showDetails`): an occurrence as
/// `CalendarEvent/get` gives it, with only `properties` if given.
fn event_json(event: OwnEvent, properties: Option<&[String]>) -> Result<Value, Bug> {
    let base = calendar_ids::event_id(event.resource)?;
    let (id, base) = match event.occurrence {
        Some(key) => (
            calendar_ids::occurrence_id(event.resource, key)?,
            Value::String(base.as_str().to_owned()),
        ),
        None => (base, Value::Null),
    };
    let mut out = Map::new();
    out.insert("id".to_owned(), Value::String(id.as_str().to_owned()));
    out.insert("baseEventId".to_owned(), base);
    let mut calendars = Map::new();
    calendars.insert(
        calendar_ids::calendar_id(event.calendar)?
            .as_str()
            .to_owned(),
        Value::Bool(true),
    );
    out.insert("calendarIds".to_owned(), Value::Object(calendars));
    for (name, value) in body(event.members).into_map() {
        out.entry(name).or_insert(value);
    }
    if let Some(keep) = properties {
        out.retain(|name, _| keep.iter().any(|k| k == name));
    }
    Ok(Value::Object(out))
}

/// The periods as JMAP, each with its event if it has one.
fn busy_periods(
    periods: Vec<(DavBusyPeriod, Option<OwnEvent>)>,
    properties: Option<&[String]>,
) -> Result<Vec<BusyPeriod>, Bug> {
    let mut list = Vec::with_capacity(periods.len());
    for (period, event) in periods {
        let (Ok(utc_start), Ok(utc_end)) = (
            UtcDate::from_unix_timestamp(period.start),
            UtcDate::from_unix_timestamp(period.end),
        ) else {
            continue;
        };
        list.push(BusyPeriod {
            utc_start,
            utc_end,
            busy_status: match period.status {
                DavBusyStatus::Tentative => BusyStatus::Tentative,
                DavBusyStatus::Unavailable => BusyStatus::Unavailable,
                _ => BusyStatus::Confirmed,
            },
            event: event
                .map(|event| event_json(event, properties))
                .transpose()?,
        });
    }
    Ok(list)
}

pub(crate) async fn get_availability(
    ctx: &Ctx<'_>,
    args: PrincipalGetAvailabilityArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let running = super::account(ctx)?;
    if !running.supports_free_busy() {
        return Err(
            MethodError::new(MethodErrorType::AccountNotSupportedByMethod)
                .with_description("the calendar server gives no free-busy")
                .into(),
        );
    }
    let number = ids::principal_number(&args.id);
    let principal = number.and_then(|number| running.known_principal(number));
    // An address without a principal (only for sharing) is never asked
    // about: some servers would mail outsiders a free-busy request.
    let address_only =
        principal.is_none() && number.is_some_and(|number| running.known_address(number).is_some());
    if address_only {
        return Ok(MethodResponse::PrincipalGetAvailability(
            PrincipalGetAvailabilityResponse { list: Vec::new() },
        ));
    }
    let principal = principal.ok_or_else(|| CallError::from(MethodErrorType::NotFound))?;
    let start = args.utc_start.unix_timestamp();
    let end = args.utc_end.unix_timestamp();
    let longest = super::MAX_AVAILABILITY_DURATION
        .parse::<Duration>()
        .map_err(|_| Bug("maxAvailabilityDuration"))?
        .seconds();
    if end.saturating_sub(start) > i64::try_from(longest).unwrap_or(i64::MAX) {
        return Err(MethodError::new(MethodErrorType::TooLarge)
            .with_description("the range is longer than maxAvailabilityDuration")
            .into());
    }
    let empty = || {
        MethodResponse::PrincipalGetAvailability(PrincipalGetAvailabilityResponse {
            list: Vec::new(),
        })
    };
    // Before 1970 there is nothing to ask (the request writes UTC times
    // from 1970 on).
    if end <= start || end <= 0 {
        return Ok(empty());
    }
    let Some(attendee) = principal.addresses.first().cloned().or_else(|| {
        super::email_of(ctx, &super::Who::Dav(principal.clone()))
            .and_then(|email| CalendarAddress::parse(&email))
    }) else {
        return Ok(empty());
    };
    let organizer = running
        .own_principal()
        .and_then(|own| own.addresses.first().cloned())
        .or_else(|| CalendarAddress::parse(&ctx.account.email))
        .ok_or_else(|| {
            MethodError::new(MethodErrorType::Forbidden)
                .with_description("the account has no calendar address")
        })?;
    let periods: Vec<(_, Option<OwnEvent>)> = if principal.own {
        let mut own: Vec<String> = principal
            .addresses
            .iter()
            .map(|a| a.address().to_owned())
            .collect();
        own.push(ctx.account.email.clone());
        running
            .own_availability(&own, (start.max(0), end), args.show_details)
            .await
            .map_err(|e| super::groupware_error(&e))?
            .into_iter()
            .map(|found| (found.period, found.event))
            .collect()
    } else {
        running
            .free_busy(&organizer, &attendee, start.max(0), end)
            .await
            .map_err(|e| super::groupware_error(&e))?
            .unwrap_or_default()
            .into_iter()
            .map(|period| (period, None))
            .collect()
    };
    let list = busy_periods(periods, args.event_properties.as_deref())?;
    Ok(MethodResponse::PrincipalGetAvailability(
        PrincipalGetAvailabilityResponse { list },
    ))
}

#[cfg(test)]
mod tests {
    use nebula_groupware::model::calendar::RecurrenceKey;
    use nebula_store::{CollectionId, ResourceId};
    use serde_json::json;

    use super::*;

    fn event() -> OwnEvent {
        let members = json!({
            "@type": "Event",
            "title": "Planning",
            "start": "2026-10-19T08:00:00",
            "duration": "PT1H",
        });
        OwnEvent {
            resource: ResourceId::from_raw(7).unwrap(),
            calendar: CollectionId::from_raw(3).unwrap(),
            // 2026-10-19T08:00:00 as a wall-clock reading.
            occurrence: Some(RecurrenceKey::from_naive(1_792_396_800).unwrap()),
            members: members.as_object().unwrap().clone(),
        }
    }

    #[test]
    fn event_without_repetition_as_itself() {
        let single = OwnEvent {
            occurrence: None,
            ..event()
        };
        let all = event_json(single, None).unwrap();
        assert_eq!(all["id"], "v7");
        assert_eq!(all["baseEventId"], Value::Null);
        assert_eq!(all["title"], "Planning");
    }

    #[test]
    fn event_as_an_occurrence() {
        let all = event_json(event(), None).unwrap();
        assert_eq!(all["id"], "v7_20261019T080000");
        assert_eq!(all["baseEventId"], "v7");
        assert_eq!(all["calendarIds"], json!({"c3": true}));
        assert_eq!(all["title"], "Planning");
        assert_eq!(all["@type"], "Event");
        // eventProperties: only those.
        let some = event_json(event(), Some(&["title".to_owned(), "id".to_owned()])).unwrap();
        assert_eq!(
            some,
            json!({"id": "v7_20261019T080000", "title": "Planning"})
        );
        assert_eq!(event_json(event(), Some(&[])).unwrap(), json!({}));
    }
}
