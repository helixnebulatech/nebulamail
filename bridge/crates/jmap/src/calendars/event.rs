// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `CalendarEvent/get` (by stored id and by occurrence id) and
//! `CalendarEvent/changes`.
//!
//! As Stalwart 0.16: without `properties` the whole event comes with `id`,
//! `calendarIds`, `isDraft` and `isOrigin` (no `utcStart`, `utcEnd` or
//! `baseEventId`, and no `iCalendar`); with `properties`, just those (and
//! `id`) and the unconverted iCalendar properties under `iCalendar`. An
//! occurrence has no `recurrenceRule` or `recurrenceOverrides` (Stalwart
//! sends them as `null` when asked for; a body here has no nulls, and the
//! web reads both the same).
//! `timeZone` is the zone of floating events and of occurrences of events
//! in UTC or floating. An event that doesn't repeat, which an expanded
//! query names by its own id, comes as itself with `baseEventId: null`;
//! its occurrence id, the only one the bridge gave before, still works.

use std::collections::BTreeMap;

use nebula_groupware::model::calendar::{EventJson, EventOptions, ParsedCalendar, Zone};
use nebula_groupware::{GroupwareAccount, GroupwareError, ResourceText};
use nebula_jmap_proto::method::{ChangesArguments, GetResponse};
use nebula_jmap_proto::object::js_body::{JsBody, JsCalendar};
use nebula_jmap_proto::object::{CalendarEvent, CalendarEventGetArguments, CalendarEventProperty};
use nebula_jmap_proto::types::{TrueSet, UtcDate};
use nebula_jmap_proto::{CalendarEventId, CalendarId, MethodError, MethodResponse};
use nebula_store::{CollectionKind, DataType, ResourceId, ResourceQuery, StoredResource};
use serde_json::{Map, Value};

use super::ids::{self, EventRef};
use crate::changes;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// A checked body from calcard's JSON. A member our checks refuse (it
/// should not happen) is left out rather than losing the event.
pub(crate) fn body(members: Map<String, Value>) -> JsBody<JsCalendar> {
    match JsBody::new(members.clone()) {
        Ok(body) => body,
        Err(error) => {
            let mut members = members;
            for (name, _) in error.problems() {
                members.remove(name);
            }
            tracing::info!("calendars: left out members calcard wrote that do not check");
            JsBody::new(members).unwrap_or_default()
        }
    }
}

/// Which properties to send.
pub(crate) struct Selection {
    /// `None`: everything.
    properties: Option<Vec<CalendarEventProperty>>,
}

impl Selection {
    pub(crate) fn new(properties: Option<Vec<CalendarEventProperty>>) -> Self {
        Self { properties }
    }

    fn wants(&self, property: &CalendarEventProperty) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|list| list.contains(property))
    }

    fn wants_or_all(&self, property: &CalendarEventProperty) -> bool {
        self.properties.is_none() || self.wants(property)
    }

    fn body_names(&self) -> Option<Vec<&str>> {
        self.properties.as_ref().map(|list| {
            list.iter()
                .filter_map(|p| match p {
                    CalendarEventProperty::Body(name) => Some(name.as_str()),
                    _ => None,
                })
                .collect()
        })
    }
}

/// One event or occurrence, ready to send.
fn to_event(
    id: CalendarEventId,
    base: Option<&CalendarEventId>,
    calendar: &CalendarId,
    event: EventJson,
    is_origin: bool,
    selection: &Selection,
) -> CalendarEvent {
    let utc = |seconds: i64| UtcDate::from_unix_timestamp(seconds).ok();
    let mut out = CalendarEvent {
        id: Some(id),
        ..CalendarEvent::default()
    };
    if selection.wants(&CalendarEventProperty::BaseEventId) {
        out.base_event_id = Some(base.cloned());
    }
    if selection.wants_or_all(&CalendarEventProperty::CalendarIds) {
        out.calendar_ids = Some(TrueSet::from_iter([calendar.clone()]));
    }
    if selection.wants_or_all(&CalendarEventProperty::IsDraft) {
        out.is_draft = Some(false);
    }
    if selection.wants_or_all(&CalendarEventProperty::IsOrigin) {
        out.is_origin = Some(is_origin);
    }
    if selection.wants(&CalendarEventProperty::UseDefaultAlerts) {
        out.use_default_alerts = Some(false);
    }
    if selection.wants(&CalendarEventProperty::UtcStart) {
        out.utc_start = event.utc_range.and_then(|(start, _)| utc(start));
    }
    if selection.wants(&CalendarEventProperty::UtcEnd) {
        out.utc_end = event.utc_range.and_then(|(_, end)| utc(end));
    }
    let full = body(event.members);
    out.body = match selection.body_names() {
        None => full,
        Some(names) => full.select(names.iter().copied()),
    };
    out
}

/// What one resource is asked for.
struct Wanted {
    resource: ResourceId,
    refs: Vec<(CalendarEventId, EventRef)>,
}

/// The ids asked for, each once, grouped by resource (first-seen order);
/// ids that are not ours go straight to `not_found`.
fn group(requested: Vec<CalendarEventId>, not_found: &mut Vec<CalendarEventId>) -> Vec<Wanted> {
    let mut seen: Vec<CalendarEventId> = Vec::new();
    let mut groups: Vec<Wanted> = Vec::new();
    for id in requested {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        let Some(event) = ids::parse_event(&id) else {
            not_found.push(id);
            continue;
        };
        let resource = event.resource();
        match groups.iter_mut().find(|g| g.resource == resource) {
            Some(group) => group.refs.push((id, event)),
            None => groups.push(Wanted {
                resource,
                refs: vec![(id, event)],
            }),
        }
    }
    groups
}

/// What [`build`] makes of one resource: each id with its event (`None`:
/// no such occurrence), and the organizer's address.
type Built = (Vec<(CalendarEventId, Option<EventJson>)>, Option<String>);

/// Builds every event asked for from one resource's text (CPU work).
fn build(
    text: &[u8],
    refs: &[(CalendarEventId, EventRef)],
    zone: Zone,
    options: &EventOptions,
) -> Built {
    let Ok(parsed) = ParsedCalendar::parse(text) else {
        return (
            refs.iter().map(|(id, _)| (id.clone(), None)).collect(),
            None,
        );
    };
    let organizer = parsed.organizer();
    let mut out = Vec::with_capacity(refs.len());
    for (id, event) in refs {
        let json = match event {
            EventRef::Base(_) => parsed.base_event(zone, options),
            EventRef::Occurrence(_, key) => parsed.occurrence(*key, zone, options),
        };
        out.push((id.clone(), json.ok().flatten()));
    }
    (out, organizer)
}

/// Most unreadable local copies one `CalendarEvent/get` reads again from
/// the server; the rest are `notFound` without a request.
const MAX_REFETCHES: usize = 8;

/// The ids of every event in the scope, if there are at most `max`
/// (`ids: null`; a client asks for more by id).
fn all_ids(resources: &[StoredResource], max: usize) -> Result<Vec<CalendarEventId>, CallError> {
    if resources.len() > max {
        return Err(MethodError::request_too_large(format!(
            "more than {max} events: ask for them by id"
        ))
        .into());
    }
    Ok(resources
        .iter()
        .map(|resource| ids::event_id(resource.id))
        .collect::<Result<_, _>>()?)
}

/// What reading one event's text gave, for `/get`.
#[derive(Debug)]
enum Read {
    Text(Vec<u8>),
    /// Not there, or its copy still unreadable (corrupt): `notFound`.
    NotFound,
    /// The server or the database failed: the call fails (a passing
    /// error must not tell the client the event doesn't exist).
    Failed(GroupwareError),
}

fn outcome(read: Result<Option<ResourceText>, GroupwareError>) -> Read {
    match read {
        Ok(Some(found)) => Read::Text(found.text.into_bytes()),
        Ok(None) => Read::NotFound,
        Err(GroupwareError::Unreadable) => {
            tracing::warn!("calendars: an event's local copy is unreadable");
            Read::NotFound
        }
        Err(error) => Read::Failed(error),
    }
}

/// The text of `stored` (`None`: not in the scope). An unreadable copy
/// is read again from the server, at most [`MAX_REFETCHES`] times per
/// call (`refetches` counts them).
async fn read_event(
    running: &GroupwareAccount,
    stored: Option<&StoredResource>,
    refetches: &mut usize,
) -> Result<Option<ResourceText>, GroupwareError> {
    let Some(stored) = stored else {
        return Ok(None);
    };
    match running.resource_text(stored.id).await {
        Err(GroupwareError::Unreadable) if *refetches < MAX_REFETCHES => {
            *refetches = refetches.saturating_add(1);
            tracing::info!("calendars: local copy unreadable; reading it again");
            running.refetch(stored).await
        }
        other => other,
    }
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: CalendarEventGetArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.get.account_id)?;
    let running = super::account(ctx)?;
    let zone = super::zone(&args.time_zone)?;
    let own = super::own_addresses(ctx, &running);
    let state = ctx.state(DataType::CalendarEvent).await?;
    let requested: Vec<CalendarEventId> = if let Some(list) = &args.get.ids {
        list.iter().filter_map(|id| ctx.resolve(id)).collect()
    } else {
        let mut all = ctx
            .store()
            .visible_resources(ctx.id(), ResourceQuery::all(CollectionKind::Calendar))
            .await?;
        crate::sharing::scope::retain(ctx, CollectionKind::Calendar, &mut all).await?;
        all_ids(&all, ctx.limits.max_objects_in_get)?
    };
    let selection = Selection::new(args.get.properties.clone());
    let overrides_between = match (
        args.recurrence_overrides_after,
        args.recurrence_overrides_before,
    ) {
        (None, None) => None,
        (after, before) => {
            let after = after.map_or(i64::MIN, UtcDate::unix_timestamp);
            let before = before.map_or(i64::MAX, UtcDate::unix_timestamp);
            (after < before).then_some((after, before))
        }
    };
    let options = EventOptions {
        keep_unconverted: args.get.properties.is_some(),
        only_participants: args.reduce_participants.then(|| own.clone()),
        overrides_between,
    };
    let mut not_found = Vec::new();
    let mut found: BTreeMap<CalendarEventId, CalendarEvent> = BTreeMap::new();
    let order = requested.clone();
    let mut refetches = 0_usize;
    for wanted in group(requested, &mut not_found) {
        let stored = ctx
            .store()
            .visible_resource(ctx.id(), CollectionKind::Calendar, wanted.resource)
            .await?;
        let stored = crate::sharing::scope::resource(ctx, CollectionKind::Calendar, stored).await?;
        let read = read_event(&running, stored.as_ref(), &mut refetches).await;
        let text = match outcome(read) {
            Read::Text(text) => text,
            Read::NotFound => {
                not_found.extend(wanted.refs.into_iter().map(|(id, _)| id));
                continue;
            }
            Read::Failed(error) => return Err(super::groupware_error(&error)),
        };
        let Some(stored) = stored else {
            not_found.extend(wanted.refs.into_iter().map(|(id, _)| id));
            continue;
        };
        let calendar = ids::calendar_id(stored.collection)?;
        let refs = wanted.refs;
        let options = options.clone();
        let (built, organizer) =
            super::blocking(move || build(&text, &refs, zone, &options)).await?;
        let is_origin = organizer.as_deref().is_none_or(|organizer| {
            own.iter()
                .any(|address| address.eq_ignore_ascii_case(organizer))
        });
        let base = ids::event_id(wanted.resource)?;
        for (id, json) in built {
            let Some(json) = json else {
                not_found.push(id);
                continue;
            };
            let series = (id != base).then_some(&base);
            let event = to_event(id.clone(), series, &calendar, json, is_origin, &selection);
            found.insert(id, event);
        }
    }
    let mut list = Vec::with_capacity(found.len());
    for id in order {
        if let Some(event) = found.remove(&id) {
            list.push(event);
        }
    }
    Ok(MethodResponse::CalendarEventGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    super::account(ctx)?;
    let response = changes::changes(ctx, DataType::CalendarEvent, args, |row| {
        ResourceId::from_raw(row)
            .ok_or(Bug("event row"))
            .and_then(ids::event_id)
    })
    .await?;
    let response =
        crate::sharing::scope::resource_changes(ctx, CollectionKind::Calendar, response, |id| {
            ids::parse_event(id).map(EventRef::resource)
        })
        .await?;
    Ok(MethodResponse::CalendarEventChanges(response))
}

#[cfg(test)]
mod tests {
    use nebula_store::{CollectionId, DavHref, Parsed, UnixTime};

    use super::*;

    fn resource(id: i64) -> StoredResource {
        StoredResource {
            id: ResourceId::from_raw(id).unwrap(),
            collection: CollectionId::from_raw(1).unwrap(),
            href: DavHref::new(&format!("https://dav.example/cal/work/{id}.ics")).unwrap(),
            etag: None,
            size: 1,
            parsed: Parsed::Invalid,
            updated_at: UnixTime::from_secs(1),
            orphaned_at: None,
        }
    }

    /// `ids: null` gives every event up to `maxObjectsInGet`, and an
    /// error past it (not a silent cut).
    #[test]
    fn all_ids_are_bounded() {
        let three: Vec<StoredResource> = (1..=3).map(resource).collect();
        let listed = all_ids(&three, 3).unwrap();
        assert_eq!(
            listed
                .iter()
                .map(CalendarEventId::as_str)
                .collect::<Vec<_>>(),
            ["v1", "v2", "v3"]
        );
        let Err(CallError::Method(error)) = all_ids(&three, 2) else {
            panic!("too many")
        };
        assert_eq!(
            error.kind(),
            nebula_jmap_proto::MethodErrorType::RequestTooLarge
        );
    }

    /// A copy that stays unreadable is `notFound`, one event at a time; a
    /// failing server or database fails the call instead.
    #[test]
    fn read_outcomes() {
        let text = ResourceText {
            resource: resource(1),
            text: "BEGIN:VCALENDAR".to_owned(),
        };
        assert!(matches!(outcome(Ok(Some(text))), Read::Text(t) if t == b"BEGIN:VCALENDAR"));
        assert!(matches!(outcome(Ok(None)), Read::NotFound));
        assert!(matches!(
            outcome(Err(GroupwareError::Unreadable)),
            Read::NotFound
        ));
        assert!(matches!(
            outcome(Err(GroupwareError::Dav(nebula_dav::DavError::TooMany))),
            Read::Failed(_)
        ));
        assert!(matches!(
            outcome(Err(GroupwareError::NotDiscovered)),
            Read::Failed(_)
        ));
    }
}
