// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What the JMAP calendar methods need from an iCalendar object
//! with calcard and the rules of Stalwart 0.16's
//! `CalendarEvent/query` and `/get` (the server the web was written
//! against):
//!
//! - **Occurrences** ([`ParsedCalendar::instances`]): every `VEVENT`
//!   instance (RRULE, RDATE, EXDATE, overrides), floating times read in the
//!   query's zone. Each has a [`RecurrenceKey`]: its `RECURRENCE-ID` (an
//!   override) or its start (any other instance), as a wall-clock reading
//!   in the event's own zone. The key does not change when the occurrence
//!   is moved, so the JMAP id built from it is stable.
//! - **One occurrence as an event** ([`ParsedCalendar::occurrence`]):
//!   Stalwart's way: the component alone, without its recurrence rules,
//!   with `DTSTART`/`DTEND` at the occurrence in the event's zone (the
//!   query's zone when the event is in UTC or floating) and a
//!   `RECURRENCE-ID`.
//! - **The stored event** ([`ParsedCalendar::base_event`]): the main
//!   `VEVENT` with its overrides in `recurrenceOverrides`.
//! - **`CalendarEvent/parse`** ([`parse_events`]): every event of a blob.
//! - **Tasks** (`VTODO`, JSCalendar `Task`): the
//!   same paths as events. A task is placed at its `DUE` (or its
//!   `DTSTART` if it has no `DUE`), a point in time
//!   ([`Instance::overlaps`]); one with neither is not placed at all
//!   (calcard would fall back to `CREATED`/`COMPLETED`), so it only comes
//!   in queries without dates. One occurrence of a task keeps `DUE` (not
//!   `DTEND`) and an all-day `DUE` or `DTSTART` stays a date.
//!
//! Limits: at most [`MAX_OCCURRENCES`] instances per object and calcard's
//! own budget of unproductive iterations, so a rule that never matches
//! still ends. Everything runs inside `catch_unwind` (see
//! [`super::convert`]); CPU work: call it off the async threads.

use std::cell::RefCell;
use std::collections::HashSet;
use std::str::FromStr;

use calcard::common::PartialDateTime;
use calcard::common::timezone::Tz;
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarParameter,
    ICalendarParameterName, ICalendarParameterValue, ICalendarParticipationRole, ICalendarProperty,
    ICalendarStatus, ICalendarValue, ICalendarValueType,
};
use calcard::jscalendar::import::ImportOptions;
use serde_json::{Map, Value};

use super::convert::{Format, MAX_OCCURRENCES, checked_text, guarded};
use crate::ConvertError;

type CalendarObject<'x> = calcard::jscalendar::JSCalendar<'x, String, String>;

/// A time zone calcard knows (an IANA name, or a name calcard maps to
/// one). Never floating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone(Tz);

impl Zone {
    /// UTC.
    pub const UTC: Self = Self(Tz::UTC);

    /// The zone with this name, if calcard knows it.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        guarded(Format::ICalendar, || {
            Ok(Tz::from_str(name).ok().filter(|tz| !tz.is_floating()))
        })
        .ok()
        .flatten()
        .map(Self)
    }

    /// The instant of a wall-clock reading in this zone (`naive`: seconds
    /// since the epoch as if it were UTC). In a gap (spring forward) the
    /// offset before the change applies, as calcard does everywhere.
    #[must_use]
    pub fn to_utc(self, naive: i64) -> Option<i64> {
        guarded(Format::ICalendar, || {
            Ok(PartialDateTime::from_naive_timestamp(naive)
                .to_date_time()
                .and_then(|dt| dt.to_date_time_with_tz(self.0))
                .map(|zoned| zoned.timestamp()))
        })
        .ok()
        .flatten()
    }

    /// calcard's zone (for the alerts).
    pub(super) const fn tz(self) -> Tz {
        self.0
    }

    /// The zone's IANA name (`Etc/UTC` for UTC).
    #[must_use]
    pub fn iana_name(self) -> String {
        self.0
            .name()
            .map_or_else(|| "Etc/UTC".to_owned(), std::borrow::Cow::into_owned)
    }
}

/// Which occurrence of an event: its original start as a wall-clock
/// reading in the event's zone, in seconds since the epoch as if it were
/// UTC (Stalwart's "recurrence key", to the second).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecurrenceKey(i64);

impl RecurrenceKey {
    /// The first key: 0001-01-01T00:00:00.
    pub const MIN: Self = Self(-62_135_596_800);
    /// The last key: 9999-12-31T23:59:59. Keys outside the years 1 to 9999
    /// can't be written in an id (`YYYYMMDDTHHMMSS`), so they don't exist.
    pub const MAX: Self = Self(253_402_300_799);

    /// A key from its wall-clock seconds, if they are within
    /// [`Self::MIN`]..=[`Self::MAX`].
    #[must_use]
    pub const fn from_naive(seconds: i64) -> Option<Self> {
        if seconds < Self::MIN.0 || seconds > Self::MAX.0 {
            return None;
        }
        Some(Self(seconds))
    }

    /// The wall-clock seconds.
    #[must_use]
    pub const fn naive(self) -> i64 {
        self.0
    }
}

/// One occurrence of an event or a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instance {
    /// Which occurrence.
    pub key: RecurrenceKey,
    /// Start, seconds since the epoch (UTC). A task: its `DTSTART`, or its
    /// `DUE` if it has no start.
    pub utc_start: i64,
    /// End, seconds since the epoch (UTC). A task: its `DUE` (else its
    /// start plus `DURATION`, or its start).
    pub utc_end: i64,
    /// calcard's index of the component it comes from.
    component: u32,
    /// A task: where it is placed (its `DUE`, else its start).
    task_at: Option<i64>,
}

impl Instance {
    /// calcard's index of the component it comes from (for the alerts).
    pub(super) const fn component(&self) -> u32 {
        self.component
    }

    /// Whether it is an occurrence of a task (`VTODO`).
    #[must_use]
    pub const fn is_task(&self) -> bool {
        self.task_at.is_some()
    }

    /// Where it sorts: the start of an event, the `DUE` (or start) of a
    /// task.
    #[must_use]
    pub fn placed_at(&self) -> i64 {
        self.task_at.unwrap_or(self.utc_start)
    }

    /// Whether it falls in `[after, before)` (UTC seconds): an event that
    /// starts before `before` and ends after `after` (draft-ietf-jmap-
    /// calendars, as Stalwart); a task whose `DUE` (or start) is in the
    /// range, as a point in time (a task due at midnight is on the day
    /// that starts then, not on the one before).
    #[must_use]
    pub fn overlaps(&self, after: i64, before: i64) -> bool {
        match self.task_at {
            Some(at) => (after..before).contains(&at),
            None => self.utc_start < before && self.utc_end > after,
        }
    }
}

/// The instances of an object, sorted by start. `complete` is false when a
/// limit was reached or a component could not be expanded: what was found
/// is still there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Instances {
    /// The occurrences.
    pub list: Vec<Instance>,
    /// Whether every occurrence is in `list`.
    pub complete: bool,
}

/// What `CalendarEvent/get` asks for, beyond the event.
#[derive(Debug, Clone, Default)]
pub struct EventOptions {
    /// Keep the iCalendar properties calcard can't convert, under
    /// `iCalendar` (Stalwart: only when `properties` is given).
    pub keep_unconverted: bool,
    /// `reduceParticipants`: only attendees that are the user (these
    /// addresses, without `mailto:`) or have the `owner` role.
    pub only_participants: Option<Vec<String>>,
    /// `recurrenceOverridesAfter`/`Before`: overrides whose original start
    /// (UTC seconds) is outside `[after, before)` are left out.
    pub overrides_between: Option<(i64, i64)>,
}

/// An event as JSCalendar, with what the JMAP layer adds.
#[derive(Debug, Clone, PartialEq)]
pub struct EventJson {
    /// The JSCalendar `Event`.
    pub members: Map<String, Value>,
    /// Start and end of the occurrence (or of the first one of a series).
    pub utc_range: Option<(i64, i64)>,
}

/// A parsed iCalendar object, for several reads. The occurrences are
/// expanded once per zone.
#[derive(Debug, Clone)]
pub struct ParsedCalendar(ICalendar, RefCell<Vec<(Zone, Instances)>>);

/// What a calendar object holds, by its main component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ItemKind {
    /// `VEVENT`: a JSCalendar `Event`.
    Event,
    /// `VTODO`: a JSCalendar `Task`.
    Task,
}

/// Whether this component is an event or a task (journals are not
/// shown).
fn is_item(component: &ICalendarComponent) -> bool {
    matches!(
        component.component_type,
        ICalendarComponentType::VEvent | ICalendarComponentType::VTodo
    )
}

fn is_task(component: &ICalendarComponent) -> bool {
    component.component_type == ICalendarComponentType::VTodo
}

fn has_date(component: &ICalendarComponent, name: &ICalendarProperty) -> bool {
    component
        .property(name)
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_partial_date_time)
        .is_some_and(|date| date.to_date_time().is_some())
}

/// Whether the component has a place in time: any event; a task with a
/// `DTSTART`, a `DUE` or a `RECURRENCE-ID`. calcard would also place a
/// task without them at its `CREATED` or `COMPLETED`; here such a task has
/// no date.
pub(super) fn is_placed(component: &ICalendarComponent) -> bool {
    match component.component_type {
        ICalendarComponentType::VEvent => true,
        ICalendarComponentType::VTodo => [
            ICalendarProperty::Dtstart,
            ICalendarProperty::Due,
            ICalendarProperty::RecurrenceId,
        ]
        .iter()
        .any(|name| has_date(component, name)),
        _ => false,
    }
}

/// Whether a task is done or cancelled (`STATUS`): its alerts don't ring.
pub(super) fn is_finished_task(component: &ICalendarComponent) -> bool {
    is_task(component)
        && component
            .property(&ICalendarProperty::Status)
            .and_then(|entry| entry.values.first())
            .is_some_and(|value| {
                matches!(
                    value,
                    ICalendarValue::Status(ICalendarStatus::Completed | ICalendarStatus::Cancelled)
                )
            })
}

/// The component's own time zone: the `TZID` of a date it has (Stalwart's
/// rule), never UTC.
pub(super) fn component_zone(component: &ICalendarComponent) -> Option<Tz> {
    component
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.name,
                ICalendarProperty::Dtstart
                    | ICalendarProperty::Dtend
                    | ICalendarProperty::Due
                    | ICalendarProperty::Exdate
                    | ICalendarProperty::Exrule
                    | ICalendarProperty::Rdate
                    | ICalendarProperty::Rrule
                    | ICalendarProperty::RecurrenceId
            )
        })
        .filter_map(|entry| entry.tz_id().and_then(|id| Tz::from_str(id).ok()))
        .rfind(|tz| *tz != Tz::UTC && !tz.is_floating())
}

/// The `RECURRENCE-ID` of an override as a wall-clock reading in `zone`
/// (the zone of the instance), as Stalwart computes it.
fn recurrence_id_naive(component: &ICalendarComponent, zone: Tz) -> Option<i64> {
    rid_naive(component.property(&ICalendarProperty::RecurrenceId)?, zone)
}

/// A `RECURRENCE-ID` (or another date-time entry) as a wall-clock reading
/// in its `TZID`, else in `zone` (a UTC value is converted to `zone`; a
/// floating one is read there).
pub(super) fn rid_naive(entry: &ICalendarEntry, zone: Tz) -> Option<i64> {
    let tz = entry
        .tz_id()
        .and_then(|id| Tz::from_str(id).ok())
        .unwrap_or(zone);
    let date_time = entry
        .values
        .first()?
        .as_partial_date_time()?
        .to_date_time()?;
    Some(
        date_time
            .to_date_time_with_tz(tz)?
            .with_timezone(tz)
            .naive_timestamp(),
    )
}

fn date_time_entry(name: ICalendarProperty, zone: &str, naive: i64) -> ICalendarEntry {
    ICalendarEntry::new(name)
        .with_param(ICalendarParameter::tzid(zone.to_owned()))
        .with_value(PartialDateTime::from_naive_timestamp(naive))
}

/// `name` of a task at an occurrence (wall-clock `naive` in `zone`): a
/// date if the task's `like` is a date (an all-day task stays one), else
/// a date-time in `zone`.
fn task_date_entry(
    source: &ICalendarComponent,
    like: &ICalendarProperty,
    name: ICalendarProperty,
    zone: &str,
    naive: i64,
) -> ICalendarEntry {
    let is_date = source
        .property(like)
        .and_then(|entry| entry.values.first())
        .and_then(ICalendarValue::as_partial_date_time)
        .is_some_and(|date| !date.has_time());
    if !is_date {
        return date_time_entry(name, zone, naive);
    }
    let full = PartialDateTime::from_naive_timestamp(naive);
    ICalendarEntry::new(name)
        .with_param(ICalendarParameter::value(ICalendarValueType::Date))
        .with_value(PartialDateTime {
            year: full.year,
            month: full.month,
            day: full.day,
            ..PartialDateTime::default()
        })
}

/// The task's date its occurrences are keyed by: `DTSTART`, else `DUE`
/// (calcard's start of a task without one).
fn task_key_date(source: &ICalendarComponent) -> ICalendarProperty {
    if has_date(source, &ICalendarProperty::Dtstart) || !has_date(source, &ICalendarProperty::Due) {
        ICalendarProperty::Dtstart
    } else {
        ICalendarProperty::Due
    }
}

/// The dates of one occurrence of a task: `DTSTART` if the task has a
/// start (or has neither: an override placed by its `RECURRENCE-ID`),
/// `DUE` if it has one (calcard's instance ends at the `DUE`).
fn task_entries(
    source: &ICalendarComponent,
    zone: &str,
    (start, end): (i64, i64),
) -> Vec<ICalendarEntry> {
    let start_like = task_key_date(source);
    let mut out = Vec::new();
    if start_like == ICalendarProperty::Dtstart {
        out.push(task_date_entry(
            source,
            &start_like,
            ICalendarProperty::Dtstart,
            zone,
            start,
        ));
    }
    if has_date(source, &ICalendarProperty::Due) {
        out.push(task_date_entry(
            source,
            &ICalendarProperty::Due,
            ICalendarProperty::Due,
            zone,
            end,
        ));
    }
    out
}

/// The properties of one occurrence of `source` (Stalwart's rules): no
/// recurrence properties, `DTSTART`/`DTEND` at the occurrence (`start`,
/// `end`: wall-clock seconds in `zone`) and a `RECURRENCE-ID`.
fn occurrence_entries(
    source: &ICalendarComponent,
    zone: &str,
    (start, end): (i64, i64),
    first_of_component: Option<i64>,
) -> Vec<ICalendarEntry> {
    let is_recurrent = source.is_recurrent();
    let is_recurrent_or_override = is_recurrent || source.is_recurrence_override();
    let task = is_task(source);
    let mut has_duration = false;
    let mut entries: Vec<ICalendarEntry> = source
        .entries
        .iter()
        .filter(|entry| match &entry.name {
            ICalendarProperty::Dtstart
            | ICalendarProperty::Dtend
            | ICalendarProperty::Exdate
            | ICalendarProperty::Exrule
            | ICalendarProperty::Rdate
            | ICalendarProperty::Rrule
            | ICalendarProperty::RecurrenceId => false,
            // A task's own dates (its DUE is set below, at the occurrence).
            ICalendarProperty::Due if task => false,
            ICalendarProperty::Completed | ICalendarProperty::Created if task => true,
            ICalendarProperty::Due | ICalendarProperty::Completed | ICalendarProperty::Created => {
                is_recurrent
            }
            ICalendarProperty::Duration => {
                has_duration = true;
                true
            }
            _ => true,
        })
        .cloned()
        .collect();
    if task {
        entries.extend(task_entries(source, zone, (start, end)));
    } else {
        entries.push(date_time_entry(ICalendarProperty::Dtstart, zone, start));
    }
    if is_recurrent_or_override {
        // The override's own RECURRENCE-ID, unless it is a range whose later
        // instances need their own.
        let own = source
            .property(&ICalendarProperty::RecurrenceId)
            .filter(|entry| {
                entry
                    .parameters(&ICalendarParameterName::Range)
                    .next()
                    .is_none()
                    || first_of_component == Some(start)
            })
            .cloned();
        entries.push(own.unwrap_or_else(|| {
            if task {
                task_date_entry(
                    source,
                    &task_key_date(source),
                    ICalendarProperty::RecurrenceId,
                    zone,
                    start,
                )
            } else {
                date_time_entry(ICalendarProperty::RecurrenceId, zone, start)
            }
        }));
    }
    if !has_duration && !task {
        entries.push(date_time_entry(ICalendarProperty::Dtend, zone, end));
    }
    entries
}

/// The wall-clock reading of an instant in `zone`.
pub(super) fn naive_in(zone: Tz, utc: i64) -> Option<i64> {
    let date_time = PartialDateTime::from_naive_timestamp(utc).to_date_time()?;
    Some(
        date_time
            .to_date_time_with_tz(Tz::UTC)?
            .with_timezone(zone)
            .naive_timestamp(),
    )
}

fn to_members(ical: ICalendar, keep_unconverted: bool) -> Result<Map<String, Value>, ConvertError> {
    let format = Format::ICalendar;
    let options = ImportOptions::new()
        .include_ical_components(keep_unconverted)
        .return_first(true);
    let js: CalendarObject<'static> = ical
        .into_jscalendar_with(options)
        .map_err(|_| ConvertError::Invalid { format })?;
    match serde_json::to_value(&js.0) {
        Ok(Value::Object(members)) => Ok(members),
        _ => Err(ConvertError::Invalid { format }),
    }
}

/// `reduceParticipants`: keeps the organizer's, the user's and `owner`
/// attendees.
fn reduce_participants(ical: &mut ICalendar, own: &[String]) {
    for component in &mut ical.components {
        if !component.component_type.is_scheduling_object() {
            continue;
        }
        component.entries.retain(|entry| {
            if entry.name != ICalendarProperty::Attendee {
                return true;
            }
            let owner = entry.parameters(&ICalendarParameterName::Role).any(|role| {
                matches!(
                    role,
                    ICalendarParameterValue::Role(ICalendarParticipationRole::Owner)
                )
            });
            owner
                || entry
                    .calendar_address()
                    .is_some_and(|address| own.iter().any(|a| a.eq_ignore_ascii_case(address)))
        });
    }
}

impl ParsedCalendar {
    /// Parses an iCalendar object (at most 1 MiB, UTF-8).
    ///
    /// # Errors
    /// [`ConvertError`] for anything that is not one `VCALENDAR`.
    pub fn parse(input: &[u8]) -> Result<Self, ConvertError> {
        let format = Format::ICalendar;
        let text = checked_text(input, format)?;
        guarded(format, || {
            ICalendar::parse(text)
                .map(|ical| Self(ical, RefCell::new(Vec::new())))
                .map_err(|_| ConvertError::Invalid { format })
        })
    }

    /// The parsed object (for the alerts, [`super::alarm`]).
    pub(super) const fn icalendar(&self) -> &ICalendar {
        &self.0
    }

    /// Whether it has at least one event or task (`VEVENT`, `VTODO`).
    #[must_use]
    pub fn has_events(&self) -> bool {
        self.0.components.iter().any(is_item)
    }

    /// Whether the event or task repeats (`RRULE`, `RDATE` or overrides).
    #[must_use]
    pub fn is_recurring(&self) -> bool {
        self.0
            .components
            .iter()
            .any(|c| is_item(c) && c.is_recurrent_or_override())
    }

    /// Whether `CalendarEvent/query` expands it into occurrences with their
    /// own ids. As Stalwart 1.0: anything but one component without
    /// `RRULE` or `RDATE`, which is simply the event (also when it is a
    /// lone override).
    #[must_use]
    pub fn is_series(&self) -> bool {
        let mut items = self
            .0
            .components
            .iter()
            .filter(|c| c.component_type.is_scheduling_object());
        match (items.next(), items.next()) {
            (Some(only), None) => only.is_recurrent(),
            _ => true,
        }
    }

    /// Whether it is an event or a task (by its main component).
    #[must_use]
    pub fn kind(&self) -> Option<ItemKind> {
        self.main_event().map(|(_, c)| {
            if is_task(c) {
                ItemKind::Task
            } else {
                ItemKind::Event
            }
        })
    }

    /// The main event or task: the first `VEVENT`/`VTODO` that is not an
    /// override (or the first one if all are).
    fn main_event(&self) -> Option<(u32, &ICalendarComponent)> {
        let events = || {
            self.0
                .components
                .iter()
                .enumerate()
                .filter(|(_, c)| is_item(c))
        };
        events()
            .find(|(_, c)| !c.is_recurrence_override())
            .or_else(|| events().next())
            .and_then(|(index, c)| u32::try_from(index).ok().map(|index| (index, c)))
    }

    /// The organizer's address (without `mailto:`), if the event has one.
    #[must_use]
    pub fn organizer(&self) -> Option<String> {
        self.main_event()
            .and_then(|(_, c)| c.property(&ICalendarProperty::Organizer))
            .and_then(ICalendarEntry::calendar_address)
            .map(str::to_owned)
    }

    /// Every event occurrence, floating times read in `zone`, sorted by
    /// start. Two instances with the same key keep the first.
    ///
    /// # Errors
    /// [`ConvertError::Panicked`] if calcard fails.
    pub fn instances(&self, zone: Zone) -> Result<Instances, ConvertError> {
        if let Some((_, known)) = self.1.borrow().iter().find(|(z, _)| *z == zone) {
            return Ok(known.clone());
        }
        let instances = self.expand(zone)?;
        self.1.borrow_mut().push((zone, instances.clone()));
        Ok(instances)
    }

    fn expand(&self, zone: Zone) -> Result<Instances, ConvertError> {
        guarded(Format::ICalendar, || {
            let expanded = self.0.expand_dates(zone.0, MAX_OCCURRENCES);
            let mut list: Vec<Instance> = Vec::with_capacity(expanded.events.len());
            let mut seen_overrides: Vec<u32> = Vec::new();
            let mut keys: HashSet<RecurrenceKey> = HashSet::new();
            let mut out_of_range = false;
            let mut events = expanded.events;
            events.sort_by_key(|event| (event.start.timestamp(), event.comp_id));
            for event in &events {
                let Some(component) = usize::try_from(event.comp_id)
                    .ok()
                    .and_then(|index| self.0.components.get(index))
                    .filter(|c| is_item(c) && is_placed(c))
                else {
                    continue;
                };
                // An override keeps its RECURRENCE-ID as key on its first
                // instance (later ones of a THISANDFUTURE override are keyed
                // by their own start, as in Stalwart).
                let own = (component.is_recurrence_override()
                    && !seen_overrides.contains(&event.comp_id))
                .then(|| recurrence_id_naive(component, event.start.timezone()))
                .flatten();
                if component.is_recurrence_override() {
                    seen_overrides.push(event.comp_id);
                }
                // A key outside the years 1 to 9999 has no id: that
                // occurrence is left out (the rest of the event shows).
                let Some(key) =
                    RecurrenceKey::from_naive(own.unwrap_or_else(|| event.start.naive_timestamp()))
                else {
                    tracing::debug!("calendar: an occurrence outside years 1-9999 left out");
                    out_of_range = true;
                    continue;
                };
                if !keys.insert(key) {
                    continue;
                }
                let (utc_start, utc_end) = (event.start.timestamp(), event.end.timestamp());
                let task_at = is_task(component).then(|| {
                    if has_date(component, &ICalendarProperty::Due) {
                        utc_end
                    } else {
                        utc_start
                    }
                });
                list.push(Instance {
                    key,
                    utc_start,
                    utc_end,
                    component: event.comp_id,
                    task_at,
                });
            }
            let complete = expanded.errors.is_empty() && !expanded.truncated && !out_of_range;
            Ok(Instances { list, complete })
        })
    }

    /// The stored event as JSCalendar (the main `VEVENT`, its overrides in
    /// `recurrenceOverrides`), with the UTC range of its first own
    /// occurrence. `None` if it has no event.
    ///
    /// # Errors
    /// [`ConvertError`] if calcard fails.
    pub fn base_event(
        &self,
        zone: Zone,
        options: &EventOptions,
    ) -> Result<Option<EventJson>, ConvertError> {
        let Some((main, _)) = self.main_event() else {
            return Ok(None);
        };
        let first = self
            .instances(zone)?
            .list
            .into_iter()
            .filter(|instance| instance.component == main)
            .min_by_key(|instance| instance.utc_start)
            .map(|instance| (instance.utc_start, instance.utc_end));
        let mut ical = self.0.clone();
        guarded(Format::ICalendar, move || {
            if let Some((after, before)) = options.overrides_between {
                let outside: Vec<u32> = ical
                    .components
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.is_recurrence_override())
                    .filter(|(_, c)| {
                        c.property(&ICalendarProperty::RecurrenceId)
                            .and_then(|entry| entry.values.first())
                            .and_then(ICalendarValue::as_partial_date_time)
                            .and_then(PartialDateTime::to_date_time)
                            .and_then(|dt| dt.to_date_time_with_tz(zone.0))
                            .is_some_and(|dt| !(after..before).contains(&dt.timestamp()))
                    })
                    .filter_map(|(index, _)| u32::try_from(index).ok())
                    .collect();
                if !outside.is_empty() {
                    for component in &mut ical.components {
                        component.component_ids.retain(|id| !outside.contains(id));
                    }
                }
            }
            if let Some(own) = &options.only_participants {
                reduce_participants(&mut ical, own);
            }
            let members = to_members(ical, options.keep_unconverted)?;
            Ok(Some(EventJson {
                members,
                utc_range: first,
            }))
        })
    }

    /// One occurrence as its own JSCalendar event (Stalwart's rules, see
    /// the module docs). `None` if the event has no such occurrence.
    ///
    /// # Errors
    /// [`ConvertError`] if calcard fails.
    pub fn occurrence(
        &self,
        key: RecurrenceKey,
        zone: Zone,
        options: &EventOptions,
    ) -> Result<Option<EventJson>, ConvertError> {
        let instances = self.instances(zone)?;
        let Some(instance) = instances.list.into_iter().find(|i| i.key == key) else {
            return Ok(None);
        };
        let first_of_component = self.first_naive_start(instance.component, zone)?;
        guarded(Format::ICalendar, || {
            let Some(source) = usize::try_from(instance.component)
                .ok()
                .and_then(|index| self.0.components.get(index))
            else {
                return Ok(None);
            };
            let tz = component_zone(source).unwrap_or(zone.0);
            let (Some(start), Some(end)) = (
                naive_in(tz, instance.utc_start),
                naive_in(tz, instance.utc_end),
            ) else {
                return Ok(None);
            };
            let entries = occurrence_entries(
                source,
                &Zone(tz).iana_name(),
                (start, end),
                first_of_component,
            );
            let mut components = vec![
                ICalendarComponent {
                    component_type: ICalendarComponentType::VCalendar,
                    entries: Vec::new(),
                    component_ids: vec![1],
                },
                ICalendarComponent {
                    component_type: source.component_type.clone(),
                    entries,
                    component_ids: Vec::new(),
                },
            ];
            let mut children = Vec::new();
            for child in &source.component_ids {
                let Some(sub) = usize::try_from(*child)
                    .ok()
                    .and_then(|index| self.0.components.get(index))
                else {
                    continue;
                };
                let mut sub = sub.clone();
                sub.component_ids.clear();
                children.push(u32::try_from(components.len()).unwrap_or(u32::MAX));
                components.push(sub);
            }
            if let Some(event) = components.get_mut(1) {
                event.component_ids = children;
            }
            let mut ical = ICalendar { components };
            if let Some(own) = &options.only_participants {
                reduce_participants(&mut ical, own);
            }
            let members = to_members(ical, options.keep_unconverted)?;
            Ok(Some(EventJson {
                members,
                utc_range: Some((instance.utc_start, instance.utc_end)),
            }))
        })
    }

    /// The wall-clock start, in the component's zone, of its first
    /// instance.
    fn first_naive_start(&self, component: u32, zone: Zone) -> Result<Option<i64>, ConvertError> {
        let source = usize::try_from(component)
            .ok()
            .and_then(|index| self.0.components.get(index));
        let tz = source.and_then(component_zone).unwrap_or(zone.0);
        Ok(self
            .instances(zone)?
            .list
            .iter()
            .filter(|i| i.component == component)
            .min_by_key(|i| i.utc_start)
            .and_then(|i| naive_in(tz, i.utc_start)))
    }
}

/// `CalendarEvent/parse`: every event of an iCalendar blob (a series with
/// its overrides inside, other events after), as Stalwart answers, with
/// the unconverted properties under `iCalendar`.
///
/// # Errors
/// [`ConvertError`] for anything that is not one `VCALENDAR`.
pub fn parse_events(input: &[u8]) -> Result<Vec<Map<String, Value>>, ConvertError> {
    let format = Format::ICalendar;
    let text = checked_text(input, format)?;
    guarded(format, || {
        let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        let js: CalendarObject<'static> = ical
            .into_jscalendar_with(ImportOptions::new())
            .map_err(|_| ConvertError::Invalid { format })?;
        let Ok(Value::Object(mut group)) = serde_json::to_value(&js.0) else {
            return Err(ConvertError::Invalid { format });
        };
        let entries = match group.remove("entries") {
            Some(Value::Array(entries)) => entries,
            _ => Vec::new(),
        };
        Ok(entries
            .into_iter()
            .filter_map(|entry| match entry {
                Value::Object(members) => Some(members),
                _ => None,
            })
            .collect())
    })
}

/// The `mailto:` addresses of a `calendar-user-address-set`, each once
/// (case-insensitively), written `mailto:` + address. Other URIs (the
/// principal's URL, `urn:uuid:`…) are not addresses to schedule with.
#[must_use]
pub fn participant_addresses<'a>(set: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for uri in set {
        let Some((scheme, address)) = uri.trim().split_once(':') else {
            continue;
        };
        let address = address.trim();
        if !scheme.eq_ignore_ascii_case("mailto")
            || address.is_empty()
            || address.len() > 320
            || address.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            continue;
        }
        let uri = format!("mailto:{address}");
        if !out.iter().any(|known| known.eq_ignore_ascii_case(&uri)) {
            out.push(uri);
        }
    }
    out
}

/// The number of a participant identity: a stable hash of its address
/// (FNV-1a, 63 bits, case-insensitive), so the same address always has
/// the same id and the change log can name it.
#[must_use]
pub fn participant_number(address: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in address.bytes().map(|b| b.to_ascii_lowercase()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let positive = hash & 0x7fff_ffff_ffff_ffff;
    i64::try_from(positive).unwrap_or(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEKLY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n\
        BEGIN:VEVENT\r\nUID:w@example.com\r\nDTSTAMP:20261001T000000Z\r\n\
        DTSTART;TZID=Europe/Madrid:20261019T100000\r\nDTEND;TZID=Europe/Madrid:20261019T103000\r\n\
        RRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Madrid:20261026T100000\r\n\
        SUMMARY:Weekly\r\nEND:VEVENT\r\n\
        BEGIN:VEVENT\r\nUID:w@example.com\r\nDTSTAMP:20261001T000000Z\r\n\
        RECURRENCE-ID;TZID=Europe/Madrid:20261102T100000\r\n\
        DTSTART;TZID=Europe/Madrid:20261102T120000\r\nDTEND;TZID=Europe/Madrid:20261102T123000\r\n\
        SUMMARY:Moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    /// `YYYY-MM-DDTHH:MM` as seconds since the epoch, as if UTC.
    fn naive(text: &str) -> i64 {
        let numbers: Vec<i64> = text
            .split(['-', 'T', ':'])
            .map(|part| part.parse().unwrap())
            .collect();
        let [y, m, d, h, mi] = numbers[..] else {
            panic!("bad date {text}")
        };
        // Days from civil (Howard Hinnant).
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86_400 + h * 3600 + mi * 60
    }

    #[test]
    fn keys_are_original_starts() {
        let parsed = ParsedCalendar::parse(WEEKLY.as_bytes()).unwrap();
        let instances = parsed.instances(Zone::UTC).unwrap();
        assert!(instances.complete);
        let keys: Vec<i64> = instances.list.iter().map(|i| i.key.naive()).collect();
        assert_eq!(
            keys,
            [
                naive("2026-10-19T10:00"),
                naive("2026-11-02T10:00"),
                naive("2026-11-09T10:00")
            ]
        );
        // The moved one starts at 12:00 Madrid (11:00 UTC, after DST).
        assert_eq!(instances.list[1].utc_start, naive("2026-11-02T11:00"));
    }

    /// An occurrence whose key falls outside the years 1 to 9999 has no
    /// id: it is left out, the rest of the event shows.
    #[test]
    fn keys_outside_the_years_with_ids_are_left_out() {
        assert!(RecurrenceKey::from_naive(RecurrenceKey::MAX.naive()).is_some());
        assert!(RecurrenceKey::from_naive(RecurrenceKey::MAX.naive() + 1).is_none());
        assert!(RecurrenceKey::from_naive(RecurrenceKey::MIN.naive()).is_some());
        assert!(RecurrenceKey::from_naive(RecurrenceKey::MIN.naive() - 1).is_none());
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n\
            BEGIN:VEVENT\r\nUID:[EMAIL]\r\nDTSTAMP:20261001T000000Z\r\n\
            DTSTART:20261019T100000Z\r\nDURATION:PT30M\r\nRRULE:FREQ=WEEKLY;COUNT=2\r\n\
            SUMMARY:Weekly\r\nEND:VEVENT\r\n\
            BEGIN:VEVENT\r\nUID:[EMAIL]\r\nDTSTAMP:20261001T000000Z\r\n\
            RECURRENCE-ID:00000101T000000Z\r\n\
            DTSTART:20261102T100000Z\r\nDURATION:PT30M\r\n\
            SUMMARY:Year zero\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let parsed = ParsedCalendar::parse(text.as_bytes()).unwrap();
        let instances = parsed.instances(Zone::UTC).unwrap();
        // Something was left out.
        assert!(!instances.complete);
        let keys: Vec<i64> = instances.list.iter().map(|i| i.key.naive()).collect();
        assert!(keys.contains(&naive("2026-10-19T10:00")), "{keys:?}");
        assert!(keys.contains(&naive("2026-10-26T10:00")), "{keys:?}");
        assert!(
            keys.iter()
                .all(|key| RecurrenceKey::from_naive(*key).is_some()),
            "{keys:?}"
        );
    }

    #[test]
    fn a_series_is_more_than_one_lone_event() {
        let calendar = |event: &str| {
            let text = format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\n{event}END:VCALENDAR\r\n"
            );
            ParsedCalendar::parse(text.as_bytes()).unwrap()
        };
        let event = |extra: &str| {
            format!(
                "BEGIN:VEVENT\r\nUID:s@example.com\r\nDTSTAMP:20261001T000000Z\r\n\
                 DTSTART:20261019T100000Z\r\n{extra}END:VEVENT\r\n"
            )
        };
        assert!(!calendar(&event("")).is_series());
        assert!(calendar(&event("RRULE:FREQ=DAILY;COUNT=2\r\n")).is_series());
        assert!(calendar(&event("RDATE:20261020T100000Z\r\n")).is_series());
        // One occurrence of a series nobody stored: the event, as in Stalwart.
        assert!(!calendar(&event("RECURRENCE-ID:20261019T100000Z\r\n")).is_series());
        assert!(
            ParsedCalendar::parse(WEEKLY.as_bytes())
                .unwrap()
                .is_series()
        );
    }

    #[test]
    fn an_occurrence_is_its_own_event() {
        let parsed = ParsedCalendar::parse(WEEKLY.as_bytes()).unwrap();
        let options = EventOptions {
            keep_unconverted: true,
            ..EventOptions::default()
        };
        let key = RecurrenceKey::from_naive(naive("2026-11-09T10:00")).unwrap();
        let event = parsed
            .occurrence(key, Zone::UTC, &options)
            .unwrap()
            .unwrap();
        assert_eq!(event.members["start"], "2026-11-09T10:00:00");
        assert_eq!(event.members["recurrenceId"], "2026-11-09T10:00:00");
        assert_eq!(event.members["timeZone"], "Europe/Madrid");
        assert!(event.members.get("recurrenceRule").is_none());
        let moved = RecurrenceKey::from_naive(naive("2026-11-02T10:00")).unwrap();
        let event = parsed
            .occurrence(moved, Zone::UTC, &options)
            .unwrap()
            .unwrap();
        assert_eq!(event.members["title"], "Moved");
        assert_eq!(event.members["start"], "2026-11-02T12:00:00");
        let gone = RecurrenceKey::from_naive(naive("2026-10-26T10:00")).unwrap();
        assert!(
            parsed
                .occurrence(gone, Zone::UTC, &options)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_base_event_has_its_overrides() {
        let parsed = ParsedCalendar::parse(WEEKLY.as_bytes()).unwrap();
        let event = parsed
            .base_event(Zone::UTC, &EventOptions::default())
            .unwrap()
            .unwrap();
        assert_eq!(event.members["title"], "Weekly");
        assert!(event.members["recurrenceOverrides"]["2026-11-02T10:00:00"].is_object());
        assert_eq!(
            event.utc_range,
            Some((naive("2026-10-19T08:00"), naive("2026-10-19T08:30")))
        );
        let none = EventOptions {
            overrides_between: Some((0, 1)),
            ..EventOptions::default()
        };
        let event = parsed.base_event(Zone::UTC, &none).unwrap().unwrap();
        let overrides = event.members.get("recurrenceOverrides");
        // The override is gone; the EXDATE stays as an excluded override.
        assert!(overrides.is_none_or(|o| o.get("2026-11-02T10:00:00").is_none()));
    }

    #[test]
    fn zones_and_ids() {
        assert!(Zone::named("Europe/Madrid").is_some());
        assert!(Zone::named("Mars/Olympus").is_none());
        let madrid = Zone::named("Europe/Madrid").unwrap();
        assert_eq!(
            madrid.to_utc(naive("2026-10-19T10:00")),
            Some(naive("2026-10-19T08:00"))
        );
        assert_eq!(
            participant_number("mailto:Alice@Example.com"),
            participant_number("mailto:alice@example.com")
        );
        assert!(participant_number("") > 0);
    }

    #[test]
    fn parse_returns_every_event() {
        let events = parse_events(WEEKLY.as_bytes()).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["uid"], "w@example.com");
        assert!(parse_events(b"BEGIN:VCARD\r\nEND:VCARD\r\n").is_err());
    }
}
