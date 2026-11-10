// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! When an event's alerts are due: the `VALARM`s
//! of each occurrence, read as JSCalendar alerts (RFC 8984 §4.5) so the
//! ids are the keys the web sees in `alerts`.
//!
//! - **Ids**: the alarm's `JSID`, or `k1`, `k2`… by position among the
//!   component's alarms (calcard's rule when it converts to JSCalendar).
//! - **Which ring**: `ACTION:DISPLAY` and `AUDIO` (Apple writes `AUDIO`;
//!   JSCalendar has no such action and reads it as `display`); no
//!   `EMAIL`, `PROCEDURE` or unknown actions.
//! - **Triggers**: an offset from the start (default) or the end of the
//!   occurrence (`RELATED=END`); weeks and days are nominal (RFC 8984
//!   §1.4.6: same wall-clock time in the event's zone across a DST
//!   change), hours, minutes and seconds exact. An absolute trigger
//!   (`VALUE=DATE-TIME`, how snoozing works) rings once, also in a series;
//!   its occurrence is the one its parent alarm rang for (`RELATED-TO`,
//!   RFC 8984 §4.5.2; or RFC 9074 `RELTYPE=SNOOZE` from the parent), else
//!   the first one that has not ended at that time.
//! - **Seen**: an alarm whose `ACKNOWLEDGED` (RFC 9074) is at or after its
//!   time does not ring (in a series, earlier occurrences count as seen,
//!   later ones still ring).
//! - **Repeats** (RFC 5545 §3.6.6, `REPEAT` and `DURATION`, which
//!   JSCalendar lacks: calcard keeps them under the alert's `iCalendar`):
//!   the alarm rings again `REPEAT` times, `DURATION` apart, each one a
//!   [`DueAlert`] with its [`DueAlert::repetition`]. At most
//!   [`AlarmLimits::repeats`] times, and only `DURATION`s of
//!   [`MIN_REPEAT_INTERVAL`] or more (shorter ones ring once). An
//!   `ACKNOWLEDGED` at or after the first ring silences the repeats too
//!   (dismissing stops them).
//! - **Tasks** (`VTODO`): the same rules; "end" is the `DUE` (RFC 5545
//!   §3.8.6.3 `RELATED=END`, RFC 8984 `relativeTo: "end"`), "start" the
//!   `DTSTART`, or the `DUE` of a task without one (calcard's instance:
//!   clients such as tasks.org write start-relative triggers on tasks with
//!   only a `DUE`). A task without dates has no occurrence and never
//!   rings; one that is `COMPLETED` or `CANCELLED` (that occurrence's
//!   override, in a series) does not ring either.
//!
//! Limits (a calendar object can come from anyone: an invitation lands in
//! the calendar by itself):
//! - the occurrences come from [`ParsedCalendar::instances`] (capped there);
//! - at most [`MAX_ALARMS_PER_COMPONENT`] alarms per component, the first
//!   ones (their `kN` ids do not change; a snooze added after the 64th
//!   alarm does not ring);
//! - offset alarms are only worked out for occurrences near the window
//!   ([`AlarmLimits::lead`] before its end, [`AlarmLimits::lag`] after its
//!   start, as the plan does with single events); absolute ones (snooze)
//!   look at every occurrence of their component;
//! - at most [`AlarmLimits::work`] trigger evaluations per object (then it
//!   stops with what it has), and at most `max` alerts are returned.
//!
//! CPU work: call it off the async threads.

use std::collections::{HashMap, HashSet};

use calcard::common::PartialDateTime;
use calcard::common::timezone::Tz;
use calcard::icalendar::{
    ICalendarAction, ICalendarComponent, ICalendarComponentType, ICalendarParameterName,
    ICalendarParameterValue, ICalendarProperty, ICalendarRelated, ICalendarRelationshipType,
    ICalendarValue,
};

use super::calendar::{
    Instance, ParsedCalendar, RecurrenceKey, Zone, component_zone, is_finished_task, naive_in,
};
use super::convert::{Format, guarded};
use crate::ConvertError;

/// Longest alert id kept (the JMAP `CalendarAlert` allows 255 bytes).
const MAX_ALERT_ID: usize = 255;

/// Shortest `DURATION` between repeats of an alarm, in seconds: a
/// reminder every few seconds is noise (and many messages).
pub const MIN_REPEAT_INTERVAL: i64 = 60;

/// Repeats of one alarm, at most, by default.
pub const DEFAULT_MAX_REPEATS: u32 = 10;

/// Alarms of one component looked at, at most (the first ones).
pub const MAX_ALARMS_PER_COMPONENT: usize = 64;

/// Trigger evaluations per object, at most, by default: with the window
/// filter a normal event needs a handful.
pub const MAX_ALARM_EVALUATIONS: usize = 10_000;

/// How long before its start an occurrence is looked at, by default: 8
/// days and the largest zone offset.
pub const DEFAULT_LEAD: i64 = (8 * 24 + 14) * 3600;

/// How long after its end an occurrence is looked at, by default: 1 day
/// and the largest zone offset.
pub const DEFAULT_LAG: i64 = (24 + 14) * 3600;

/// How many alerts [`alerts_in`] returns, and how much work it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlarmLimits {
    /// Alerts per object, at most (every repeat counts).
    pub max: usize,
    /// Repeats of one alarm (`REPEAT`), at most: 0 rings each alarm once.
    pub repeats: u32,
    /// Seconds before the window an occurrence may start and still have
    /// an offset alarm in it (an alert this long before the start).
    pub lead: i64,
    /// Seconds after the window an occurrence may have ended and still
    /// have an offset alarm in it.
    pub lag: i64,
    /// Trigger evaluations, at most.
    pub work: usize,
}

impl AlarmLimits {
    /// `max` alerts with `repeats`, and the default window margins and
    /// work.
    #[must_use]
    pub const fn new(max: usize, repeats: u32) -> Self {
        Self {
            max,
            repeats,
            lead: DEFAULT_LEAD,
            lag: DEFAULT_LAG,
            work: MAX_ALARM_EVALUATIONS,
        }
    }
}

/// One alert of one occurrence, due at `fire_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueAlert {
    /// The key of the alert in the event's `alerts`.
    pub alert_id: String,
    /// When it rings, UTC seconds.
    pub fire_at: i64,
    /// The UTC start of the occurrence.
    pub occurrence: i64,
    /// The occurrence of a recurring event (`None` for a single event).
    pub recurrence: Option<RecurrenceKey>,
    /// Which ring of the alarm: 0 the first, then its repeats
    /// (`REPEAT`/`DURATION`).
    pub repetition: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// From the start or the end: nominal days, then exact seconds.
    Offset { days: i64, seconds: i64, end: bool },
    /// At this UTC time.
    At(i64),
}

#[derive(Debug, Clone)]
struct Alarm {
    id: String,
    trigger: Trigger,
    acknowledged: Option<i64>,
    /// The alarm it hangs from (a snooze's original).
    parent: Option<String>,
    /// `REPEAT` and `DURATION` (seconds), if both are there and usable.
    repeat: Option<(u32, i64)>,
}

/// `REPEAT` and `DURATION` of an alarm (RFC 5545: both or neither). A
/// `DURATION` under [`MIN_REPEAT_INTERVAL`], a missing one, or a `REPEAT`
/// of 0 or less: no repeats.
fn repeat(component: &ICalendarComponent) -> Option<(u32, i64)> {
    let count = component
        .property(&ICalendarProperty::Repeat)?
        .values
        .first()?
        .as_integer()?;
    let every = match component
        .property(&ICalendarProperty::Duration)?
        .values
        .first()?
    {
        ICalendarValue::Duration(duration) if !duration.neg => duration.as_seconds(),
        _ => return None,
    };
    let count = u32::try_from(count).ok().filter(|count| *count > 0)?;
    (every >= MIN_REPEAT_INTERVAL).then_some((count, every))
}

fn text_values<'a>(
    component: &'a ICalendarComponent,
    name: &'a ICalendarProperty,
) -> impl Iterator<Item = (&'a str, bool, bool)> + 'a {
    // (value, is a parent link, is a snooze link)
    component.properties(name).filter_map(|entry| {
        let value = entry.values.first()?.as_text()?;
        let mut parent = true;
        let mut snooze = false;
        for kind in entry.parameters(&ICalendarParameterName::Reltype) {
            match kind {
                ICalendarParameterValue::Reltype(ICalendarRelationshipType::Parent) => {}
                ICalendarParameterValue::Reltype(ICalendarRelationshipType::Snooze) => {
                    parent = false;
                    snooze = true;
                }
                _ => parent = false,
            }
        }
        Some((value, parent, snooze))
    })
}

fn date_time(component: &ICalendarComponent, name: &ICalendarProperty) -> Option<i64> {
    component
        .property(name)?
        .values
        .first()?
        .as_partial_date_time()
        .filter(|value| value.has_date_and_time())?
        .to_timestamp()
}

fn trigger(component: &ICalendarComponent) -> Option<Trigger> {
    let entry = component.property(&ICalendarProperty::Trigger)?;
    match entry.values.first()? {
        ICalendarValue::Duration(duration) => {
            let sign = if duration.neg { -1 } else { 1 };
            let days = i64::from(duration.weeks)
                .saturating_mul(7)
                .saturating_add(i64::from(duration.days))
                .saturating_mul(sign);
            let seconds = i64::from(duration.hours)
                .saturating_mul(3600)
                .saturating_add(i64::from(duration.minutes).saturating_mul(60))
                .saturating_add(i64::from(duration.seconds))
                .saturating_mul(sign);
            let end = entry
                .parameters(&ICalendarParameterName::Related)
                .any(|related| {
                    matches!(
                        related,
                        ICalendarParameterValue::Related(ICalendarRelated::End)
                    )
                });
            Some(Trigger::Offset { days, seconds, end })
        }
        ICalendarValue::PartialDateTime(value) if value.has_date_and_time() => {
            value.to_timestamp().map(Trigger::At)
        }
        _ => None,
    }
}

/// Whether the alarm is one to show (`DISPLAY`, `AUDIO`; JSCalendar's
/// default `display` if there is no `ACTION`).
fn rings(component: &ICalendarComponent) -> bool {
    match component
        .property(&ICalendarProperty::Action)
        .and_then(|entry| entry.values.first())
    {
        None => true,
        Some(ICalendarValue::Action(action)) => {
            matches!(action, ICalendarAction::Display | ICalendarAction::Audio)
        }
        Some(_) => false,
    }
}

/// The display alarms of an event component, with calcard's ids.
fn alarms(parsed: &ParsedCalendar, component: &ICalendarComponent) -> Vec<Alarm> {
    let ical = parsed.icalendar();
    let children: Vec<&ICalendarComponent> = component
        .component_ids
        .iter()
        .filter_map(|id| usize::try_from(*id).ok())
        .filter_map(|index| ical.components.get(index))
        .filter(|child| child.component_type == ICalendarComponentType::VAlarm)
        .take(MAX_ALARMS_PER_COMPONENT)
        .collect();
    // UID → id, for RELATED-TO (calcard writes the parent's UID there).
    let mut ids: Vec<(String, Option<&str>)> = Vec::with_capacity(children.len());
    for (position, child) in children.iter().enumerate() {
        let id = child
            .jsid()
            .map_or_else(|| format!("k{}", position.saturating_add(1)), str::to_owned);
        ids.push((id, child.uid()));
    }
    let id_of = |value: &str| {
        ids.iter()
            .find(|(_, uid)| *uid == Some(value))
            .map_or_else(|| value.to_owned(), |(id, _)| id.clone())
    };
    let mut snoozed_by: HashMap<String, String> = HashMap::new();
    for ((id, _), child) in ids.iter().zip(&children) {
        for (value, _, snooze) in text_values(child, &ICalendarProperty::RelatedTo) {
            if snooze {
                snoozed_by.insert(id_of(value), id.clone());
            }
        }
    }
    let mut out = Vec::new();
    for ((id, _), child) in ids.iter().zip(&children) {
        if !rings(child)
            || id.is_empty()
            || id.len() > MAX_ALERT_ID
            || id.chars().any(char::is_control)
        {
            continue;
        }
        let Some(trigger) = trigger(child) else {
            continue;
        };
        let parent = text_values(child, &ICalendarProperty::RelatedTo)
            .find(|(_, parent, _)| *parent)
            .map(|(value, _, _)| id_of(value))
            .or_else(|| snoozed_by.get(id).cloned());
        out.push(Alarm {
            id: id.clone(),
            trigger,
            acknowledged: date_time(child, &ICalendarProperty::Acknowledged),
            parent,
            repeat: repeat(child),
        });
    }
    out
}

/// `naive` (wall-clock seconds) in `tz` as UTC seconds.
fn to_utc(tz: Tz, naive: i64) -> Option<i64> {
    PartialDateTime::from_naive_timestamp(naive)
        .to_date_time()
        .and_then(|dt| dt.to_date_time_with_tz(tz))
        .map(|zoned| zoned.timestamp())
}

/// When an offset trigger rings for an occurrence.
fn offset_time(instance: &Instance, tz: Tz, days: i64, seconds: i64, end: bool) -> Option<i64> {
    let base = if end {
        instance.utc_end
    } else {
        instance.utc_start
    };
    let shifted = if days == 0 {
        base
    } else {
        let naive = naive_in(tz, base)?.checked_add(days.checked_mul(86_400)?)?;
        to_utc(tz, naive)?
    };
    shifted.checked_add(seconds)
}

fn is_recurring(parsed: &ParsedCalendar) -> bool {
    parsed.icalendar().components.iter().any(|component| {
        matches!(
            component.component_type,
            ICalendarComponentType::VEvent | ICalendarComponentType::VTodo
        ) && component.is_recurrent_or_override()
    })
}

/// The rings of an alarm first due at `first` (it and its repeats, at most
/// `repeats` of them) that fall in `[from, to)`, with their number.
fn ring_times(alarm: &Alarm, first: i64, repeats: u32, (from, to): (i64, i64)) -> Vec<(u32, i64)> {
    let (count, every) = alarm
        .repeat
        .map_or((0, 0), |(count, every)| (count.min(repeats), every));
    (0..=count)
        .map_while(|repetition| {
            i64::from(repetition)
                .checked_mul(every)
                .and_then(|later| first.checked_add(later))
                .map(|at| (repetition, at))
        })
        .take_while(|(_, at)| *at < to)
        .filter(|(_, at)| *at >= from)
        .collect()
}

/// The display alerts of `parsed` that ring in `[from, to)` (UTC seconds),
/// at most `max`, sorted by time, with up to [`DEFAULT_MAX_REPEATS`]
/// repeats per alarm. Floating times are read in `zone`.
///
/// # Errors
/// [`ConvertError::Panicked`] if calcard fails.
pub fn alerts_between(
    parsed: &ParsedCalendar,
    zone: Zone,
    window: (i64, i64),
    max: usize,
) -> Result<Vec<DueAlert>, ConvertError> {
    alerts_in(
        parsed,
        zone,
        window,
        AlarmLimits::new(max, DEFAULT_MAX_REPEATS),
    )
}

/// [`alerts_between`] with its limits.
///
/// # Errors
/// [`ConvertError::Panicked`] if calcard fails.
pub fn alerts_in(
    parsed: &ParsedCalendar,
    zone: Zone,
    window: (i64, i64),
    limits: AlarmLimits,
) -> Result<Vec<DueAlert>, ConvertError> {
    alerts_counted(parsed, zone, window, limits).map(|(out, _)| out)
}

/// The budget of trigger evaluations ran out.
struct OutOfWork;

/// The trigger evaluations left, counted down.
struct Budget(usize);

impl Budget {
    /// Takes one evaluation; `false` once there are none left.
    fn take(&mut self) -> bool {
        match self.0.checked_sub(1) {
            Some(left) => {
                self.0 = left;
                true
            }
            None => false,
        }
    }
}

/// The alarms of every component with occurrences (not only those near
/// the window: a snooze needs its component's), with the component's zone.
fn component_alarms(
    parsed: &ParsedCalendar,
    list: &[Instance],
    zone: Zone,
) -> Vec<(u32, Tz, Vec<Alarm>)> {
    let components = &parsed.icalendar().components;
    let mut out: Vec<(u32, Tz, Vec<Alarm>)> = Vec::new();
    for instance in list {
        let index = instance.component();
        if out.iter().any(|(known, _, _)| *known == index) {
            continue;
        }
        let entry = match usize::try_from(index)
            .ok()
            .and_then(|position| components.get(position))
        {
            // A task already done (or cancelled) needs no reminder; in a
            // series, an occurrence completed by its override.
            Some(component) if !is_finished_task(component) => (
                index,
                component_zone(component).unwrap_or(zone.tz()),
                alarms(parsed, component),
            ),
            _ => (index, zone.tz(), Vec::new()),
        };
        out.push(entry);
    }
    out
}

/// The latest occurrence among `own` whose offset alarm (`days`,
/// `seconds`, `end`) rang at or before `when`; [`OutOfWork`] when the budget
/// ran out.
fn rang_for<'a>(
    own: &[&'a Instance],
    tz: Tz,
    (days, seconds, end): (i64, i64, bool),
    when: i64,
    budget: &mut Budget,
) -> Result<Option<&'a Instance>, OutOfWork> {
    let mut found: Option<&Instance> = None;
    for instance in own {
        if !budget.take() {
            return Err(OutOfWork);
        }
        if offset_time(instance, tz, days, seconds, end).is_some_and(|rang| rang <= when)
            && found.is_none_or(|best| instance.utc_start >= best.utc_start)
        {
            found = Some(instance);
        }
    }
    Ok(found)
}

/// [`alerts_in`], and how many trigger evaluations it used (at most
/// `limits.work`).
///
/// # Errors
/// [`ConvertError::Panicked`] if calcard fails.
pub fn alerts_counted(
    parsed: &ParsedCalendar,
    zone: Zone,
    (from, to): (i64, i64),
    limits: AlarmLimits,
) -> Result<(Vec<DueAlert>, usize), ConvertError> {
    let max = limits.max;
    let instances = parsed.instances(zone)?;
    guarded(Format::ICalendar, || {
        let recurring = is_recurring(parsed);
        let mut budget = Budget(limits.work);
        let mut out: Vec<DueAlert> = Vec::new();
        let mut seen: HashSet<(String, i64)> = HashSet::new();
        let mut push = |alarm: &Alarm, first: i64, instance: &Instance, out: &mut Vec<DueAlert>| {
            if alarm.acknowledged.is_some_and(|seen_at| seen_at >= first) {
                return;
            }
            let rings = ring_times(alarm, first, limits.repeats, (from, to));
            // Only what rings in the window is remembered, so `seen` stays
            // as small as `out`.
            if rings.is_empty() || !seen.insert((alarm.id.clone(), instance.utc_start)) {
                return;
            }
            for (repetition, fire_at) in rings {
                out.push(DueAlert {
                    alert_id: alarm.id.clone(),
                    fire_at,
                    occurrence: instance.utc_start,
                    recurrence: recurring.then_some(instance.key),
                    repetition,
                });
            }
        };
        let by_component = component_alarms(parsed, &instances.list, zone);
        let near = |instance: &Instance| {
            instance.utc_start.saturating_sub(limits.lead) < to
                && instance.utc_end.saturating_add(limits.lag) >= from
        };
        let mut exhausted = false;
        // Offsets: every occurrence near the window.
        'offsets: for instance in instances.list.iter().filter(|instance| near(instance)) {
            let Some((_, tz, alarms)) = by_component
                .iter()
                .find(|(index, _, _)| *index == instance.component())
            else {
                continue;
            };
            for alarm in alarms {
                if let Trigger::Offset { days, seconds, end } = alarm.trigger {
                    if !budget.take() {
                        exhausted = true;
                        break 'offsets;
                    }
                    if let Some(fire_at) = offset_time(instance, *tz, days, seconds, end) {
                        push(alarm, fire_at, instance, &mut out);
                    }
                }
            }
            if out.len() >= max {
                break;
            }
        }
        // Absolute triggers: once, for one occurrence of their component.
        'absolute: for (component, tz, alarms) in &by_component {
            if exhausted {
                break;
            }
            let own: Vec<&Instance> = instances
                .list
                .iter()
                .filter(|instance| instance.component() == *component)
                .collect();
            for alarm in alarms {
                let Trigger::At(when) = alarm.trigger else {
                    continue;
                };
                let parent = alarm
                    .parent
                    .as_ref()
                    .and_then(|id| alarms.iter().find(|a| &a.id == id));
                let by_parent = match parent.map(|parent| parent.trigger) {
                    Some(Trigger::Offset { days, seconds, end }) => {
                        let Ok(found) =
                            rang_for(&own, *tz, (days, seconds, end), when, &mut budget)
                        else {
                            exhausted = true;
                            break 'absolute;
                        };
                        found
                    }
                    _ => None,
                };
                let instance = by_parent
                    .or_else(|| own.iter().find(|instance| instance.utc_end > when).copied())
                    .or_else(|| own.last().copied());
                if let Some(instance) = instance {
                    push(alarm, when, instance, &mut out);
                }
            }
        }
        if exhausted {
            tracing::debug!("calendar alerts: work limit reached");
        }
        out.sort_by(|a, b| {
            (a.fire_at, &a.alert_id, a.repetition).cmp(&(b.fire_at, &b.alert_id, b.repetition))
        });
        out.truncate(max);
        Ok((out, limits.work.saturating_sub(budget.0)))
    })
}

/// A zone by name, for callers that only have the collection's `TZID`
/// (falls back to UTC).
#[must_use]
pub fn zone_or_utc(name: Option<&str>) -> Zone {
    name.and_then(Zone::named).unwrap_or(Zone::UTC)
}

#[cfg(test)]
mod tests {
    use super::super::calendar::EventOptions;
    use super::*;

    /// `YYYY-MM-DDTHH:MM` as seconds since the epoch, as if UTC.
    fn at(text: &str) -> i64 {
        let numbers: Vec<i64> = text
            .split(['-', 'T', ':'])
            .map(|part| part.parse().unwrap())
            .collect();
        let [y, m, d, h, mi] = numbers[..] else {
            panic!("bad date {text}")
        };
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        (era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + mi * 60
    }

    fn ical(events: &str) -> ParsedCalendar {
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n{}END:VCALENDAR\r\n",
            events.replace('\n', "\r\n")
        );
        ParsedCalendar::parse(text.as_bytes()).unwrap()
    }

    type Row = (String, i64, i64, Option<i64>);

    fn due(parsed: &ParsedCalendar, from: &str, to: &str) -> Vec<Row> {
        alerts_between(parsed, Zone::UTC, (at(from), at(to)), 100)
            .unwrap()
            .into_iter()
            .map(|a| {
                (
                    a.alert_id,
                    a.fire_at,
                    a.occurrence,
                    a.recurrence.map(RecurrenceKey::naive),
                )
            })
            .collect()
    }

    const SINGLE: &str = "BEGIN:VEVENT\nUID:s@example.com\nDTSTAMP:20261001T000000Z\n\
        DTSTART;TZID=Europe/Madrid:20261019T100000\nDTEND;TZID=Europe/Madrid:20261019T110000\n\
        SUMMARY:Single\n\
        BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:a\nTRIGGER:-PT15M\nEND:VALARM\n\
        BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:b\nTRIGGER;RELATED=END:PT5M\nEND:VALARM\n\
        BEGIN:VALARM\nACTION:EMAIL\nDESCRIPTION:c\nSUMMARY:c\nATTENDEE:mailto:a@example.com\n\
        TRIGGER:-PT1H\nEND:VALARM\n\
        BEGIN:VALARM\nACTION:AUDIO\nTRIGGER;VALUE=DATE-TIME:20261019T070000Z\nEND:VALARM\n\
        BEGIN:VALARM\nJSID:mine\nACTION:DISPLAY\nDESCRIPTION:d\nTRIGGER:-P1D\nEND:VALARM\n\
        END:VEVENT\n";

    #[test]
    fn relative_absolute_and_actions() {
        let parsed = ical(SINGLE);
        let start = at("2026-10-19T08:00");
        assert_eq!(
            due(&parsed, "2026-10-18T00:00", "2026-10-20T00:00"),
            [
                ("mine".to_owned(), at("2026-10-18T08:00"), start, None),
                ("k4".to_owned(), at("2026-10-19T07:00"), start, None),
                ("k1".to_owned(), at("2026-10-19T07:45"), start, None),
                ("k2".to_owned(), at("2026-10-19T09:05"), start, None),
            ]
        );
        // The window is half open, and `max` cuts.
        assert_eq!(
            due(&parsed, "2026-10-19T07:00", "2026-10-19T07:45").len(),
            1
        );
        let one = alerts_between(&parsed, Zone::UTC, (0, i64::MAX), 1).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].alert_id, "mine");
    }

    /// The ids are the keys of `alerts` in the JSCalendar the web gets.
    #[test]
    fn ids_are_calcards() {
        let parsed = ical(SINGLE);
        let event = parsed
            .base_event(Zone::UTC, &EventOptions::default())
            .unwrap()
            .unwrap();
        let mut keys: Vec<String> = event.members["alerts"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(keys, ["k1", "k2", "k3", "k4", "mine"]);
        assert_eq!(event.members["alerts"]["k3"]["action"], "email");
    }

    const WEEKLY: &str = "BEGIN:VEVENT\nUID:w@example.com\nDTSTAMP:20261001T000000Z\n\
        DTSTART;TZID=Europe/Madrid:20261019T100000\nDTEND;TZID=Europe/Madrid:20261019T103000\n\
        RRULE:FREQ=WEEKLY;COUNT=4\nSUMMARY:Weekly\n\
        BEGIN:VALARM\nUID:alarm-1\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT15M\n\
        ACKNOWLEDGED:20261026T085000Z\nEND:VALARM\n\
        BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER;VALUE=DATE-TIME:20261026T085500Z\n\
        RELATED-TO;RELTYPE=PARENT:alarm-1\nEND:VALARM\n\
        END:VEVENT\n";

    #[test]
    fn series_acknowledged_and_snoozed() {
        let parsed = ical(WEEKLY);
        // 19 and 26 Oct were seen (acknowledged at 08:50 UTC on the 26th);
        // 2 and 9 Nov (after the change to CET) still ring. The snooze
        // rings once, for the occurrence its parent rang for.
        assert_eq!(
            due(&parsed, "2026-10-01T00:00", "2026-12-01T00:00"),
            [
                (
                    "k2".to_owned(),
                    at("2026-10-26T08:55"),
                    at("2026-10-26T09:00"),
                    Some(at("2026-10-26T10:00"))
                ),
                (
                    "k1".to_owned(),
                    at("2026-11-02T08:45"),
                    at("2026-11-02T09:00"),
                    Some(at("2026-11-02T10:00"))
                ),
                (
                    "k1".to_owned(),
                    at("2026-11-09T08:45"),
                    at("2026-11-09T09:00"),
                    Some(at("2026-11-09T10:00"))
                ),
            ]
        );
    }

    /// RFC 8984 §1.4.6: a day is a calendar day. The day before 10:00 on
    /// 25 Oct (CET, after the change at 03:00) is 10:00 on 24 Oct (CEST),
    /// 08:00 UTC, not 24 h before (09:00 UTC).
    #[test]
    fn days_are_nominal_across_dst() {
        let parsed = ical(
            "BEGIN:VEVENT\nUID:d@example.com\nDTSTAMP:20261001T000000Z\n\
             DTSTART;TZID=Europe/Madrid:20261025T100000\nDURATION:PT1H\n\
             BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-P1DT1H\nEND:VALARM\n\
             END:VEVENT\n",
        );
        assert_eq!(
            due(&parsed, "2026-10-01T00:00", "2026-11-01T00:00"),
            [(
                "k1".to_owned(),
                at("2026-10-24T07:00"),
                at("2026-10-25T09:00"),
                None
            )]
        );
    }

    /// What the web writes when it snoozes (apps/web/src/calendar/alerts.ts),
    /// through calcard's export: the snooze keeps its key and its parent.
    #[test]
    fn the_webs_snooze_survives_the_export() {
        let json = serde_json::json!({
            "@type": "Event", "uid": "web@example.com", "title": "Web",
            "start": "2026-10-19T10:00:00", "timeZone": "Europe/Madrid", "duration": "PT30M",
            "recurrenceRule": {"@type": "RecurrenceRule", "frequency": "daily", "count": 3},
            "alerts": {
                "a1": {"@type": "Alert", "action": "display",
                       "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M"},
                       "acknowledged": "2026-10-20T07:46:00Z"},
                "snzab12": {"@type": "Alert", "action": "display",
                            "relatedTo": {"a1": {"@type": "Relation", "relation": {"parent": true}}},
                            "trigger": {"@type": "AbsoluteTrigger", "when": "2026-10-20T07:56:00Z"}}
            }
        });
        let text = super::super::convert::jscalendar_to_icalendar(&json.to_string()).unwrap();
        let parsed = ParsedCalendar::parse(text.as_bytes()).unwrap();
        assert_eq!(
            due(&parsed, "2026-10-01T00:00", "2026-11-01T00:00"),
            [
                (
                    "snzab12".to_owned(),
                    at("2026-10-20T07:56"),
                    at("2026-10-20T08:00"),
                    Some(at("2026-10-20T10:00"))
                ),
                (
                    "a1".to_owned(),
                    at("2026-10-21T07:45"),
                    at("2026-10-21T08:00"),
                    Some(at("2026-10-21T10:00"))
                ),
            ]
        );
    }

    /// RFC 9074 snooze (the original names the snooze alarm), in a
    /// floating series read in Madrid.
    #[test]
    fn rfc_9074_snooze_and_floating() {
        let parsed = ical(
            "BEGIN:VEVENT\nUID:f@example.com\nDTSTAMP:20261001T000000Z\n\
             DTSTART:20261019T100000\nDTEND:20261019T110000\nRRULE:FREQ=DAILY;COUNT=3\n\
             BEGIN:VALARM\nUID:orig\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT10M\n\
             ACKNOWLEDGED:20261020T095000Z\nRELATED-TO;RELTYPE=SNOOZE:snz\nEND:VALARM\n\
             BEGIN:VALARM\nUID:snz\nACTION:DISPLAY\nDESCRIPTION:x\n\
             TRIGGER;VALUE=DATE-TIME:20261020T120000Z\nEND:VALARM\n\
             END:VEVENT\n",
        );
        let madrid = Zone::named("Europe/Madrid").unwrap();
        let got = alerts_between(&parsed, madrid, (0, i64::MAX), 10).unwrap();
        let summary: Vec<(&str, i64, i64)> = got
            .iter()
            .map(|a| (a.alert_id.as_str(), a.fire_at, a.occurrence))
            .collect();
        // Floating 10:00 read in Madrid (08:00 UTC); the snooze at 12:00
        // UTC belongs to the 20th (its parent rang at 07:50 that day).
        assert_eq!(
            summary,
            [
                ("k2", at("2026-10-20T12:00"), at("2026-10-20T08:00")),
                ("k1", at("2026-10-21T07:50"), at("2026-10-21T08:00")),
            ]
        );
    }

    const REPEATING: &str = "BEGIN:VEVENT\nUID:r@example.com\nDTSTAMP:20261001T000000Z\n\
        DTSTART:20261019T100000Z\nDURATION:PT1H\nSUMMARY:Repeat\n\
        BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:a\nTRIGGER:-PT15M\nREPEAT:3\nDURATION:PT5M\nEND:VALARM\n\
        BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:b\nTRIGGER:-PT1H\nREPEAT:50\nDURATION:PT10S\nEND:VALARM\n\
        BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:c\nTRIGGER:-PT2H\nREPEAT:2\nEND:VALARM\n\
        BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:d\nTRIGGER;VALUE=DATE-TIME:20261019T060000Z\n\
        REPEAT:1\nDURATION:PT1H\nEND:VALARM\n\
        END:VEVENT\n";

    fn rings(
        parsed: &ParsedCalendar,
        from: &str,
        to: &str,
        repeats: u32,
    ) -> Vec<(String, i64, u32)> {
        alerts_in(
            parsed,
            Zone::UTC,
            (at(from), at(to)),
            AlarmLimits::new(100, repeats),
        )
        .unwrap()
        .into_iter()
        .map(|a| (a.alert_id, a.fire_at, a.repetition))
        .collect()
    }

    /// RFC 5545 §3.6.6: `REPEAT` more rings `DURATION` apart. Too short a
    /// `DURATION` (10 s) or a `REPEAT` without one rings once.
    #[test]
    fn repeats() {
        let parsed = ical(REPEATING);
        let k = |id: &str, time: &str, n: u32| (id.to_owned(), at(time), n);
        assert_eq!(
            rings(&parsed, "2026-10-19T00:00", "2026-10-20T00:00", 10),
            [
                k("k4", "2026-10-19T06:00", 0),
                k("k4", "2026-10-19T07:00", 1),
                k("k3", "2026-10-19T08:00", 0),
                k("k2", "2026-10-19T09:00", 0),
                k("k1", "2026-10-19T09:45", 0),
                k("k1", "2026-10-19T09:50", 1),
                k("k1", "2026-10-19T09:55", 2),
                k("k1", "2026-10-19T10:00", 3),
            ]
        );
        // The limit, and 0: each alarm once.
        assert_eq!(
            rings(&parsed, "2026-10-19T09:00", "2026-10-20T00:00", 1),
            [
                k("k2", "2026-10-19T09:00", 0),
                k("k1", "2026-10-19T09:45", 0),
                k("k1", "2026-10-19T09:50", 1),
            ]
        );
        assert_eq!(
            rings(&parsed, "2026-10-19T09:00", "2026-10-20T00:00", 0).len(),
            2
        );
        // A window that starts between rings only has the later ones.
        assert_eq!(
            rings(&parsed, "2026-10-19T09:52", "2026-10-19T09:56", 10),
            [k("k1", "2026-10-19T09:55", 2)]
        );
        // The default of `alerts_between`.
        assert_eq!(
            due(&parsed, "2026-10-19T09:44", "2026-10-19T10:01").len(),
            4
        );
    }

    /// Dismissing (an `ACKNOWLEDGED` after the first ring) stops the
    /// repeats of that occurrence; the next occurrence rings again.
    #[test]
    fn acknowledged_stops_repeats() {
        let parsed = ical(
            "BEGIN:VEVENT\nUID:ra@example.com\nDTSTAMP:20261001T000000Z\n\
             DTSTART:20261019T100000Z\nDURATION:PT1H\nRRULE:FREQ=DAILY;COUNT=2\n\
             BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:a\nTRIGGER:-PT15M\nREPEAT:2\nDURATION:PT5M\n\
             ACKNOWLEDGED:20261019T094600Z\nEND:VALARM\nEND:VEVENT\n",
        );
        assert_eq!(
            rings(&parsed, "2026-10-19T00:00", "2026-10-21T00:00", 10),
            [
                ("k1".to_owned(), at("2026-10-20T09:45"), 0),
                ("k1".to_owned(), at("2026-10-20T09:50"), 1),
                ("k1".to_owned(), at("2026-10-20T09:55"), 2),
            ]
        );
    }

    /// `REPEAT` and `DURATION` are not JSCalendar: calcard keeps them under
    /// the alert's `iCalendar`, and they come back when the event is
    /// edited (the web changes the title, then dismisses the alert).
    #[test]
    fn repeat_survives_an_edit() {
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n{}END:VCALENDAR\r\n",
            REPEATING.replace('\n', "\r\n")
        );
        let mut edit = super::super::edit::EditableEvent::open(text.as_bytes()).unwrap();
        edit.event.insert("title".to_owned(), "Edited".into());
        edit.event["alerts"]["k1"]
            .as_object_mut()
            .unwrap()
            .insert("acknowledged".to_owned(), "2026-10-19T09:46:00Z".into());
        let written = edit.to_icalendar(at("2026-10-19T09:46")).unwrap();
        assert!(written.contains("REPEAT:3"), "{written}");
        assert!(written.contains("DURATION:PT5M"), "{written}");
        let parsed = ParsedCalendar::parse(written.as_bytes()).unwrap();
        // k1 was dismissed: no more of its repeats; the others still ring.
        let left: Vec<(String, u32)> = rings(&parsed, "2026-10-19T00:00", "2026-10-20T00:00", 10)
            .into_iter()
            .map(|(id, _, n)| (id, n))
            .collect();
        assert_eq!(
            left,
            [
                ("k4".to_owned(), 0),
                ("k4".to_owned(), 1),
                ("k3".to_owned(), 0),
                ("k2".to_owned(), 0)
            ]
        );
    }

    fn valarms(count: usize, trigger: &str) -> String {
        "BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:{trigger}\nEND:VALARM\n"
            .replace("{trigger}", trigger)
            .repeat(count)
    }

    /// Only the first [`MAX_ALARMS_PER_COMPONENT`] alarms of a component.
    #[test]
    fn alarms_per_component_are_capped() {
        let parsed = ical(&format!(
            "BEGIN:VEVENT\nUID:alarm-test\nDTSTAMP:20261001T000000Z\n\
             DTSTART:20261019T100000Z\nDURATION:PT1H\n{}END:VEVENT\n",
            valarms(100, "-PT15M")
        ));
        let component = parsed
            .icalendar()
            .components
            .iter()
            .find(|c| c.component_type == ICalendarComponentType::VEvent)
            .unwrap();
        let found = alarms(&parsed, component);
        assert_eq!(found.len(), MAX_ALARMS_PER_COMPONENT);
        assert_eq!(found[0].id, "k1");
        assert_eq!(found[63].id, "k64");
    }

    /// A long series far from the window costs nothing, however many
    /// alarms it has, and gives nothing.
    #[test]
    fn far_occurrences_are_not_worked_out() {
        let parsed = ical(&format!(
            "BEGIN:VEVENT\nUID:alarm-test\nDTSTAMP:20261001T000000Z\n\
             DTSTART:19900101T100000Z\nDURATION:PT1H\nRRULE:FREQ=DAILY;COUNT=10000\n\
             {}END:VEVENT\n",
            valarms(64, "P36500D")
        ));
        let started = std::time::Instant::now();
        let (due, used) = alerts_counted(
            &parsed,
            Zone::UTC,
            (at("2026-10-19T00:00"), at("2026-10-20T00:00")),
            AlarmLimits::new(64, DEFAULT_MAX_REPEATS),
        )
        .unwrap();
        assert!(due.is_empty());
        assert_eq!(used, 0);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// Many occurrences near the window with many alarms: the work stops
    /// at the limit.
    #[test]
    fn work_is_capped_per_object() {
        let parsed = ical(&format!(
            "BEGIN:VEVENT\nUID:alarm-test\nDTSTAMP:20261001T000000Z\n\
             DTSTART:20261019T100000Z\nDURATION:PT1M\nRRULE:FREQ=MINUTELY;COUNT=5000\n\
             {}END:VEVENT\n",
            valarms(64, "-PT200H")
        ));
        let (_, used) = alerts_counted(
            &parsed,
            Zone::UTC,
            (at("2026-10-19T00:00"), at("2026-10-20T00:00")),
            AlarmLimits::new(64, DEFAULT_MAX_REPEATS),
        )
        .unwrap();
        assert_eq!(used, MAX_ALARM_EVALUATIONS);
    }

    /// A daily alarm still rings with the window filter.
    #[test]
    fn daily_series_still_rings() {
        let parsed = ical(
            "BEGIN:VEVENT\nUID:alarm-test\nDTSTAMP:20201001T000000Z\n\
             DTSTART:20201019T100000Z\nDURATION:PT30M\nRRULE:FREQ=DAILY\n\
             BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT15M\nEND:VALARM\n\
             END:VEVENT\n",
        );
        assert_eq!(
            due(&parsed, "2026-10-19T09:30", "2026-10-19T10:30"),
            [(
                "k1".to_owned(),
                at("2026-10-19T09:45"),
                at("2026-10-19T10:00"),
                Some(at("2026-10-19T10:00"))
            )]
        );
    }

    /// A snooze (absolute) in a monthly series whose next occurrence is 20
    /// days away: no occurrence is near the window, and it still rings.
    #[test]
    fn snooze_far_from_the_next_occurrence() {
        let parsed = ical(
            "BEGIN:VEVENT\nUID:alarm-test\nDTSTAMP:20261001T000000Z\n\
             DTSTART:20261001T100000Z\nDURATION:PT1H\nRRULE:FREQ=MONTHLY\n\
             BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT15M\nEND:VALARM\n\
             BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\n\
             TRIGGER;VALUE=DATE-TIME:20261112T120000Z\nEND:VALARM\n\
             END:VEVENT\n",
        );
        let limits = AlarmLimits {
            lead: 86_400,
            lag: 86_400,
            ..AlarmLimits::new(64, 0)
        };
        let got = alerts_in(
            &parsed,
            Zone::UTC,
            (at("2026-11-12T00:00"), at("2026-11-13T00:00")),
            limits,
        )
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].alert_id, "k2");
        assert_eq!(got[0].fire_at, at("2026-11-12T12:00"));
        assert_eq!(got[0].occurrence, at("2026-12-01T10:00"));
    }
}
