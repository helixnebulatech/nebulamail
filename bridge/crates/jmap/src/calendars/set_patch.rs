// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `CalendarEvent/set` changes to a JSCalendar event, without I/O (the
//! iCalendar side is `nebula_groupware::model::edit`):
//!
//! - **The stored event** (a series, or an event without repetition):
//!   the patch applied as is ([`JsBody::apply`]), the way the web patches
//!   (`participants/<k>/participationStatus`, `alerts/<id>/acknowledged`,
//!   `alerts/<id>`, `recurrenceOverrides/…`, whole members).
//! - **One occurrence** (its synthetic id): the patch goes into the
//!   series' `recurrenceOverrides` under the occurrence's original start,
//!   as Stalwart does: with its current `start` and `duration` if the
//!   override has none, members that only the series can have
//!   (`recurrenceRule`, `uid`, `organizerCalendarAddress`…) left out, and
//!   deep paths kept as patch keys (RFC 8984 §4.3.5) or applied inside a
//!   member the override already has whole. A task's occurrence also
//!   keeps its `due`.
//! - **Deleting one occurrence**: `excluded: true` (an `EXDATE`).

use nebula_groupware::model::edit::OccurrenceTarget;
use nebula_jmap_proto::object::js_body::{BodyPatch, JsBody, JsCalendar};
use nebula_jmap_proto::{SetError, SetErrorType};
use serde_json::{Map, Value};

/// Members only the series has (Stalwart's "inherited" properties): an
/// occurrence patch that sets them is ignored, as Stalwart does.
const SERIES_ONLY: &[&str] = &[
    "@type",
    "method",
    "organizerCalendarAddress",
    "privacy",
    "prodId",
    "recurrenceId",
    "recurrenceIdTimeZone",
    "sentBy",
    "uid",
    "recurrenceOverrides",
    "recurrenceRule",
    "recurrenceRules",
    "excludedRecurrenceRules",
    "relatedTo",
];

fn not_editable() -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description("this event can't be edited here")
}

/// The event as a checked body (the stored one should always be one).
pub(crate) fn checked(event: Map<String, Value>) -> Result<JsBody<JsCalendar>, SetError> {
    JsBody::new(event).map_err(|_| not_editable())
}

/// Applies a patch to the stored event. `None` if nothing changes.
pub(crate) fn patch_series(
    event: Map<String, Value>,
    patches: &[BodyPatch],
) -> Result<Option<Map<String, Value>>, SetError> {
    let mut body = checked(event)?;
    let before = body.clone();
    body.apply(patches)?;
    // A task may lose its start (RFC 8984 §5.2); an event can't.
    if body.get("@type").and_then(Value::as_str) != Some("Task") && body.get("start").is_none() {
        return Err(
            SetError::invalid_properties(["start"]).with_description("an event must have a start")
        );
    }
    Ok((body != before).then(|| body.into_map()))
}

/// The override of the occurrence (its existing key if the series has
/// one), created if missing.
fn override_of<'a>(
    event: &'a mut Map<String, Value>,
    target: &OccurrenceTarget,
) -> Result<&'a mut Map<String, Value>, SetError> {
    let overrides = event
        .entry("recurrenceOverrides")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(overrides) = overrides else {
        return Err(not_editable());
    };
    match overrides
        .entry(target.key.clone())
        .or_insert_with(|| Value::Object(Map::new()))
    {
        Value::Object(map) => Ok(map),
        _ => Err(not_editable()),
    }
}

/// JSON pointer of a patch path, as an RFC 8984 patch key.
fn pointer_key(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| t.replace('~', "~0").replace('/', "~1"))
        .collect::<Vec<_>>()
        .join("/")
}

/// Sets `value` at `tokens` inside `target`; `false` if a parent is
/// missing or not an object.
fn set_deep(target: &mut Map<String, Value>, tokens: &[String], value: Value) -> bool {
    let Some((last, parents)) = tokens.split_last() else {
        return false;
    };
    let mut at = target;
    for token in parents {
        match at.get_mut(token) {
            Some(Value::Object(next)) => at = next,
            _ => return false,
        }
    }
    if value.is_null() {
        at.remove(last);
    } else {
        at.insert(last.clone(), value);
    }
    true
}

/// The series' value at the patch key `key` (a JSON pointer, `~0`/`~1`
/// escaped, as [`pointer_key`] writes them).
fn series_value<'a>(series: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    let mut tokens = key
        .split('/')
        .map(|t| t.replace("~1", "/").replace("~0", "~"));
    let first = tokens.next()?;
    let mut at = series.get(&first)?;
    for token in tokens {
        at = at.as_object()?.get(&token)?;
    }
    Some(at)
}

/// Whether setting `value` at `key` in a new override changes nothing:
/// the series has that value there (or has nothing there and `value` is
/// `null`). Repeating the series' value is inheriting it.
fn same_as_series(series: &Map<String, Value>, key: &str, value: &Value) -> bool {
    match series_value(series, key) {
        Some(inherited) => inherited == value,
        None => value.is_null(),
    }
}

/// Sets `value` at `tokens` in an override (RFC 8984 §4.3.5 patch keys):
/// inside a member the override has as a key (whole, or a shorter patch
/// key), or as its own key, replacing the deeper keys under it. Never two
/// keys where one is inside the other.
///
/// # Errors
/// `invalidProperties` on the path when the member it goes into is
/// `null`, not an object, or lacks a level on the way.
fn set_in_override(
    instance: &mut Map<String, Value>,
    tokens: &[String],
    value: Value,
) -> Result<(), SetError> {
    let invalid = || {
        SetError::invalid_properties([pointer_key(tokens)])
            .with_description("invalid patch: its parent is not an object in this occurrence")
    };
    let ancestor = (1..tokens.len()).find_map(|at| {
        let prefix = tokens.get(..at)?;
        let key = pointer_key(prefix);
        instance.contains_key(&key).then_some((key, at))
    });
    if let Some((key, at)) = ancestor {
        let rest = tokens.get(at..).unwrap_or_default();
        return match instance.get_mut(&key) {
            Some(Value::Object(whole)) => {
                if set_deep(whole, rest, value) {
                    Ok(())
                } else {
                    Err(invalid())
                }
            }
            _ => Err(invalid()),
        };
    }
    let key = pointer_key(tokens);
    let prefix = format!("{key}/");
    instance.retain(|existing, _| !existing.starts_with(&prefix));
    instance.insert(key, value);
    Ok(())
}

/// Applies a patch to one occurrence of the series `event`. `None` if
/// nothing changes (also when the patch only repeats the series).
pub(crate) fn patch_occurrence(
    event: Map<String, Value>,
    target: &OccurrenceTarget,
    patches: &[BodyPatch],
) -> Result<Option<Map<String, Value>>, SetError> {
    let mut event = event;
    let before = event.clone();
    let existed = before
        .get("recurrenceOverrides")
        .and_then(Value::as_object)
        .is_some_and(|overrides| overrides.contains_key(&target.key));
    let instance = override_of(&mut event, target)?;
    if instance.get("excluded").and_then(Value::as_bool) == Some(true) {
        return Err(SetError::not_found());
    }
    let original = instance.clone();
    for patch in patches {
        let tokens = patch.path.tokens();
        let Some(first) = tokens.first() else {
            continue;
        };
        if SERIES_ONLY.contains(&first.as_str())
            || (first == "participants" && tokens.get(2).is_some_and(|t| t == "calendarAddress"))
        {
            continue;
        }
        set_in_override(instance, tokens, patch.value.clone())?;
    }
    if !existed {
        // A new override keeps only what differs from the series (and
        // from the occurrence's own time, which it gets anyway).
        let own_time = [
            ("start", &target.start),
            ("duration", &target.duration),
            ("due", &target.due),
        ];
        instance.retain(|key, value| {
            let own = own_time.iter().any(|(name, time)| {
                key == name && time.is_some() && time.as_deref() == value.as_str()
            });
            !own && !same_as_series(&before, key, value)
        });
    }
    if *instance == original {
        return Ok(None);
    }
    // Stalwart's rule: the instance keeps its time unless the patch moves
    // it (an override without them would follow the series).
    if !instance.contains_key("start")
        && let Some(start) = &target.start
    {
        instance.insert("start".to_owned(), Value::from(start.clone()));
    }
    if !instance.contains_key("duration")
        && let Some(duration) = &target.duration
    {
        instance.insert("duration".to_owned(), Value::from(duration.clone()));
    }
    // A task's occurrence keeps its due the same way (completing one
    // occurrence of a recurring task.7 "Tareas").
    if !instance.contains_key("due")
        && let Some(due) = &target.due
    {
        instance.insert("due".to_owned(), Value::from(due.clone()));
    }
    let body = checked(event)?;
    let event = body.into_map();
    Ok((event != before).then_some(event))
}

/// Excludes one occurrence of the series `event` (`EXDATE`).
pub(crate) fn exclude_occurrence(
    event: Map<String, Value>,
    target: &OccurrenceTarget,
) -> Result<Map<String, Value>, SetError> {
    let mut event = event;
    let instance = override_of(&mut event, target)?;
    instance.clear();
    instance.insert("excluded".to_owned(), Value::Bool(true));
    Ok(checked(event)?.into_map())
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::object::CalendarEventPatch;
    use serde_json::json;

    use super::*;

    fn patches(value: Value) -> Vec<BodyPatch> {
        CalendarEventPatch::parse(&serde_json::from_value(value).unwrap())
            .unwrap()
            .body
    }

    fn series() -> Map<String, Value> {
        json!({
            "@type": "Event", "uid": "w@example.com", "title": "Weekly",
            "start": "2026-10-19T10:00:00", "timeZone": "Europe/Madrid", "duration": "PT30M",
            "recurrenceRule": {"frequency": "weekly", "count": 4},
            "participants": {
                "pa": {"@type": "Participant", "calendarAddress": "mailto:alice@example.com",
                       "participationStatus": "accepted", "roles": {"owner": true}},
                "pb": {"@type": "Participant", "calendarAddress": "mailto:bob@example.com",
                       "participationStatus": "needs-action"}
            },
            "alerts": {"a1": {"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"}}}
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn target() -> OccurrenceTarget {
        OccurrenceTarget {
            recurring: true,
            key: "2026-10-26T10:00:00".to_owned(),
            start: Some("2026-10-26T10:00:00".to_owned()),
            duration: Some("PT30M".to_owned()),
            due: None,
        }
    }

    /// `answerEvent`, `dismissAlert` and `snoozeAlert` on the series.
    #[test]
    fn series_patches_by_path() {
        let answered = patch_series(
            series(),
            &patches(json!({"participants/pb/participationStatus": "accepted"})),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            answered["participants"]["pb"]["participationStatus"],
            "accepted"
        );
        let dismissed = patch_series(
            series(),
            &patches(json!({
                "alerts/a1/acknowledged": "2026-10-19T08:00:00Z",
                "alerts/snz1": {"@type": "Alert", "action": "display",
                    "relatedTo": {"a1": {"@type": "Relation", "relation": {"parent": true}}},
                    "trigger": {"@type": "AbsoluteTrigger", "when": "2026-10-19T08:10:00Z"}}
            })),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            dismissed["alerts"]["a1"]["acknowledged"],
            "2026-10-19T08:00:00Z"
        );
        assert!(dismissed["alerts"]["snz1"].is_object());
        let removed = patch_series(dismissed, &patches(json!({"alerts/snz1": null})))
            .unwrap()
            .unwrap();
        assert!(removed["alerts"].get("snz1").is_none());
        // The same value again: no write.
        assert!(
            patch_series(series(), &patches(json!({"title": "Weekly"})))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            patch_series(
                series(),
                &patches(json!({"alerts/zz/acknowledged": "2026-10-19T08:00:00Z"}))
            )
            .unwrap_err()
            .kind(),
            SetErrorType::InvalidPatch
        );
    }

    /// The editor's "this event" (`eventProperties` with `occurrence`),
    /// a drag (`start` + `participants`) and an answer to one occurrence.
    #[test]
    fn occurrence_patches_become_overrides() {
        let edited = patch_occurrence(
            series(),
            &target(),
            &patches(json!({
                "title": "Moved", "start": "2026-10-26T12:00:00",
                "organizerCalendarAddress": "mailto:alice@example.com",
                "description": null, "alerts": null
            })),
        )
        .unwrap()
        .unwrap();
        let instance = &edited["recurrenceOverrides"]["2026-10-26T10:00:00"];
        assert_eq!(instance["title"], "Moved");
        assert_eq!(instance["start"], "2026-10-26T12:00:00");
        assert_eq!(instance["duration"], "PT30M");
        assert!(instance.get("organizerCalendarAddress").is_none());
        assert!(instance["description"].is_null() && instance["alerts"].is_null());
        assert_eq!(edited["title"], "Weekly");

        let answered = patch_occurrence(
            series(),
            &target(),
            &patches(json!({"participants/pb/participationStatus": "declined"})),
        )
        .unwrap()
        .unwrap();
        let instance = &answered["recurrenceOverrides"]["2026-10-26T10:00:00"];
        assert_eq!(instance["participants/pb/participationStatus"], "declined");
        assert_eq!(instance["start"], "2026-10-26T10:00:00");
        // With the override's own participants, the answer goes inside
        // (they differ from the series': a guest more).
        let mut guests = series()["participants"].clone();
        guests["pc"] = json!({"@type": "Participant", "calendarAddress": "mailto:[EMAIL]"});
        let with_guests = patch_occurrence(
            series(),
            &target(),
            &patches(json!({"participants": guests})),
        )
        .unwrap()
        .unwrap();
        let again = patch_occurrence(
            with_guests,
            &target(),
            &patches(json!({"participants/pb/participationStatus": "tentative"})),
        )
        .unwrap()
        .unwrap();
        let instance = &again["recurrenceOverrides"]["2026-10-26T10:00:00"];
        assert_eq!(
            instance["participants"]["pb"]["participationStatus"],
            "tentative"
        );
        assert!(
            instance
                .get("participants/pb/participationStatus")
                .is_none()
        );
    }

    /// An occurrence patch that changes nothing makes no override: series
    /// members are ignored, repeating the series is inheriting it (also
    /// deep, like an answer, and `null` for what the series lacks).
    #[test]
    fn patches_that_change_nothing() {
        let unchanged =
            |value: Value| patch_occurrence(series(), &target(), &patches(value)).unwrap();
        assert!(unchanged(json!({"organizerCalendarAddress": "mailto:[EMAIL]"})).is_none());
        assert!(unchanged(json!({"title": "Weekly"})).is_none());
        assert!(
            unchanged(json!({"participants/pb/participationStatus": "needs-action"})).is_none()
        );
        assert!(unchanged(json!({"description": null})).is_none());
        assert!(unchanged(json!({"start": "2026-10-26T10:00:00"})).is_none());
        let changed = unchanged(json!({"title": "Otro", "description": null})).unwrap();
        let instance = changed["recurrenceOverrides"]["2026-10-26T10:00:00"]
            .as_object()
            .unwrap();
        assert_eq!(instance["title"], "Otro");
        assert_eq!(instance["start"], "2026-10-26T10:00:00");
        assert_eq!(instance["duration"], "PT30M");
        assert!(!instance.contains_key("description"));
        // An existing override: repeating its own value changes nothing.
        let again =
            patch_occurrence(changed, &target(), &patches(json!({"title": "Otro"}))).unwrap();
        assert!(again.is_none());
    }

    /// A patch key never sits inside another one of the same override.
    #[test]
    fn override_keys_do_not_overlap() {
        let snooze = json!({"@type": "Alert", "action": "display",
            "trigger": {"@type": "AbsoluteTrigger", "when": "2026-10-26T08:10:00Z"}});
        let step = |event: Map<String, Value>, value: Value| {
            patch_occurrence(event, &target(), &patches(value))
                .unwrap()
                .unwrap()
        };
        let created = step(series(), json!({"alerts/snz1": snooze}));
        let acknowledged = step(
            created,
            json!({"alerts/snz1/acknowledged": "2026-10-26T08:11:00Z"}),
        );
        let instance = &acknowledged["recurrenceOverrides"]["2026-10-26T10:00:00"];
        assert_eq!(
            instance["alerts/snz1"]["acknowledged"],
            "2026-10-26T08:11:00Z"
        );
        assert!(instance.get("alerts/snz1/acknowledged").is_none());
        let removed = step(acknowledged, json!({"alerts/snz1": null}));
        let instance = removed["recurrenceOverrides"]["2026-10-26T10:00:00"]
            .as_object()
            .unwrap();
        let alert_keys: Vec<&String> = instance
            .keys()
            .filter(|k| k.starts_with("alerts"))
            .collect();
        assert_eq!(alert_keys, ["alerts/snz1"]);
        assert!(instance["alerts/snz1"].is_null());
        // The result still converts.
        assert!(checked(removed).is_ok());

        // Into a whole member that lacks a level, or a `null` one: refused,
        // not written beside it.
        let with_alerts = step(series(), json!({"alerts": {}}));
        let error = patch_occurrence(
            with_alerts,
            &target(),
            &patches(json!({"alerts/snz1/acknowledged": "2026-10-26T08:11:00Z"})),
        )
        .unwrap_err();
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        let without = step(series(), json!({"alerts": null}));
        let error = patch_occurrence(
            without,
            &target(),
            &patches(json!({"alerts/a1/acknowledged": "2026-10-26T08:11:00Z"})),
        )
        .unwrap_err();
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
    }

    #[test]
    fn excluded_occurrences() {
        let gone = exclude_occurrence(series(), &target()).unwrap();
        assert_eq!(
            gone["recurrenceOverrides"]["2026-10-26T10:00:00"],
            json!({"excluded": true})
        );
        assert_eq!(
            patch_occurrence(gone, &target(), &patches(json!({"title": "x"})))
                .unwrap_err()
                .kind(),
            SetErrorType::NotFound
        );
    }
}
