// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The JSCalendar (RFC 8984) and JSContact (RFC 9553) part of a
//! `CalendarEvent` or `ContactCard`: everything that is not a JMAP
//! property.
//!
//! It is not modelled member by member (calcard in `groupware` converts
//! it): it is a JSON object with size, depth and count limits
//! ([`JsonLimits`]), no JMAP member names, and the types of the members
//! the bridge itself reads (`uid`, `start`, `timeZone`, `@type`…) checked.
//! Other members, including vendor ones, pass through as they are.

use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

use super::bounded::{JsonLimits, apply_patch, check_object, check_value_at};
use crate::error::SetError;
use crate::macros::serde_via_str;
use crate::references::JsonPointer;
use crate::types::{Duration, LocalDateTime, TimeZoneId, UnsignedInt, UtcDate};

/// A property name in `properties` that is not one of the JMAP ones: a
/// JSCalendar or JSContact member (`title`, `@type`, `iCalendar`,
/// `example.com:extra`).
///
/// 1 to 128 characters from `A-Z a-z 0-9 @ : . _ -`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsPropertyName(String);

/// Why a string is not a property name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid property name")]
pub struct JsPropertyNameError;

impl JsPropertyName {
    /// Longest name accepted.
    pub const MAX_LEN: usize = 128;

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for JsPropertyName {
    type Err = JsPropertyNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = (1..=Self::MAX_LEN).contains(&s.len())
            && s.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'@' | b':' | b'.' | b'_' | b'-')
            });
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(JsPropertyNameError)
        }
    }
}

impl fmt::Display for JsPropertyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for JsPropertyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JsPropertyName({})", self.0)
    }
}

serde_via_str!(JsPropertyName);

/// What kind of body: JSCalendar or JSContact. Sealed.
pub trait BodyKind: sealed::Sealed + fmt::Debug + Clone + PartialEq + Eq {
    /// The `@type`s a body may have (if it has one), the usual first.
    const TYPES: &'static [&'static str];
    /// Limits for the whole body.
    const LIMITS: JsonLimits;
    /// JMAP-level properties: never part of the body.
    const JMAP_PROPERTIES: &'static [&'static str];

    /// Checks a top-level member (`null` means "remove it").
    ///
    /// # Errors
    /// Why the value is wrong.
    fn check_member(name: &str, value: &Value) -> Result<(), String>;

    /// Checks a patch deeper than the top level (`tokens` has two or
    /// more). Most paths are free-form.
    ///
    /// # Errors
    /// Why the path or the value is wrong.
    fn check_deep(tokens: &[String], value: &Value) -> Result<(), String> {
        let _ = (tokens, value);
        Ok(())
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::JsCalendar {}
    impl Sealed for super::JsContact {}
}

/// Marker for JSCalendar `Event` and `Task` bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JsCalendar {}

/// Marker for JSContact `Card` bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum JsContact {}

/// How a well-known member is checked.
#[derive(Clone, Copy)]
enum Rule {
    /// `@type`: one of the kind's types, never removed nor changed.
    Type,
    /// A string without control characters, never removed (`uid`).
    Line,
    /// A string or `null`.
    Text,
    /// A string, never removed (`version`).
    RequiredText,
    /// A `LocalDateTime` or `null`.
    LocalOrNull,
    /// A `TimeZoneId` or `null`.
    Zone,
    /// A `Duration` or `null`.
    Duration,
    /// A `UTCDateTime` or `null`.
    Utc,
    /// A Boolean or `null`.
    Bool,
    /// An `UnsignedInt` or `null`.
    Unsigned,
    /// An object or `null`.
    Object,
    /// An array or `null`.
    Array,
    /// An object whose keys are `LocalDateTime`s, or `null`
    /// (`recurrenceOverrides`).
    OverridesMap,
}

/// Longest `uid` accepted (they are usually a UUID URN or an email-like
/// string).
const MAX_UID: usize = 1024;

fn check_rule(rule: Rule, types: &[&str], value: &Value) -> Result<(), String> {
    let ok = match (rule, value) {
        (Rule::Type, Value::String(text)) => types.contains(&text.as_str()),
        (Rule::Line, Value::String(text)) => {
            !text.is_empty() && text.len() <= MAX_UID && !text.chars().any(char::is_control)
        }
        (Rule::LocalOrNull, Value::String(text)) => text.parse::<LocalDateTime>().is_ok(),
        (Rule::Zone, Value::String(text)) => text.parse::<TimeZoneId>().is_ok(),
        (Rule::Duration, Value::String(text)) => text.parse::<Duration>().is_ok(),
        (Rule::Utc, Value::String(text)) => text.parse::<UtcDate>().is_ok(),
        (Rule::Unsigned, Value::Number(number)) => {
            number.as_u64().and_then(UnsignedInt::new).is_some()
        }
        (Rule::OverridesMap, Value::Object(map)) => {
            map.keys().all(|key| key.parse::<LocalDateTime>().is_ok())
        }
        (
            Rule::Text
            | Rule::LocalOrNull
            | Rule::Zone
            | Rule::Duration
            | Rule::Utc
            | Rule::Bool
            | Rule::Unsigned
            | Rule::Object
            | Rule::Array
            | Rule::OverridesMap,
            Value::Null,
        )
        | (Rule::Text | Rule::RequiredText, Value::String(_))
        | (Rule::Bool, Value::Bool(_))
        | (Rule::Object, Value::Object(_))
        | (Rule::Array, Value::Array(_)) => true,
        _ => false,
    };
    if ok {
        return Ok(());
    }
    Err(match rule {
        Rule::Type => format!("must be one of {types:?}"),
        Rule::Line => "must be a non-empty string without control characters".to_owned(),
        Rule::Text => "must be a string or null".to_owned(),
        Rule::RequiredText => "must be a string".to_owned(),
        Rule::LocalOrNull => "must be a LocalDateTime or null".to_owned(),
        Rule::Zone => "must be a time zone id or null".to_owned(),
        Rule::Duration => "must be a Duration or null".to_owned(),
        Rule::Utc => "must be a UTCDateTime or null".to_owned(),
        Rule::Bool => "must be a boolean or null".to_owned(),
        Rule::Unsigned => "must be an UnsignedInt or null".to_owned(),
        Rule::Object => "must be an object or null".to_owned(),
        Rule::Array => "must be an array or null".to_owned(),
        Rule::OverridesMap => "must be an object keyed by LocalDateTime, or null".to_owned(),
    })
}

/// JSCalendar members the bridge reads (RFC 8984 and its "bis" update, as
/// Stalwart and calcard write them).
const JSCALENDAR_RULES: &[(&str, Rule)] = &[
    ("@type", Rule::Type),
    ("uid", Rule::Line),
    // A task may lose its start (an event never: checked on create).
    ("start", Rule::LocalOrNull),
    ("recurrenceId", Rule::LocalOrNull),
    ("timeZone", Rule::Zone),
    ("recurrenceIdTimeZone", Rule::Zone),
    ("duration", Rule::Duration),
    ("showWithoutTime", Rule::Bool),
    ("excluded", Rule::Bool),
    ("title", Rule::Text),
    ("description", Rule::Text),
    ("descriptionContentType", Rule::Text),
    ("status", Rule::Text),
    ("freeBusyStatus", Rule::Text),
    ("privacy", Rule::Text),
    ("color", Rule::Text),
    ("method", Rule::Text),
    ("organizerCalendarAddress", Rule::Text),
    ("created", Rule::Utc),
    ("updated", Rule::Utc),
    ("sequence", Rule::Unsigned),
    ("participants", Rule::Object),
    ("alerts", Rule::Object),
    ("locations", Rule::Object),
    ("virtualLocations", Rule::Object),
    ("links", Rule::Object),
    ("keywords", Rule::Object),
    ("categories", Rule::Object),
    ("relatedTo", Rule::Object),
    ("localizations", Rule::Object),
    ("replyTo", Rule::Object),
    ("recurrenceRule", Rule::Object),
    ("excludedRecurrenceRule", Rule::Object),
    ("recurrenceRules", Rule::Array),
    ("excludedRecurrenceRules", Rule::Array),
    ("recurrenceOverrides", Rule::OverridesMap),
    ("iCalendar", Rule::Object),
    ("priority", Rule::Unsigned),
    // Task (RFC 8984 §5.2).
    ("due", Rule::LocalOrNull),
    ("estimatedDuration", Rule::Duration),
    ("percentComplete", Rule::Unsigned),
    ("progress", Rule::Text),
    ("progressUpdated", Rule::Utc),
];

/// JSContact members the bridge reads (RFC 9553).
const JSCONTACT_RULES: &[(&str, Rule)] = &[
    ("@type", Rule::Type),
    ("version", Rule::RequiredText),
    ("uid", Rule::Line),
    ("kind", Rule::Text),
    ("language", Rule::Text),
    ("prodId", Rule::Text),
    ("created", Rule::Utc),
    ("updated", Rule::Utc),
    ("name", Rule::Object),
    ("speakToAs", Rule::Object),
    ("members", Rule::Object),
    ("emails", Rule::Object),
    ("phones", Rule::Object),
    ("addresses", Rule::Object),
    ("organizations", Rule::Object),
    ("titles", Rule::Object),
    ("notes", Rule::Object),
    ("nicknames", Rule::Object),
    ("anniversaries", Rule::Object),
    ("media", Rule::Object),
    ("onlineServices", Rule::Object),
    ("links", Rule::Object),
    ("keywords", Rule::Object),
    ("relatedTo", Rule::Object),
    ("personalInfo", Rule::Object),
    ("preferredLanguages", Rule::Object),
    ("calendars", Rule::Object),
    ("schedulingAddresses", Rule::Object),
    ("directories", Rule::Object),
    ("cryptoKeys", Rule::Object),
    ("localizations", Rule::Object),
    ("vCard", Rule::Object),
];

fn check_with(
    rules: &[(&str, Rule)],
    types: &[&str],
    name: &str,
    value: &Value,
) -> Result<(), String> {
    match rules.iter().find(|(known, _)| *known == name) {
        Some((_, rule)) => check_rule(*rule, types, value),
        None => Ok(()),
    }
}

impl BodyKind for JsCalendar {
    const TYPES: &'static [&'static str] = &["Event", "Task"];
    const LIMITS: JsonLimits = JsonLimits::JSCALENDAR;
    const JMAP_PROPERTIES: &'static [&'static str] = &[
        "id",
        "baseEventId",
        "calendarIds",
        "isDraft",
        "isOrigin",
        "useDefaultAlerts",
        "utcStart",
        "utcEnd",
    ];

    fn check_member(name: &str, value: &Value) -> Result<(), String> {
        check_with(JSCALENDAR_RULES, Self::TYPES, name, value)?;
        check_calendar_inside(name, value)
    }

    fn check_deep(tokens: &[String], value: &Value) -> Result<(), String> {
        check_calendar_path(tokens, value)
    }
}

/// `Participant` members with a type (RFC 8984 §4.4.6). `scheduleStatus`
/// and `sendTo` are left free.
const PARTICIPANT_RULES: &[(&str, Rule)] = &[
    ("name", Rule::Text),
    ("email", Rule::Text),
    ("description", Rule::Text),
    ("kind", Rule::Text),
    ("roles", Rule::Object),
    ("locationId", Rule::Text),
    ("language", Rule::Text),
    ("participationStatus", Rule::Text),
    ("participationComment", Rule::Text),
    ("expectReply", Rule::Bool),
    ("scheduleAgent", Rule::Text),
    ("scheduleForceSend", Rule::Bool),
    ("scheduleSequence", Rule::Unsigned),
    ("scheduleUpdated", Rule::Utc),
    ("sentBy", Rule::Text),
    ("invitedBy", Rule::Text),
    ("delegatedTo", Rule::Object),
    ("delegatedFrom", Rule::Object),
    ("memberOf", Rule::Object),
    ("links", Rule::Object),
    ("progress", Rule::Text),
    ("progressUpdated", Rule::Utc),
    ("percentComplete", Rule::Unsigned),
    // JSCalendar "bis", as calcard writes it.
    ("calendarAddress", Rule::Text),
];

/// Override patch pointers that "MUST be ignored" (RFC 8984 §4.3.5): not
/// checked either.
const OVERRIDE_IGNORED: &[&str] = &[
    "@type",
    "excludedRecurrenceRules",
    "method",
    "privacy",
    "prodId",
    "recurrenceId",
    "recurrenceIdTimeZone",
    "recurrenceOverrides",
    "recurrenceRules",
    "relatedTo",
    "replyTo",
    "sentBy",
    "timeZones",
    "uid",
];

/// The members inside a top-level member that has typed members of its
/// own: each override (RFC 8984 §4.3.5) and each participant (§4.4.6).
fn check_calendar_inside(name: &str, value: &Value) -> Result<(), String> {
    let Value::Object(map) = value else {
        return Ok(());
    };
    if name != "recurrenceOverrides" && name != "participants" {
        return Ok(());
    }
    for (key, inner) in map {
        check_calendar_path(&[name.to_owned(), key.clone()], inner)
            .map_err(|why| format!("{key}: {why}"))?;
    }
    Ok(())
}

/// A participant (a whole one, or `null` to remove it).
fn check_participant(value: &Value) -> Result<(), String> {
    match value {
        Value::Null => Ok(()),
        Value::Object(members) => members.iter().try_for_each(|(name, inner)| {
            check_with(PARTICIPANT_RULES, &[], name, inner).map_err(|why| format!("{name} {why}"))
        }),
        _ => Err("a participant must be an object or null".to_owned()),
    }
}

/// An override's `PatchObject` (RFC 8984 §1.4.9 and §4.3.5): each key is a
/// path from the event, its value checked as a patch at that path.
fn check_override(value: &Value) -> Result<(), String> {
    let patches = match value {
        Value::Null => return Ok(()),
        Value::Object(patches) => patches,
        _ => return Err("an override must be an object or null".to_owned()),
    };
    for (key, inner) in patches {
        let path =
            JsonPointer::from_patch_key(key).map_err(|_| format!("{key:?} is not a patch key"))?;
        let tokens = path.tokens();
        if tokens
            .first()
            .is_some_and(|first| OVERRIDE_IGNORED.contains(&first.as_str()))
        {
            continue;
        }
        check_calendar_path(tokens, inner).map_err(|why| format!("{key} {why}"))?;
    }
    Ok(())
}

/// Checks `value` at `tokens` of an event or task (a patch path, or a
/// member of an override): the members with a rule have its type, also
/// inside overrides and participants (RFC 8620 §5.3 `invalidProperties`).
/// Other paths are free-form.
fn check_calendar_path(tokens: &[String], value: &Value) -> Result<(), String> {
    match tokens {
        [name] => JsCalendar::check_member(name, value),
        // An override is keyed by the occurrence's original start.
        [first, key, rest @ ..] if first == "recurrenceOverrides" => {
            if key.parse::<LocalDateTime>().is_err() {
                return Err("recurrenceOverrides keys must be LocalDateTime".to_owned());
            }
            match rest {
                [] => check_override(value),
                [next, ..] if OVERRIDE_IGNORED.contains(&next.as_str()) => Ok(()),
                _ => check_calendar_path(rest, value),
            }
        }
        [first, _] if first == "participants" => check_participant(value),
        [first, _, name] if first == "participants" => {
            check_with(PARTICIPANT_RULES, &[], name, value)
        }
        _ => Ok(()),
    }
}

impl BodyKind for JsContact {
    const TYPES: &'static [&'static str] = &["Card"];
    const LIMITS: JsonLimits = JsonLimits::JSCONTACT;
    const JMAP_PROPERTIES: &'static [&'static str] = &["id", "addressBookIds"];

    fn check_member(name: &str, value: &Value) -> Result<(), String> {
        check_with(JSCONTACT_RULES, Self::TYPES, name, value)
    }
}

/// Why a body is not acceptable. Each problem names the member, so a
/// `/set` can list them in `invalidProperties`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid body: {}", .problems.iter().map(|(name, why)| format!("{name}: {why}")).collect::<Vec<_>>().join("; "))]
pub struct JsBodyError {
    problems: Vec<(String, String)>,
}

impl JsBodyError {
    fn one(name: impl Into<String>, why: impl Into<String>) -> Self {
        Self {
            problems: vec![(name.into(), why.into())],
        }
    }

    /// The members with problems, and why.
    #[must_use]
    pub fn problems(&self) -> &[(String, String)] {
        &self.problems
    }

    /// As an `invalidProperties` `SetError`.
    #[must_use]
    pub fn to_set_error(&self) -> SetError {
        SetError::invalid_properties(self.problems.iter().map(|(name, _)| name.clone()))
            .with_description(self.to_string())
    }
}

/// A JSCalendar or JSContact body: checked, bounded, without JMAP
/// members. Serializes as the object itself.
#[derive(Clone, PartialEq, Eq)]
pub struct JsBody<K: BodyKind> {
    members: Map<String, Value>,
    kind: PhantomData<K>,
}

impl<K: BodyKind> fmt::Debug for JsBody<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Bodies hold personal data: only the member names.
        f.debug_struct("JsBody")
            .field("type", &K::TYPES)
            .field("members", &self.members.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl<K: BodyKind> Default for JsBody<K> {
    fn default() -> Self {
        Self {
            members: Map::new(),
            kind: PhantomData,
        }
    }
}

impl<K: BodyKind> JsBody<K> {
    /// Checks a body.
    ///
    /// # Errors
    /// [`JsBodyError`] listing every JMAP member, badly typed member, or
    /// the limit it goes over.
    pub fn new(members: Map<String, Value>) -> Result<Self, JsBodyError> {
        check_body::<K>(&members)?;
        Ok(Self {
            members,
            kind: PhantomData,
        })
    }

    /// The members.
    #[must_use]
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.members
    }

    /// The members, by value.
    #[must_use]
    pub fn into_map(self) -> Map<String, Value> {
        self.members
    }

    /// One member.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.members.get(name)
    }

    /// The `uid`, if it has one (always a valid string when present).
    #[must_use]
    pub fn uid(&self) -> Option<&str> {
        self.members.get("uid").and_then(Value::as_str)
    }

    /// Only these members (for `properties`). Unknown names are left out.
    #[must_use]
    pub fn select<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut members = Map::new();
        for name in names {
            if let Some(value) = self.members.get(name) {
                members.insert(name.to_owned(), value.clone());
            }
        }
        Self {
            members,
            kind: PhantomData,
        }
    }

    /// Applies parsed patches, then checks the result again (so a patched
    /// body is as valid as a new one).
    ///
    /// # Errors
    /// `invalidPatch` for paths that don't fit the body;
    /// `invalidProperties` if the result breaks a rule or a limit.
    pub fn apply(&mut self, patches: &[BodyPatch]) -> Result<(), SetError> {
        let mut members = self.members.clone();
        for patch in patches {
            apply_patch(&mut members, &patch.path, patch.value.clone())?;
        }
        check_body::<K>(&members).map_err(|error| error.to_set_error())?;
        self.members = members;
        Ok(())
    }
}

fn check_body<K: BodyKind>(members: &Map<String, Value>) -> Result<(), JsBodyError> {
    let mut problems = Vec::new();
    for (name, value) in members {
        if K::JMAP_PROPERTIES.contains(&name.as_str()) {
            problems.push((name.clone(), "not part of the body".to_owned()));
        } else if let Err(why) = K::check_member(name, value) {
            problems.push((name.clone(), why));
        } else if value.is_null() {
            // A stored body has no nulls at the top level: null means
            // "not there" (RFC 8984 §1.3).
            problems.push((name.clone(), "must not be null".to_owned()));
        }
    }
    if !problems.is_empty() {
        return Err(JsBodyError { problems });
    }
    check_object(members, &K::LIMITS).map_err(|error| JsBodyError::one("body", error.to_string()))
}

impl<K: BodyKind> Serialize for JsBody<K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.members.serialize(serializer)
    }
}

/// One change to a body, from a `/set` update: set the value at `path`
/// (`null` removes it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyPatch {
    /// Where (never a JMAP property; at most [`BodyPatch::MAX_TOKENS`]
    /// deep).
    pub path: JsonPointer,
    /// The new value (bounded like the body).
    pub value: Value,
}

impl BodyPatch {
    /// Deepest path accepted (`recurrenceOverrides/<start>/participants/
    /// <id>/participationStatus` is 5).
    pub const MAX_TOKENS: usize = 12;

    /// Checks one patch of a `K` body. Top-level members follow the same
    /// rules as in a new body (`null` is allowed where it means "remove").
    ///
    /// # Errors
    /// `invalidPatch` for a path that is too deep; `invalidProperties` for
    /// a bad value, a JMAP property or a value over the limits.
    pub(crate) fn parse<K: BodyKind>(
        key: &str,
        path: JsonPointer,
        value: Value,
    ) -> Result<Self, SetError> {
        let tokens = path.tokens();
        if tokens.len() > Self::MAX_TOKENS {
            return Err(SetError::invalid_patch(format!("{key:?} is too deep")));
        }
        let invalid = |why: String| {
            SetError::invalid_properties([key]).with_description(format!("{key}: {why}"))
        };
        match tokens {
            [] => return Err(SetError::invalid_patch("empty path")),
            // An event does not become a task, nor the other way round.
            [name] if name == "@type" => {
                return Err(invalid("can't be changed".to_owned()));
            }
            [name] => K::check_member(name, &value).map_err(invalid)?,
            _ => K::check_deep(tokens, &value).map_err(invalid)?,
        }
        let depth = tokens.len().saturating_sub(1);
        check_value_at(&value, depth, &K::LIMITS).map_err(|error| invalid(error.to_string()))?;
        Ok(Self { path, value })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    fn object(value: Value) -> Map<String, Value> {
        let Value::Object(map) = value else {
            panic!("not an object")
        };
        map
    }

    #[test]
    fn property_names() {
        for good in ["title", "@type", "iCalendar", "example.com:extra", "x-1_y"] {
            assert_eq!(good.parse::<JsPropertyName>().unwrap().as_str(), good);
        }
        for bad in ["", "a/b", "a b", "título", &"x".repeat(129)] {
            assert!(bad.parse::<JsPropertyName>().is_err(), "{bad:?}");
        }
    }

    /// What Stalwart's `CalendarEvent/get` returns, minus the JMAP members.
    #[test]
    fn stalwart_event_is_a_valid_body() {
        let body = JsBody::<JsCalendar>::new(object(json!({
            "title": "Weekly team sync",
            "timeZone": "Europe/Madrid",
            "start": "2026-10-19T10:00:00",
            "recurrenceRule": {"frequency": "weekly", "count": 12},
            "@type": "Event",
            "updated": "2026-10-24T01:50:28Z",
            "locations": {"l1": {"name": "Sala Andrómeda", "@type": "Location"}},
            "uid": "nebula-seed-weekly-sync",
            "alerts": {"a1": {"action": "display", "@type": "Alert",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT10M", "relativeTo": "start"}}},
            "duration": "PT45M",
            "recurrenceOverrides": {"2026-10-26T10:00:00": {"title": "Moved"}},
            "iCalendar": {"properties": [["x-foo", {}, "text", "bar"]]},
            "example.com:vendor": [1, 2, 3]
        })))
        .unwrap();
        assert_eq!(body.uid(), Some("nebula-seed-weekly-sync"));
        let selected = body.select(["title", "start", "nope"]);
        assert_eq!(
            serde_json::to_value(&selected).unwrap(),
            json!({"title": "Weekly team sync", "start": "2026-10-19T10:00:00"})
        );
        assert!(!format!("{body:?}").contains("Andrómeda"));
    }

    #[test]
    fn bad_bodies_list_every_member() {
        let error = JsBody::<JsCalendar>::new(object(json!({
            "@type": "Journal",
            "id": "e1",
            "calendarIds": {"c": true},
            "start": "2026-10-19T10:00:00Z",
            "timeZone": "Not a zone",
            "duration": "1 hour",
            "uid": "",
            "sequence": -1,
            "recurrenceOverrides": {"tomorrow": {}},
            "title": null,
            "participants": []
        })))
        .unwrap_err();
        let mut names: Vec<_> = error.problems().iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "@type",
                "calendarIds",
                "duration",
                "id",
                "participants",
                "recurrenceOverrides",
                "sequence",
                "start",
                "timeZone",
                "title",
                "uid"
            ]
        );
        let set_error = error.to_set_error();
        assert_eq!(set_error.kind(), SetErrorType::InvalidProperties);
        assert_eq!(set_error.properties().len(), 11);

        assert!(JsBody::<JsContact>::new(object(json!({"@type": "Event"}))).is_err());
        assert!(JsBody::<JsContact>::new(object(json!({"addressBookIds": {}}))).is_err());
        assert!(JsBody::<JsContact>::new(object(json!({"version": 1}))).is_err());
        assert!(
            JsBody::<JsContact>::new(object(json!({
                "@type": "Card", "version": "1.0", "uid": "urn:uuid:1", "kind": "individual",
                "name": {"full": "Bob"}, "created": "2026-10-24T01:50:28Z"
            })))
            .is_ok()
        );
    }

    #[test]
    fn limits() {
        let huge = "x".repeat(JsonLimits::JSCALENDAR.max_size);
        let error = JsBody::<JsCalendar>::new(object(json!({"description": huge}))).unwrap_err();
        assert_eq!(error.problems()[0].0, "body");
        let mut deep = json!(1);
        for _ in 0..40 {
            deep = json!({"a": deep});
        }
        assert!(JsBody::<JsCalendar>::new(object(json!({"x": deep}))).is_err());
        // JSContact allows 4 MiB (data: photos).
        let photo = format!("data:image/jpeg;base64,{}", "A".repeat(3 << 20));
        assert!(
            JsBody::<JsContact>::new(object(
                json!({"media": {"p": {"kind": "photo", "uri": photo}}})
            ))
            .is_ok()
        );
    }

    #[test]
    fn patches() {
        let parse = |key: &str, value: Value| {
            BodyPatch::parse::<JsCalendar>(key, JsonPointer::from_patch_key(key).unwrap(), value)
        };
        // What the web sends (alerts.ts, editing.ts, invitations.ts).
        let good = [
            ("alerts/a1/acknowledged", json!("2026-10-26T20:49:00Z")),
            ("alerts/snz1", Value::Null),
            ("participants/p1/participationStatus", json!("accepted")),
            (
                "recurrenceOverrides/2026-10-26T10:00:00",
                json!({"title": "x"}),
            ),
            ("recurrenceOverrides/2026-10-26T10:00:00", Value::Null),
            (
                "recurrenceOverrides/2026-10-26T10:00:00/start",
                json!("2026-10-26T11:00:00"),
            ),
            ("timeZone", Value::Null),
            ("descriptionContentType", Value::Null),
            ("recurrenceRule", Value::Null),
            ("start", json!("2026-10-26T10:00:00")),
            ("title", json!("New")),
        ];
        for (key, value) in good {
            assert!(parse(key, value).is_ok(), "{key}");
        }
        let kind = |key: &str, value: Value| parse(key, value).unwrap_err().kind();
        // A task may lose its start; due and progress are task members.
        for (key, value) in [
            ("start", Value::Null),
            ("due", json!("2026-10-30T00:00:00")),
            ("progress", json!("completed")),
            ("percentComplete", json!(100)),
        ] {
            assert!(parse(key, value).is_ok(), "{key}");
        }
        assert_eq!(
            kind("due", json!("tomorrow")),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind("@type", json!("Event")),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind("start", json!("2026-10-26")),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind("@type", json!("Task")),
            SetErrorType::InvalidProperties
        );
        assert_eq!(kind("uid", Value::Null), SetErrorType::InvalidProperties);
        assert_eq!(
            kind("recurrenceOverrides/next-week", json!({})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind("recurrenceOverrides/2026-10-26T10:00:00", json!(1)),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind("a/".repeat(13).trim_end_matches('/'), json!(1)),
            SetErrorType::InvalidPatch
        );
        let mut deep = json!(1);
        for _ in 0..31 {
            deep = json!({"a": deep});
        }
        assert_eq!(kind("x/y", deep), SetErrorType::InvalidProperties);
    }

    /// RFC 8620 §5.3 `invalidProperties` also for values inside overrides
    /// (RFC 8984 §4.3.5 `PatchObject`s) and participants (§4.4.6):
    /// `UTCDateTime` (§1.4.4) needs its `Z`, a Duration follows §1.4.6.
    #[test]
    fn nested_values_are_checked() {
        let parse = |key: &str, value: Value| {
            BodyPatch::parse::<JsCalendar>(key, JsonPointer::from_patch_key(key).unwrap(), value)
        };
        let at = "recurrenceOverrides/2026-11-09T09:00:00";
        for (key, value) in [
            (
                format!("{at}/progressUpdated"),
                json!("2026-11-08T10:00:00"),
            ),
            (format!("{at}/estimatedDuration"), json!("1h")),
            (format!("{at}/participants/b/progressUpdated"), json!("x")),
            (at.to_owned(), json!({"estimatedDuration": "1h"})),
            (
                at.to_owned(),
                json!({"participants/b/progressUpdated": "x"}),
            ),
            (
                at.to_owned(),
                json!({"participants/b": {"expectReply": "yes"}}),
            ),
            (
                at.to_owned(),
                json!({"participants": {"b": {"scheduleSequence": -1}}}),
            ),
            (at.to_owned(), json!({"excluded": "true"})),
            (at.to_owned(), json!({"a~2": 1})),
            ("participants/b/progressUpdated".to_owned(), json!("x")),
            (
                "participants/b".to_owned(),
                json!({"percentComplete": "50"}),
            ),
            ("participants/b".to_owned(), json!(1)),
            (
                "participants".to_owned(),
                json!({"b": {"progressUpdated": "2026-11-08T10:00:00"}}),
            ),
            (
                "recurrenceOverrides".to_owned(),
                json!({"2026-11-09T09:00:00": {"due": "tomorrow"}}),
            ),
        ] {
            assert_eq!(
                parse(&key, value.clone()).unwrap_err().kind(),
                SetErrorType::InvalidProperties,
                "{key}: {value}"
            );
        }
        // Valid ones, and what RFC 8984 §4.3.5 says to ignore, still pass.
        for (key, value) in [
            (
                format!("{at}/progressUpdated"),
                json!("2026-11-08T10:00:00Z"),
            ),
            (format!("{at}/estimatedDuration"), json!("P1W2D")),
            (format!("{at}/estimatedDuration"), Value::Null),
            (format!("{at}/participants/b/progressUpdated"), Value::Null),
            (format!("{at}/participants/b/example.com:x"), json!(1)),
            (at.to_owned(), json!({"excluded": true})),
            (at.to_owned(), json!({})),
            (
                at.to_owned(),
                json!({"@type": "Group", "uid": 7, "title": null}),
            ),
            (
                at.to_owned(),
                json!({"participants/b/participationStatus": "accepted"}),
            ),
            (format!("{at}/uid"), json!(1)),
            ("participants/b".to_owned(), Value::Null),
            (
                "participants/b".to_owned(),
                json!({"@type": "Participant", "progressUpdated": "2026-11-08T10:00:00Z",
                       "percentComplete": 50, "roles": {"attendee": true}}),
            ),
        ] {
            assert!(parse(&key, value.clone()).is_ok(), "{key}: {value}");
        }
        // A whole body too (create, and the result of an update).
        let error = JsBody::<JsCalendar>::new(object(json!({
            "@type": "Task", "uid": "t",
            "recurrenceOverrides": {"2026-11-09T09:00:00": {"estimatedDuration": "1h"}},
            "participants": {"b": {"progressUpdated": "x"}}
        })))
        .unwrap_err();
        let mut names: Vec<_> = error.problems().iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["participants", "recurrenceOverrides"]);
    }

    #[test]
    fn apply_checks_the_result() {
        let mut body = JsBody::<JsCalendar>::new(object(json!({
            "@type": "Event", "start": "2026-10-19T10:00:00",
            "alerts": {"a1": {"trigger": {"offset": "-PT10M"}}}
        })))
        .unwrap();
        let patch = |key: &str, value: Value| {
            BodyPatch::parse::<JsCalendar>(key, JsonPointer::from_patch_key(key).unwrap(), value)
                .unwrap()
        };
        body.apply(&[
            patch("alerts/a1/acknowledged", json!("2026-10-26T20:49:00Z")),
            patch("title", json!("x")),
        ])
        .unwrap();
        assert_eq!(body.get("title"), Some(&json!("x")));
        assert_eq!(
            body.apply(&[patch("missing/x", json!(1))])
                .unwrap_err()
                .kind(),
            SetErrorType::InvalidPatch
        );
        // Unchanged after a failed patch.
        assert!(body.get("missing").is_none());
    }
}
