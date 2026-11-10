// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `CalendarEvent` (draft-ietf-jmap-calendars §5): a JSCalendar `Event`
//! (kept as a bounded [`JsBody`]) plus the JMAP properties (`id`,
//! `calendarIds`, `utcStart`…), which are typed.
//!
//! Occurrences: with `expandRecurrences`, `/query` returns one id per
//! occurrence; such an event has `baseEventId` (the series) and the
//! JSCalendar `recurrenceId` (its original start). How those ids are made
//! is up to the implementation; here they are just [`CalendarEventId`]s.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::email::SetPatch;
use super::email::patch::Members;
use super::js_body::{BodyPatch, JsBody, JsCalendar, JsPropertyName, JsPropertyNameError};
use crate::error::{MethodError, SetError};
use crate::macros::serde_via_str;
use crate::method::parse::{PropertyErrors, from_arguments, take_argument};
use crate::method::patch::Patch;
use crate::method::{
    Filter, FilterCondition, GetArguments, PatchObject, QueryArguments, QueryChangesArguments,
    SetArguments, SortProperty,
};
use crate::references::IdOrRef;
use crate::types::serde_util::{empty_map_as_null, empty_vec_as_null};
use crate::types::{
    AccountId, BlobId, CalendarEventId, CalendarId, LocalDateTime, TimeZoneId, TrueSet, UtcDate,
};

/// A `CalendarEvent` property name: a JMAP one, or any JSCalendar member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum CalendarEventProperty {
    /// `id`.
    Id,
    /// `baseEventId`: the series an occurrence belongs to.
    BaseEventId,
    /// `calendarIds`.
    CalendarIds,
    /// `isDraft`.
    IsDraft,
    /// `isOrigin`: the user is the organizer (or there is none).
    IsOrigin,
    /// `useDefaultAlerts`.
    UseDefaultAlerts,
    /// `utcStart`.
    UtcStart,
    /// `utcEnd`.
    UtcEnd,
    /// A JSCalendar member (`title`, `start`, `participants`…).
    Body(JsPropertyName),
}

impl CalendarEventProperty {
    /// The JMAP properties, i.e. every variant but `Body`.
    pub const JMAP: &'static [Self] = &[
        Self::Id,
        Self::BaseEventId,
        Self::CalendarIds,
        Self::IsDraft,
        Self::IsOrigin,
        Self::UseDefaultAlerts,
        Self::UtcStart,
        Self::UtcEnd,
    ];

    /// The property name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Id => "id",
            Self::BaseEventId => "baseEventId",
            Self::CalendarIds => "calendarIds",
            Self::IsDraft => "isDraft",
            Self::IsOrigin => "isOrigin",
            Self::UseDefaultAlerts => "useDefaultAlerts",
            Self::UtcStart => "utcStart",
            Self::UtcEnd => "utcEnd",
            Self::Body(name) => name.as_str(),
        }
    }
}

impl FromStr for CalendarEventProperty {
    type Err = JsPropertyNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Self::JMAP.iter().find(|known| known.as_str() == s) {
            Some(known) => Ok(known.clone()),
            None => s.parse().map(Self::Body),
        }
    }
}

impl fmt::Display for CalendarEventProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

serde_via_str!(CalendarEventProperty);

/// A `CalendarEvent` as returned by `/get`, `/parse` and `/set`
/// (`created`). JMAP properties are `None` when not sent; the body has
/// only the requested JSCalendar members.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<CalendarEventId>,
    /// `baseEventId` (`null` for an event that is not an occurrence).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_event_id: Option<Option<CalendarEventId>>,
    /// `calendarIds`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_ids: Option<TrueSet<CalendarId>>,
    /// `isDraft`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_draft: Option<bool>,
    /// `isOrigin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_origin: Option<bool>,
    /// `useDefaultAlerts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_default_alerts: Option<bool>,
    /// `utcStart` (in the request's `timeZone` for floating events).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utc_start: Option<UtcDate>,
    /// `utcEnd`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utc_end: Option<UtcDate>,
    /// The JSCalendar members.
    #[serde(flatten)]
    pub body: JsBody<JsCalendar>,
}

/// Properties only the server sets.
const SERVER_SET: &[&str] = &["id", "baseEventId", "isOrigin", "utcStart", "utcEnd"];

/// An event to create (`CalendarEvent/set` create).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEventCreate {
    /// The calendars it goes in (at least one; `#creationId` allowed).
    pub calendar_ids: TrueSet<IdOrRef<CalendarId>>,
    /// Whether it is a draft (no scheduling messages while it is).
    pub is_draft: bool,
    /// Whether the calendar's default alerts apply; `None`: the default.
    pub use_default_alerts: Option<bool>,
    /// The JSCalendar event (has `start`).
    pub body: JsBody<JsCalendar>,
}

impl CalendarEventCreate {
    /// Parses one object of `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every invalid or server-set property,
    /// and `calendarIds` or `start` if missing.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut calendar_ids = None;
        let mut is_draft = None;
        let mut use_default_alerts = None;
        let mut body = Map::new();
        for (key, value) in object {
            match key.as_str() {
                "calendarIds" => {
                    calendar_ids = errors
                        .parse::<TrueSet<IdOrRef<CalendarId>>>(&key, value)
                        .filter(|set| {
                            let ok = !set.is_empty();
                            if !ok {
                                errors.invalid_because("calendarIds", "must not be empty");
                            }
                            ok
                        });
                }
                "isDraft" => is_draft = errors.parse::<bool>(&key, value),
                "useDefaultAlerts" => use_default_alerts = errors.parse::<bool>(&key, value),
                name if SERVER_SET.contains(&name) => errors.invalid_because(&key, "server-set"),
                _ => {
                    body.insert(key, value);
                }
            }
        }
        // A task may have no start (nor due: RFC 8984 §5.2); an event must.
        let is_task = body.get("@type").and_then(Value::as_str) == Some("Task");
        if !is_task && !body.contains_key("start") {
            errors.invalid_because("start", "required");
        }
        let body = match JsBody::new(body) {
            Ok(body) => Some(body),
            Err(error) => {
                for (name, why) in error.problems() {
                    errors.invalid_because(name.as_str(), why.as_str());
                }
                None
            }
        };
        let calendar_ids = errors.required("calendarIds", calendar_ids);
        errors.finish()?;
        Ok(Self {
            calendar_ids: calendar_ids.unwrap_or_default(),
            is_draft: is_draft.unwrap_or(false),
            use_default_alerts,
            body: body.unwrap_or_default(),
        })
    }
}

/// A `CalendarEvent/set` update: JMAP properties typed, JSCalendar ones as
/// checked patches (apply them with [`JsBody::apply`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CalendarEventPatch {
    /// Move to other calendars (never to none).
    pub calendar_ids: Option<SetPatch<IdOrRef<CalendarId>>>,
    /// Draft or not (the draft only allows `true` → `false`).
    pub is_draft: Option<bool>,
    /// Whether the default alerts apply.
    pub use_default_alerts: Option<bool>,
    /// Changes to the JSCalendar event, in no particular order (they
    /// never overlap).
    pub body: Vec<BodyPatch>,
}

impl CalendarEventPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidPatch` for bad or overlapping paths; `invalidProperties`
    /// for server-set properties and bad values.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut out = Self::default();
        let mut members = Members::<IdOrRef<CalendarId>>::default();
        for Patch { key, path, value } in patch.patches()? {
            match path.tokens() {
                [one] if one == "calendarIds" => {
                    out.calendar_ids = errors
                        .parse::<TrueSet<IdOrRef<CalendarId>>>(&key, value)
                        .filter(|set| {
                            let ok = !set.is_empty();
                            if !ok {
                                errors.invalid_because(&key, "must not be empty");
                            }
                            ok
                        })
                        .map(|set| SetPatch::Replace(set.into_set()));
                }
                [one, member] if one == "calendarIds" => members.record(&key, member, &value)?,
                [one, ..] if one == "calendarIds" => {
                    return Err(SetError::invalid_patch(format!("{key:?} goes too deep")));
                }
                [one] if one == "isDraft" => out.is_draft = errors.parse(&key, value),
                [one] if one == "useDefaultAlerts" => {
                    out.use_default_alerts = errors.parse(&key, value);
                }
                [one, ..] if SERVER_SET.contains(&one.as_str()) => {
                    errors.invalid_because(&key, "server-set");
                }
                [one, ..] if one == "isDraft" || one == "useDefaultAlerts" => {
                    return Err(SetError::invalid_patch(format!("{key:?} goes too deep")));
                }
                _ => match BodyPatch::parse::<JsCalendar>(&key, path, value) {
                    Ok(body) => out.body.push(body),
                    Err(error) if error.kind() == crate::SetErrorType::InvalidPatch => {
                        return Err(error);
                    }
                    Err(error) => {
                        errors.invalid_because(&key, error.description().unwrap_or("invalid"));
                    }
                },
            }
        }
        errors.finish()?;
        if out.calendar_ids.is_none() {
            out.calendar_ids = members.into_patch();
        }
        Ok(out)
    }
}

/// `CalendarEvent/get` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEventGetArguments {
    /// The standard arguments.
    pub get: GetArguments<CalendarEventId, CalendarEventProperty>,
    /// Leave out `recurrenceOverrides` that start before this.
    pub recurrence_overrides_before: Option<UtcDate>,
    /// Leave out `recurrenceOverrides` that start after this.
    pub recurrence_overrides_after: Option<UtcDate>,
    /// Only the participants that are the user or the organizer.
    pub reduce_participants: bool,
    /// The zone for `utcStart`/`utcEnd` of floating events (default
    /// `Etc/UTC`).
    pub time_zone: TimeZoneId,
}

impl CalendarEventGetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` or `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let recurrence_overrides_before =
            take_argument(&mut arguments, "recurrenceOverridesBefore")?;
        let recurrence_overrides_after = take_argument(&mut arguments, "recurrenceOverridesAfter")?;
        let reduce_participants =
            take_argument(&mut arguments, "reduceParticipants")?.unwrap_or(false);
        let time_zone = take_argument(&mut arguments, "timeZone")?.unwrap_or_else(TimeZoneId::utc);
        Ok(Self {
            get: GetArguments::parse(arguments, max_objects)?,
            recurrence_overrides_before,
            recurrence_overrides_after,
            reduce_participants,
            time_zone,
        })
    }
}

/// `CalendarEvent` `FilterCondition` (draft-ietf-jmap-calendars §5.11).
/// Text conditions match case-insensitively; which ones are supported is
/// up to the implementation (`unsupportedFilter` otherwise).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventFilterCondition {
    /// In this calendar.
    #[serde(default)]
    pub in_calendar: Option<CalendarId>,
    /// Ends after this time (read in the query's `timeZone`).
    #[serde(default)]
    pub after: Option<LocalDateTime>,
    /// Starts before this time (read in the query's `timeZone`).
    #[serde(default)]
    pub before: Option<LocalDateTime>,
    /// Any text property contains this.
    #[serde(default)]
    pub text: Option<String>,
    /// `title` contains this.
    #[serde(default)]
    pub title: Option<String>,
    /// `description` contains this.
    #[serde(default)]
    pub description: Option<String>,
    /// A location contains this.
    #[serde(default)]
    pub location: Option<String>,
    /// The owner (organizer) contains this.
    #[serde(default)]
    pub owner: Option<String>,
    /// An attendee contains this (older drafts; Stalwart).
    #[serde(default)]
    pub attendee: Option<String>,
    /// A participant contains this (current draft).
    #[serde(default)]
    pub participant: Option<String>,
    /// The `uid` is exactly this.
    #[serde(default)]
    pub uid: Option<String>,
}

impl FilterCondition for CalendarEventFilterCondition {
    const PROPERTIES: &'static [&'static str] = &[
        "inCalendar",
        "after",
        "before",
        "text",
        "title",
        "description",
        "location",
        "owner",
        "attendee",
        "participant",
        "uid",
    ];
}

/// `CalendarEvent` sort properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CalendarEventSort {
    /// `start` (as `utcStart`).
    Start,
    /// `uid`.
    Uid,
    /// `recurrenceId`.
    RecurrenceId,
    /// `id`.
    Id,
    /// `created`.
    Created,
    /// `updated`.
    Updated,
}

impl SortProperty for CalendarEventSort {
    fn parse(property: &str, _extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        Ok(match property {
            "start" => Self::Start,
            "uid" => Self::Uid,
            "recurrenceId" => Self::RecurrenceId,
            "id" => Self::Id,
            "created" => Self::Created,
            "updated" => Self::Updated,
            other => {
                return Err(MethodError::unsupported_sort(format!(
                    "cannot sort events by {other:?}"
                )));
            }
        })
    }
}

/// `CalendarEvent/query` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEventQueryArguments {
    /// The standard arguments.
    pub query: QueryArguments<CalendarEventFilterCondition, CalendarEventSort, CalendarEventId>,
    /// One result per occurrence (between `after` and `before`).
    pub expand_recurrences: bool,
    /// The zone `after`/`before` and floating events are read in (default
    /// `Etc/UTC`).
    pub time_zone: TimeZoneId,
}

impl CalendarEventQueryArguments {
    /// Parses the arguments. With `expandRecurrences`, the filter must be
    /// one condition with both `after` and `before` (the draft and
    /// Stalwart agree).
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let expand_recurrences =
            take_argument(&mut arguments, "expandRecurrences")?.unwrap_or(false);
        let time_zone = take_argument(&mut arguments, "timeZone")?.unwrap_or_else(TimeZoneId::utc);
        let parsed = Self {
            query: QueryArguments::parse(arguments)?,
            expand_recurrences,
            time_zone,
        };
        if expand_recurrences && parsed.expansion_range().is_none() {
            return Err(MethodError::invalid_arguments(
                "expandRecurrences needs a filter condition with both after and before",
            ));
        }
        Ok(parsed)
    }

    /// `(after, before)` when the filter is one condition with both (check
    /// it against `maxExpandedQueryDuration` → `expandDurationTooLarge`).
    #[must_use]
    pub fn expansion_range(&self) -> Option<(LocalDateTime, LocalDateTime)> {
        match &self.query.filter {
            Some(Filter::Condition(CalendarEventFilterCondition {
                after: Some(after),
                before: Some(before),
                ..
            })) => Some((*after, *before)),
            _ => None,
        }
    }
}

/// `CalendarEvent/queryChanges` arguments: the standard ones and those of
/// the original query that change its results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEventQueryChangesArguments {
    /// The standard arguments.
    pub query_changes:
        QueryChangesArguments<CalendarEventFilterCondition, CalendarEventSort, CalendarEventId>,
    /// The original query's `expandRecurrences`.
    pub expand_recurrences: bool,
    /// The original query's `timeZone` (default `Etc/UTC`).
    pub time_zone: TimeZoneId,
}

impl CalendarEventQueryChangesArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let expand_recurrences =
            take_argument(&mut arguments, "expandRecurrences")?.unwrap_or(false);
        let time_zone = take_argument(&mut arguments, "timeZone")?.unwrap_or_else(TimeZoneId::utc);
        Ok(Self {
            query_changes: QueryChangesArguments::parse(arguments)?,
            expand_recurrences,
            time_zone,
        })
    }
}

/// `CalendarEvent/set` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEventSetArguments {
    /// The standard arguments (creates: [`CalendarEventCreate::parse`];
    /// updates: [`CalendarEventPatch::parse`]).
    pub set: SetArguments<CalendarEventId>,
    /// Send iTIP messages (invitations, updates, cancellations, replies).
    pub send_scheduling_messages: bool,
}

impl CalendarEventSetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` or `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let send_scheduling_messages =
            take_argument(&mut arguments, "sendSchedulingMessages")?.unwrap_or(false);
        Ok(Self {
            set: SetArguments::parse(arguments, max_objects)?,
            send_scheduling_messages,
        })
    }
}

/// `CalendarEvent/parse` arguments (`urn:ietf:params:jmap:calendars:parse`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventParseArguments {
    /// The account.
    pub account_id: AccountId,
    /// The iCalendar blobs to read.
    pub blob_ids: Vec<BlobId>,
    /// Properties to return; `None`: all.
    #[serde(default)]
    pub properties: Option<Vec<CalendarEventProperty>>,
}

impl CalendarEventParseArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` beyond `max_objects`
    /// blobs.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        if parsed.blob_ids.len() > max_objects {
            return Err(MethodError::request_too_large("too many blobs"));
        }
        Ok(parsed)
    }
}

/// `CalendarEvent/parse` response. Every event of each blob (a series and
/// its overrides, or several events), as Stalwart does; each may carry
/// the iTIP `method` of its VCALENDAR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventParseResponse {
    /// The account.
    pub account_id: AccountId,
    /// Blob → the events in it.
    #[serde(serialize_with = "empty_map_as_null")]
    pub parsed: BTreeMap<BlobId, Vec<CalendarEvent>>,
    /// Blobs that are not iCalendar.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub not_parsable: Vec<BlobId>,
    /// Blobs that don't exist.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub not_found: Vec<BlobId>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::{MethodErrorType, SetErrorType};
    use crate::method::GetResponse;
    use crate::types::State;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    fn body(value: Value) -> JsBody<JsCalendar> {
        JsBody::new(map(value)).unwrap()
    }

    /// `listAccountOccurrences` in apps/web/src/calendar/api.ts.
    #[test]
    fn web_occurrence_query() {
        let args = CalendarEventQueryArguments::parse(map(json!({
            "accountId": "c",
            "expandRecurrences": true,
            "filter": {"after": "2026-10-19T00:00:00", "before": "2026-11-03T00:00:00"},
            "sort": [{"isAscending": true, "property": "start"}],
            "timeZone": "Europe/Madrid"
        })))
        .unwrap();
        assert!(args.expand_recurrences);
        assert_eq!(args.time_zone.as_str(), "Europe/Madrid");
        let (after, before) = args.expansion_range().unwrap();
        assert_eq!(after.to_string(), "2026-10-19T00:00:00");
        assert_eq!(before.to_string(), "2026-11-03T00:00:00");
        assert_eq!(args.query.sort[0].property, CalendarEventSort::Start);
        assert!(args.query.sort[0].is_ascending);
    }

    /// `findByUid` in apps/web/src/calendar/api.ts, and the defaults.
    #[test]
    fn uid_query_and_defaults() {
        let args = CalendarEventQueryArguments::parse(map(
            json!({"accountId": "c", "filter": {"uid": "nebula-seed-dentist"}}),
        ))
        .unwrap();
        assert!(!args.expand_recurrences);
        assert_eq!(args.time_zone, TimeZoneId::utc());
        let Some(Filter::Condition(condition)) = args.query.filter else {
            panic!("expected a condition")
        };
        assert_eq!(condition.uid.as_deref(), Some("nebula-seed-dentist"));
        let all = CalendarEventQueryArguments::parse(map(json!({
            "accountId": "c",
            "filter": {"operator": "AND", "conditions": [
                {"inCalendar": "b"}, {"text": "dentist"}, {"title": "x"}, {"description": "y"},
                {"location": "z"}, {"owner": "o"}, {"attendee": "a"}, {"participant": "p"}
            ]},
            "sort": [{"property": "uid"}, {"property": "recurrenceId"}, {"property": "id"},
                     {"property": "created"}, {"property": "updated", "isAscending": false}]
        })))
        .unwrap();
        assert_eq!(all.query.sort.len(), 5);
    }

    #[test]
    fn query_errors() {
        let kind = |value: Value| {
            CalendarEventQueryArguments::parse(map(value))
                .unwrap_err()
                .kind()
        };
        // As Stalwart answers (checked live 2026-10-27).
        assert_eq!(
            kind(json!({"accountId": "c", "filter": {"fuzzy": "x"}})),
            MethodErrorType::UnsupportedFilter
        );
        assert_eq!(
            kind(json!({"accountId": "c", "sort": [{"property": "title"}]})),
            MethodErrorType::UnsupportedSort
        );
        assert_eq!(
            kind(json!({"accountId": "c", "expandRecurrences": true})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(
                json!({"accountId": "c", "expandRecurrences": true, "filter": {"after": "2026-10-19T00:00:00"}})
            ),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(
                json!({"accountId": "c", "expandRecurrences": true, "filter": {"operator": "AND", "conditions": [
                {"after": "2026-10-19T00:00:00", "before": "2026-11-03T00:00:00"}]}})
            ),
            MethodErrorType::InvalidArguments
        );
        // LocalDateTime, never UTC (the draft); Stalwart is laxer here.
        assert_eq!(
            kind(json!({"accountId": "c", "filter": {"after": "2026-10-19T00:00:00Z"}})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "timeZone": "Europe/../x"})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "expandRecurrences": 1})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "collapseThreads": true})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "filter": {"inCalendar": ["b"]}})),
            MethodErrorType::InvalidArguments
        );
    }

    /// The three `CalendarEvent/get` calls of apps/web/src/calendar/api.ts.
    #[test]
    fn web_gets() {
        let occurrences = CalendarEventGetArguments::parse(
            map(json!({
                "accountId": "c", "ids": ["h1m9leaaaaal", "h1na9maaaaap"],
                "properties": ["calendarIds", "title", "start", "duration", "utcStart", "utcEnd",
                    "baseEventId", "recurrenceId", "status", "participants",
                    "organizerCalendarAddress", "locations", "isOrigin"],
                "timeZone": "Europe/Madrid"
            })),
            500,
        )
        .unwrap();
        let properties = occurrences.get.properties.unwrap();
        assert!(properties.contains(&CalendarEventProperty::UtcStart));
        assert!(properties.contains(&CalendarEventProperty::Body("title".parse().unwrap())));
        assert_eq!(occurrences.time_zone.as_str(), "Europe/Madrid");

        let series = CalendarEventGetArguments::parse(
            map(json!({
                "accountId": "c", "ids": ["l"],
                "properties": ["calendarIds", "title", "showWithoutTime", "timeZone",
                    "recurrenceRule", "organizerCalendarAddress", "participants", "status", "isOrigin"]
            })),
            500,
        )
        .unwrap();
        assert_eq!(series.time_zone, TimeZoneId::utc());
        assert!(!series.reduce_participants);

        let all = CalendarEventGetArguments::parse(
            map(json!({
                "accountId": "c", "ids": null, "reduceParticipants": true,
                "recurrenceOverridesBefore": "2027-01-01T00:00:00Z",
                "recurrenceOverridesAfter": "2026-01-01T00:00:00Z"
            })),
            500,
        )
        .unwrap();
        assert!(all.reduce_participants);
        assert!(all.recurrence_overrides_before.is_some());

        let kind = |value: Value| {
            CalendarEventGetArguments::parse(map(value), 2)
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"accountId": "c", "properties": ["a/b"]})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "timeZone": ""})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "recurrenceOverridesBefore": "2027-01-01T00:00:00"})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "ids": ["a", "b", "c"]})),
            MethodErrorType::RequestTooLarge
        );
        assert_eq!(
            kind(json!({"accountId": "c", "extra": true})),
            MethodErrorType::InvalidArguments
        );
    }

    /// Stalwart 1.0's `CalendarEvent/get` for one expanded occurrence.
    #[test]
    fn get_response_round_trip() {
        let stalwart = json!({
            "start": "2026-10-19T10:00:00", "title": "Weekly team sync", "duration": "PT45M",
            "recurrenceId": "2026-10-19T10:00:00", "calendarIds": {"b": true},
            "utcStart": "2026-10-19T08:00:00Z", "utcEnd": "2026-10-19T08:45:00Z",
            "baseEventId": "l", "isOrigin": true, "id": "h1m9leaaaaal"
        });
        let mut members = map(stalwart.clone());
        let event = CalendarEvent {
            id: Some("h1m9leaaaaal".parse().unwrap()),
            base_event_id: Some(Some("l".parse().unwrap())),
            calendar_ids: Some(
                serde_json::from_value(members.remove("calendarIds").unwrap()).unwrap(),
            ),
            is_draft: None,
            is_origin: Some(true),
            use_default_alerts: None,
            utc_start: Some("2026-10-19T08:00:00Z".parse().unwrap()),
            utc_end: Some("2026-10-19T08:45:00Z".parse().unwrap()),
            body: body(json!({
                "start": "2026-10-19T10:00:00", "title": "Weekly team sync",
                "duration": "PT45M", "recurrenceId": "2026-10-19T10:00:00"
            })),
        };
        assert_eq!(serde_json::to_value(&event).unwrap(), stalwart);
        let response = GetResponse {
            account_id: "c".parse().unwrap(),
            state: State::new("s0m0a"),
            list: vec![event],
            not_found: vec!["zz".parse::<CalendarEventId>().unwrap()],
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["list"][0]["baseEventId"], "l");
        assert_eq!(value["notFound"], json!(["zz"]));
    }

    /// `createEvent` in apps/web/src/calendar/api.ts with the properties
    /// of `eventProperties` (editing.ts), nulls left out.
    #[test]
    fn web_create() {
        let created = CalendarEventCreate::parse(map(json!({
            "alerts": {"a1": {"@type": "Alert", "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M", "relativeTo": "start"}}},
            "duration": "PT1H",
            "locations": {"l1": {"@type": "Location", "name": "Sala"}},
            "participants": {
                "p1": {"@type": "Participant", "calendarAddress": "mailto:alice@nebulamail.test",
                       "roles": {"owner": true, "attendee": true}, "participationStatus": "accepted"},
                "p2": {"@type": "Participant", "calendarAddress": "mailto:bob@nebulamail.test",
                       "roles": {"attendee": true}, "participationStatus": "needs-action", "expectReply": true}
            },
            "showWithoutTime": false,
            "start": "2026-10-28T10:00:00",
            "title": "Planning",
            "calendarIds": {"b": true},
            "recurrenceRule": {"@type": "RecurrenceRule", "frequency": "weekly", "byDay": [{"day": "mo"}]},
            "timeZone": "Europe/Madrid",
            "organizerCalendarAddress": "mailto:alice@nebulamail.test",
            "description": "Agenda:\n- one\n- two"
        })))
        .unwrap();
        assert!(
            created
                .calendar_ids
                .contains(&IdOrRef::Id("b".parse().unwrap()))
        );
        assert!(!created.is_draft);
        assert_eq!(created.body.get("title"), Some(&json!("Planning")));
        assert!(created.body.get("calendarIds").is_none());
        // A calendar created in the same request.
        let with_ref = CalendarEventCreate::parse(map(json!({
            "calendarIds": {"#cal": true}, "start": "2026-10-28T00:00:00", "showWithoutTime": true,
            "duration": "P1D", "isDraft": true, "useDefaultAlerts": true
        })))
        .unwrap();
        assert!(with_ref.is_draft);
        assert_eq!(with_ref.use_default_alerts, Some(true));
    }

    #[test]
    fn create_errors_list_every_property() {
        let error = CalendarEventCreate::parse(map(json!({
            "id": "x", "baseEventId": "y", "isOrigin": true, "utcStart": "2026-10-28T10:00:00Z",
            "utcEnd": "2026-10-28T11:00:00Z", "calendarIds": {"b": false}, "isDraft": "no",
            "timeZone": "Europe Madrid", "duration": "1h", "@type": "Journal"
        })))
        .unwrap_err();
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        let mut properties = error.properties().to_vec();
        properties.sort();
        assert_eq!(
            properties,
            [
                "@type",
                "baseEventId",
                "calendarIds",
                "duration",
                "id",
                "isDraft",
                "isOrigin",
                "start",
                "timeZone",
                "utcEnd",
                "utcStart"
            ]
        );
        let empty = CalendarEventCreate::parse(map(
            json!({"calendarIds": {}, "start": "2026-10-28T10:00:00"}),
        ))
        .unwrap_err();
        assert_eq!(empty.properties(), ["calendarIds"]);
        let missing =
            CalendarEventCreate::parse(map(json!({"start": "2026-10-28T10:00:00"}))).unwrap_err();
        assert_eq!(missing.properties(), ["calendarIds"]);
        let too_big = CalendarEventCreate::parse(map(json!({
            "calendarIds": {"b": true}, "start": "2026-10-28T10:00:00",
            "description": "x".repeat(2 << 20)
        })))
        .unwrap_err();
        assert_eq!(too_big.kind(), SetErrorType::InvalidProperties);
        // A task needs no start.
        let task = CalendarEventCreate::parse(map(json!({
            "calendarIds": {"c1": true}, "@type": "Task", "title": "Milk",
            "due": "2026-10-30T00:00:00", "showWithoutTime": true, "progress": "needs-action",
            "priority": 5, "relatedTo": {"p@x": {"@type": "Relation", "relation": {"parent": true}}}
        })))
        .unwrap();
        assert_eq!(task.body.get("@type"), Some(&json!("Task")));
        let bad_due = CalendarEventCreate::parse(map(json!({
            "calendarIds": {"c1": true}, "@type": "Task", "due": "2026-10-30"
        })))
        .unwrap_err();
        assert_eq!(bad_due.properties(), ["due"]);
    }

    /// The patches the web sends: `updateEvent` with the form's
    /// properties, alerts (alerts.ts) and answers (invitations.ts).
    #[test]
    fn web_patches() {
        let parse =
            |value: Value| CalendarEventPatch::parse(&serde_json::from_value(value).unwrap());
        let full = parse(json!({
            "alerts": null, "duration": "PT30M", "locations": null, "participants": null,
            "showWithoutTime": false, "start": "2026-10-28T10:30:00", "title": "Moved",
            "calendarIds": {"b": true}, "recurrenceRule": null, "timeZone": "Europe/Madrid",
            "organizerCalendarAddress": null, "description": null, "descriptionContentType": null
        }))
        .unwrap();
        assert!(matches!(full.calendar_ids, Some(SetPatch::Replace(_))));
        assert_eq!(full.body.len(), 12);

        let alerts = parse(json!({
            "alerts/a1/acknowledged": "2026-10-26T20:49:00Z",
            "alerts/snz1": {"@type": "Alert", "action": "display",
                "relatedTo": {"a1": {"@type": "Relation", "relation": {"parent": true}}},
                "trigger": {"@type": "AbsoluteTrigger", "when": "2026-10-26T20:54:00Z"}}
        }))
        .unwrap();
        assert_eq!(alerts.body.len(), 2);

        let answer = parse(json!({"participants/p2/participationStatus": "accepted"})).unwrap();
        assert_eq!(
            answer.body[0].path.tokens(),
            ["participants", "p2", "participationStatus"]
        );

        let moved = parse(json!({"calendarIds/b": null, "calendarIds/c2": true})).unwrap();
        let Some(SetPatch::Change { add, remove }) = moved.calendar_ids else {
            panic!("expected a change")
        };
        assert_eq!((add.len(), remove.len()), (1, 1));

        let kind = |value: Value| parse(value).unwrap_err().kind();
        assert_eq!(
            kind(json!({"utcStart": "2026-10-28T10:00:00Z"})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(kind(json!({"id": "x"})), SetErrorType::InvalidProperties);
        assert_eq!(
            kind(json!({"baseEventId/x": 1})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"calendarIds": {}})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"calendarIds/b": false})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"calendarIds/b/x": true})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(kind(json!({"isDraft/x": true})), SetErrorType::InvalidPatch);
        assert_eq!(
            kind(json!({"start": "tomorrow"})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"alerts": {}, "alerts/a1": null})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(
            kind(json!({"a/b/c/d/e/f/g/h/i/j/k/l/m": 1})),
            SetErrorType::InvalidPatch
        );
    }

    #[test]
    fn set_and_parse_arguments() {
        let args = CalendarEventSetArguments::parse(
            map(json!({"accountId": "c", "destroy": ["h1m9leaaaaal"], "sendSchedulingMessages": true})),
            10,
        )
        .unwrap();
        assert!(args.send_scheduling_messages);
        assert!(
            !CalendarEventSetArguments::parse(map(json!({"accountId": "c"})), 10)
                .unwrap()
                .send_scheduling_messages
        );
        assert!(
            CalendarEventSetArguments::parse(
                map(json!({"accountId": "c", "sendSchedulingMessages": "yes"})),
                10
            )
            .is_err()
        );

        // `loadInvitation` in apps/web/src/calendar/invitations.ts.
        let parse = CalendarEventParseArguments::parse(
            map(json!({"accountId": "c", "blobIds": ["Gb1"]})),
            10,
        )
        .unwrap();
        assert_eq!(parse.blob_ids.len(), 1);
        assert!(parse.properties.is_none());
        assert_eq!(
            CalendarEventParseArguments::parse(
                map(json!({"accountId": "c", "blobIds": ["a", "b"]})),
                1
            )
            .unwrap_err()
            .kind(),
            MethodErrorType::RequestTooLarge
        );
        assert!(CalendarEventParseArguments::parse(map(json!({"accountId": "c"})), 1).is_err());
    }

    #[test]
    fn parse_response_shape() {
        let mut parsed = BTreeMap::new();
        parsed.insert(
            "Gb1".parse().unwrap(),
            vec![CalendarEvent {
                body: body(json!({"@type": "Event", "uid": "u1", "method": "request",
                    "start": "2026-10-28T10:00:00", "title": "Invite"})),
                ..CalendarEvent::default()
            }],
        );
        let response = CalendarEventParseResponse {
            account_id: "c".parse().unwrap(),
            parsed,
            not_parsable: vec!["Gb2".parse().unwrap()],
            not_found: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "c",
                "parsed": {"Gb1": [{"@type": "Event", "uid": "u1", "method": "request",
                    "start": "2026-10-28T10:00:00", "title": "Invite"}]},
                "notParsable": ["Gb2"],
                "notFound": null
            })
        );
    }

    #[test]
    fn property_names() {
        assert_eq!(
            "utcStart".parse::<CalendarEventProperty>().unwrap(),
            CalendarEventProperty::UtcStart
        );
        assert_eq!(
            "iCalendar"
                .parse::<CalendarEventProperty>()
                .unwrap()
                .as_str(),
            "iCalendar"
        );
        for known in CalendarEventProperty::JMAP {
            assert_eq!(
                &known.as_str().parse::<CalendarEventProperty>().unwrap(),
                known
            );
        }
        assert!("".parse::<CalendarEventProperty>().is_err());
    }
}
