// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Invitations (iTIP, RFC 5546) for the bridge's own writes
//! without I/O. CPU work: call it off the async
//! threads.
//!
//! - [`role`]: whether the user organizes the event or is invited to it.
//! - [`set_schedule_agent`]: `SCHEDULE-AGENT=CLIENT` (RFC 6638 §7.1) where
//!   a scheduling server must not send anything: on every `ATTENDEE` of
//!   an event the user organizes (the server acts for the organizer by
//!   looking at the attendees), on every `ORGANIZER` of an event the user
//!   is invited to (it acts for an attendee by looking at the organizer).
//!   The same value on every component, as §7.1 requires. Removing it
//!   only removes `CLIENT` (a `NONE` someone wrote stays).
//! - [`organizer_messages`]: what the organizer sends when the bridge
//!   does the scheduling (servers without `calendar-auto-schedule`):
//!   `REQUEST` on create and on changes (only the changed occurrence when
//!   only an override changed; the whole event when an occurrence comes
//!   back, an override or `EXDATE` taken away), `CANCEL` of an occurrence
//!   when it is excluded or to whoever an override leaves out of it, to
//!   attendees removed and on delete. `SEQUENCE` goes up when any part of
//!   the write is a change RFC 5546 §2.1.4 calls significant (times,
//!   rules, `EXDATE` added or taken away, status, location, attendees),
//!   and the new value is written to the stored event too.
//!
//! Tasks (`VTODO`) go the same way: the
//! organizer assigns a task to its attendees with `REQUEST`, and takes it
//! back with `CANCEL` (RFC 5546 §3.4). A task's `DUE` counts as a time.
//!
//! The messages are iCalendar texts and their recipients; turning them
//! into email (iMIP, RFC 6047) is the JMAP layer's job.

use calcard::common::PartialDateTime;
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarEntry, ICalendarMethod,
    ICalendarParameter, ICalendarParameterName, ICalendarParameterValue, ICalendarProperty,
    ICalendarScheduleAgentValue, ICalendarStatus, ICalendarValue,
};

use super::convert::{Format, checked_text, guarded, write_text};
use super::edit::PRODID;
use crate::ConvertError;

/// The user's part in an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Role {
    /// The `ORGANIZER` is one of the user's addresses.
    Organizer,
    /// Someone else organizes it and the user is an `ATTENDEE`.
    Attendee,
    /// No scheduling: no organizer, or the user is not in it.
    Neither,
}

/// What [`set_schedule_agent`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Agent {
    /// `SCHEDULE-AGENT=CLIENT`: the server must not send.
    Client,
    /// No `SCHEDULE-AGENT=CLIENT`: the server sends (the default).
    Server,
}

/// An iTIP method the bridge sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    /// An invitation or an update.
    Request,
    /// A cancellation (of the event, or of one occurrence).
    Cancel,
    /// An attendee proposes another time (RFC 5546 §3.2.7; `counter`).
    Counter,
    /// The organizer turns a proposal down (RFC 5546 §3.2.8; `counter`).
    DeclineCounter,
}

impl Method {
    /// The `METHOD` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "REQUEST",
            Self::Cancel => "CANCEL",
            Self::Counter => "COUNTER",
            Self::DeclineCounter => "DECLINECOUNTER",
        }
    }
}

/// Someone a message goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Party {
    /// The address, without `mailto:`.
    pub address: String,
    /// `CN`, if any.
    pub name: Option<String>,
}

/// What a message is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Subject {
    /// An event (`VEVENT`): an invitation.
    Event,
    /// A task (`VTODO`): an assignment.
    Task,
}

/// One iTIP message, the same for every recipient.
#[derive(Clone, PartialEq, Eq)]
pub struct ItipMessage {
    /// What it is.
    pub method: Method,
    /// An event or a task.
    pub subject: Subject,
    /// The sender's address, without `mailto:`: the organizer's (for a
    /// `COUNTER`, the attendee's who proposes).
    pub organizer: String,
    /// Who gets it.
    pub to: Vec<Party>,
    /// The `text/calendar` part.
    pub ics: String,
    /// The event's title (`SUMMARY`), for the subject. Not checked here:
    /// it may have any character.
    pub title: String,
    /// When (`DTSTART` as written, with its `TZID`; a task's `DUE`, else
    /// its `DTSTART`), for the text part.
    pub when: Option<String>,
    /// The sender's note (`COMMENT`), also for the text part.
    pub comment: Option<String>,
}

impl std::fmt::Debug for ItipMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItipMessage")
            .field("method", &self.method)
            .field("to", &self.to.len())
            .finish_non_exhaustive()
    }
}

/// What the organizer sends after a change, and the text to store (with
/// `SEQUENCE` raised where it had to).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItipPlan {
    /// The text to store instead of the new one (`None`: unchanged).
    pub text: Option<String>,
    /// The messages.
    pub messages: Vec<ItipMessage>,
}

impl ItipPlan {
    /// Every recipient once (case-insensitive addresses).
    #[must_use]
    pub fn recipients(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for party in self.messages.iter().flat_map(|m| m.to.iter()) {
            if !out.iter().any(|a| a.eq_ignore_ascii_case(&party.address)) {
                out.push(&party.address);
            }
        }
        out
    }
}

pub(super) fn parse(text: &[u8]) -> Result<ICalendar, ConvertError> {
    let format = Format::ICalendar;
    let text = checked_text(text, format)?;
    guarded(format, || {
        ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })
    })
}

pub(super) fn is_own(own: &[String], address: &str) -> bool {
    own.iter().any(|a| a.eq_ignore_ascii_case(address))
}

/// The type of the object's scheduling components: the first `VEVENT`
/// or `VTODO` (RFC 5546 §1.4: one type per object; a task is assigned
/// with the same messages an event is sent with, §3.4).
fn scheduled_type(ical: &ICalendar) -> Option<ICalendarComponentType> {
    ical.components
        .iter()
        .map(|c| &c.component_type)
        .find(|t| {
            matches!(
                t,
                ICalendarComponentType::VEvent | ICalendarComponentType::VTodo
            )
        })
        .cloned()
}

/// The events (or tasks) of the object that belong to its main `UID`,
/// main first.
pub(super) fn events(ical: &ICalendar) -> Vec<usize> {
    let Some(kind) = scheduled_type(ical) else {
        return Vec::new();
    };
    let all: Vec<usize> = ical
        .components
        .iter()
        .enumerate()
        .filter(|(_, c)| c.component_type == kind)
        .map(|(i, _)| i)
        .collect();
    let main = all
        .iter()
        .copied()
        .find(|i| {
            ical.components
                .get(*i)
                .is_some_and(|c| !c.is_recurrence_override())
        })
        .or_else(|| all.first().copied());
    let Some(main) = main else {
        return Vec::new();
    };
    let uid = |i: usize| {
        ical.components
            .get(i)
            .and_then(|c| c.property(&ICalendarProperty::Uid))
            .and_then(|e| e.values.first())
            .and_then(ICalendarValue::as_text)
            .map(str::to_owned)
    };
    let main_uid = uid(main);
    let mut out = vec![main];
    out.extend(
        all.into_iter()
            .filter(|i| *i != main && uid(*i) == main_uid),
    );
    out
}

pub(super) fn component(ical: &ICalendar, index: usize) -> Option<&ICalendarComponent> {
    ical.components.get(index)
}

pub(super) fn organizer_of(comp: &ICalendarComponent) -> Option<String> {
    comp.property(&ICalendarProperty::Organizer)
        .and_then(ICalendarEntry::calendar_address)
        .map(str::to_owned)
}

/// The user's part in the event of `text`.
///
/// # Errors
/// [`ConvertError`] if it does not parse.
pub fn role(text: &[u8], own: &[String]) -> Result<Role, ConvertError> {
    let ical = parse(text)?;
    Ok(role_in(&ical, own))
}

fn role_in(ical: &ICalendar, own: &[String]) -> Role {
    let comps = events(ical);
    let Some(main) = comps.first().and_then(|i| component(ical, *i)) else {
        return Role::Neither;
    };
    let Some(organizer) = organizer_of(main) else {
        return Role::Neither;
    };
    if is_own(own, &organizer) {
        return Role::Organizer;
    }
    let invited = comps.iter().filter_map(|i| component(ical, *i)).any(|c| {
        c.properties(&ICalendarProperty::Attendee)
            .filter_map(ICalendarEntry::calendar_address)
            .any(|a| is_own(own, a))
    });
    if invited {
        Role::Attendee
    } else {
        Role::Neither
    }
}

fn is_agent(param: &ICalendarParameter, value: &ICalendarScheduleAgentValue) -> bool {
    param.name == ICalendarParameterName::ScheduleAgent
        && matches!(&param.value, ICalendarParameterValue::ScheduleAgent(v) if v == value)
}

/// Writes or removes `SCHEDULE-AGENT=CLIENT` for `role` (see the module
/// docs). `None` if nothing changes (or the role has no scheduling).
///
/// # Errors
/// [`ConvertError`] if it does not parse.
pub fn set_schedule_agent(
    text: &[u8],
    role: Role,
    agent: Agent,
) -> Result<Option<String>, ConvertError> {
    let property = match role {
        Role::Organizer => ICalendarProperty::Attendee,
        Role::Attendee => ICalendarProperty::Organizer,
        Role::Neither => return Ok(None),
    };
    let mut ical = parse(text)?;
    let mut changed = false;
    for index in events(&ical) {
        let Some(comp) = ical.components.get_mut(index) else {
            continue;
        };
        for entry in comp.entries.iter_mut().filter(|e| e.name == property) {
            let none = entry
                .params
                .iter()
                .any(|p| is_agent(p, &ICalendarScheduleAgentValue::None));
            let client = entry
                .params
                .iter()
                .any(|p| is_agent(p, &ICalendarScheduleAgentValue::Client));
            match agent {
                Agent::Client if !none && !client => {
                    entry
                        .params
                        .retain(|p| p.name != ICalendarParameterName::ScheduleAgent);
                    entry.add_param(ICalendarParameter::schedule_agent(
                        ICalendarParameterValue::ScheduleAgent(ICalendarScheduleAgentValue::Client),
                    ));
                    changed = true;
                }
                Agent::Server if client => {
                    entry
                        .params
                        .retain(|p| !is_agent(p, &ICalendarScheduleAgentValue::Client));
                    changed = true;
                }
                _ => {}
            }
        }
    }
    // What is written back to the calendar carries no METHOD (RFC 4791
    // §4.1), even if another client stored one.
    if changed {
        super::convert::strip_method(&mut ical);
    }
    changed
        .then(|| guarded(Format::ICalendar, || write_text(&ical, Format::ICalendar)))
        .transpose()
}

// ---- What changed ----------------------------------------------------

const SIGNIFICANT: &[ICalendarProperty] = &[
    ICalendarProperty::Dtstart,
    ICalendarProperty::Dtend,
    ICalendarProperty::Duration,
    ICalendarProperty::Due,
    ICalendarProperty::Rrule,
    ICalendarProperty::Rdate,
    ICalendarProperty::Status,
    ICalendarProperty::Location,
];

/// Properties that never make a message (they change on every write, or
/// are compared apart).
const IGNORED: &[ICalendarProperty] = &[
    ICalendarProperty::Dtstamp,
    ICalendarProperty::Sequence,
    ICalendarProperty::LastModified,
    ICalendarProperty::Created,
    ICalendarProperty::Attendee,
    ICalendarProperty::Organizer,
    ICalendarProperty::RecurrenceId,
    ICalendarProperty::Exdate,
    ICalendarProperty::RequestStatus,
];

fn entries_where(
    comp: &ICalendarComponent,
    keep: impl Fn(&ICalendarProperty) -> bool,
) -> Vec<&ICalendarEntry> {
    comp.entries.iter().filter(|e| keep(&e.name)).collect()
}

fn same_entries(a: &[&ICalendarEntry], b: &[&ICalendarEntry]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x)) && b.iter().all(|x| a.contains(x))
}

fn all_attendees(comp: &ICalendarComponent) -> Vec<String> {
    let mut out: Vec<String> = comp
        .properties(&ICalendarProperty::Attendee)
        .filter_map(ICalendarEntry::calendar_address)
        .map(str::to_ascii_lowercase)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// What changed in one component.
#[derive(Debug, Clone, Copy, Default)]
struct Change {
    significant: bool,
    any: bool,
}

fn compare(old: &ICalendarComponent, new: &ICalendarComponent) -> Change {
    let significant = !same_entries(
        &entries_where(old, |n| SIGNIFICANT.contains(n)),
        &entries_where(new, |n| SIGNIFICANT.contains(n)),
    ) || all_attendees(old) != all_attendees(new);
    let other = !same_entries(
        &entries_where(old, |n| !SIGNIFICANT.contains(n) && !IGNORED.contains(n)),
        &entries_where(new, |n| !SIGNIFICANT.contains(n) && !IGNORED.contains(n)),
    );
    Change {
        significant,
        any: significant || other,
    }
}

/// Who gets messages about a component: its attendees but the user, the
/// organizer and those marked `SCHEDULE-AGENT=NONE` (`CLIENT` is us).
fn recipients(comp: &ICalendarComponent, own: &[String], organizer: &str) -> Vec<Party> {
    let mut out: Vec<Party> = Vec::new();
    for entry in comp.properties(&ICalendarProperty::Attendee) {
        let Some(address) = entry.calendar_address() else {
            continue;
        };
        if is_own(own, address)
            || address.eq_ignore_ascii_case(organizer)
            || entry
                .params
                .iter()
                .any(|p| is_agent(p, &ICalendarScheduleAgentValue::None))
            || out.iter().any(|p| p.address.eq_ignore_ascii_case(address))
        {
            continue;
        }
        let name = entry
            .parameter(&ICalendarParameterName::Cn)
            .and_then(ICalendarParameterValue::as_text)
            .map(str::to_owned);
        out.push(Party {
            address: address.to_owned(),
            name,
        });
    }
    out
}

fn union(lists: impl IntoIterator<Item = Vec<Party>>) -> Vec<Party> {
    let mut out: Vec<Party> = Vec::new();
    for party in lists.into_iter().flatten() {
        if !out
            .iter()
            .any(|p| p.address.eq_ignore_ascii_case(&party.address))
        {
            out.push(party);
        }
    }
    out
}

fn minus(a: Vec<Party>, b: &[Party]) -> Vec<Party> {
    a.into_iter()
        .filter(|p| !b.iter().any(|q| q.address.eq_ignore_ascii_case(&p.address)))
        .collect()
}

pub(super) fn sequence(comp: &ICalendarComponent) -> i64 {
    comp.property(&ICalendarProperty::Sequence)
        .and_then(|e| e.values.first())
        .and_then(ICalendarValue::as_integer)
        .unwrap_or(0)
}

fn set_sequence(comp: &mut ICalendarComponent, value: i64) {
    comp.entries
        .retain(|e| e.name != ICalendarProperty::Sequence);
    comp.add_sequence(value);
}

pub(super) fn text_of(comp: &ICalendarComponent, property: &ICalendarProperty) -> Option<String> {
    comp.property(property)
        .and_then(|e| e.values.first())
        .and_then(ICalendarValue::as_text)
        .map(str::to_owned)
}

pub(super) fn recurrence_id(comp: &ICalendarComponent) -> Option<&ICalendarEntry> {
    comp.property(&ICalendarProperty::RecurrenceId)
}

/// Whether two date entries (`RECURRENCE-ID`, `EXDATE`) name the same
/// instant: same value and same `TZID`.
fn same_date(a: &ICalendarEntry, a_value: &ICalendarValue, b: &ICalendarEntry) -> bool {
    let tz = |e: &ICalendarEntry| {
        e.parameter(&ICalendarParameterName::Tzid)
            .and_then(ICalendarParameterValue::as_text)
            .map(str::to_owned)
    };
    tz(a) == tz(b) && b.values.first() == Some(a_value)
}

/// Each `EXDATE` value as its own entry (one value, same parameters).
fn exdates(comp: &ICalendarComponent) -> Vec<ICalendarEntry> {
    comp.properties(&ICalendarProperty::Exdate)
        .flat_map(|e| {
            e.values.iter().map(|v| ICalendarEntry {
                name: ICalendarProperty::Exdate,
                params: e.params.clone(),
                values: std::iter::once(v.clone()).collect(),
            })
        })
        .collect()
}

pub(super) fn when(comp: &ICalendarComponent) -> Option<String> {
    let entry = if comp.component_type == ICalendarComponentType::VTodo {
        comp.property(&ICalendarProperty::Due)
            .or_else(|| comp.property(&ICalendarProperty::Dtstart))?
    } else {
        comp.property(&ICalendarProperty::Dtstart)?
    };
    let dt = entry.values.first()?.as_partial_date_time()?;
    let date = format!("{:04}-{:02}-{:02}", dt.year?, dt.month?, dt.day?);
    let Some(hour) = dt.hour else {
        return Some(date);
    };
    let zone = entry
        .parameter(&ICalendarParameterName::Tzid)
        .and_then(ICalendarParameterValue::as_text)
        .map(str::to_owned)
        .or_else(|| dt.tz_hour.map(|_| "UTC".to_owned()));
    let time = format!("{date} {hour:02}:{:02}", dt.minute.unwrap_or(0));
    Some(match zone {
        Some(zone) => format!("{time} ({zone})"),
        None => time,
    })
}

// ---- Building messages -----------------------------------------------

pub(super) fn envelope(method: Method) -> ICalendarComponent {
    let method = match method {
        Method::Request => ICalendarMethod::Request,
        Method::Cancel => ICalendarMethod::Cancel,
        Method::Counter => ICalendarMethod::Counter,
        Method::DeclineCounter => ICalendarMethod::Declinecounter,
    };
    let mut root = ICalendarComponent::new(ICalendarComponentType::VCalendar);
    root.add_property(
        ICalendarProperty::Prodid,
        ICalendarValue::Text(PRODID.to_owned()),
    );
    root.add_property(
        ICalendarProperty::Version,
        ICalendarValue::Text("2.0".to_owned()),
    );
    root.add_property(ICalendarProperty::Method, ICalendarValue::Method(method));
    root
}

pub(super) fn without_schedule_params(entry: &ICalendarEntry) -> ICalendarEntry {
    let mut entry = entry.clone();
    entry.params.retain(|p| {
        !matches!(
            p.name,
            ICalendarParameterName::ScheduleAgent
                | ICalendarParameterName::ScheduleStatus
                | ICalendarParameterName::ScheduleForceSend
        )
    });
    entry
}

/// A component as the organizer sends it in a REQUEST: everything but
/// the alarms (the attendee's own) and the scheduling parameters, with
/// `DTSTAMP` now.
fn request_component(comp: &ICalendarComponent, stamp: &PartialDateTime) -> ICalendarComponent {
    let mut out = ICalendarComponent::new(comp.component_type.clone());
    out.add_dtstamp(stamp.clone());
    for entry in &comp.entries {
        match entry.name {
            ICalendarProperty::Dtstamp => {}
            ICalendarProperty::Attendee | ICalendarProperty::Organizer => {
                out.entries.push(without_schedule_params(entry));
            }
            _ => out.entries.push(entry.clone()),
        }
    }
    out
}

/// A CANCEL component (RFC 5546 §3.2.5): the identifying properties,
/// `STATUS:CANCELLED`, the new `SEQUENCE` and `to` as attendees (all of
/// them when `to` is `None`).
fn cancel_component(
    comp: &ICalendarComponent,
    seq: i64,
    stamp: &PartialDateTime,
    to: Option<&[Party]>,
    instance: Option<&ICalendarEntry>,
) -> ICalendarComponent {
    let mut out = ICalendarComponent::new(comp.component_type.clone());
    out.add_property(
        ICalendarProperty::Status,
        ICalendarValue::Status(ICalendarStatus::Cancelled),
    );
    out.add_dtstamp(stamp.clone());
    out.add_sequence(seq);
    for entry in &comp.entries {
        let keep = match entry.name {
            ICalendarProperty::Organizer
            | ICalendarProperty::Uid
            | ICalendarProperty::Summary
            | ICalendarProperty::Location
            | ICalendarProperty::Duration => true,
            ICalendarProperty::Dtstart
            | ICalendarProperty::Dtend
            | ICalendarProperty::Due
            | ICalendarProperty::RecurrenceId
            | ICalendarProperty::Rrule => instance.is_none(),
            ICalendarProperty::Attendee => to.is_none_or(|to| {
                entry
                    .calendar_address()
                    .is_some_and(|a| to.iter().any(|p| p.address.eq_ignore_ascii_case(a)))
            }),
            _ => false,
        };
        if keep {
            out.entries.push(without_schedule_params(entry));
        }
    }
    if let Some(date) = instance {
        let mut start = date.clone();
        start.name = ICalendarProperty::Dtstart;
        let mut rid = date.clone();
        rid.name = ICalendarProperty::RecurrenceId;
        out.entries.push(start);
        out.entries.push(rid);
    }
    out
}

/// A message with these event components and the time zones of `source`.
pub(super) fn message(
    source: &ICalendar,
    method: Method,
    events: Vec<ICalendarComponent>,
) -> Result<String, ConvertError> {
    let mut ical = ICalendar {
        components: vec![envelope(method)],
    };
    let mut top: Vec<u32> = Vec::new();
    for event in events {
        top.push(u32::try_from(ical.components.len()).unwrap_or(u32::MAX));
        ical.components.push(event);
    }
    for tz in source
        .components
        .iter()
        .filter(|c| c.component_type == ICalendarComponentType::VTimezone)
    {
        let mut tz_copy = tz.clone();
        let children: Vec<ICalendarComponent> = tz
            .component_ids
            .iter()
            .filter_map(|i| usize::try_from(*i).ok())
            .filter_map(|i| source.components.get(i))
            .map(|c| ICalendarComponent {
                component_ids: Vec::new(),
                ..c.clone()
            })
            .collect();
        top.push(u32::try_from(ical.components.len()).unwrap_or(u32::MAX));
        let mut ids = Vec::new();
        let first = ical.components.len().saturating_add(1);
        for (offset, _) in children.iter().enumerate() {
            ids.push(u32::try_from(first.saturating_add(offset)).unwrap_or(u32::MAX));
        }
        tz_copy.component_ids = ids;
        ical.components.push(tz_copy);
        ical.components.extend(children);
    }
    if let Some(root) = ical.components.first_mut() {
        root.component_ids = top;
    }
    guarded(Format::ICalendar, || {
        ical.add_missing_timezones();
        write_text(&ical, Format::ICalendar)
    })
}

struct Builder<'a> {
    own: &'a [String],
    stamp: PartialDateTime,
    messages: Vec<ItipMessage>,
}

impl Builder<'_> {
    fn push(
        &mut self,
        source: &ICalendar,
        method: Method,
        organizer: &str,
        main: &ICalendarComponent,
        components: Vec<ICalendarComponent>,
        to: Vec<Party>,
    ) -> Result<(), ConvertError> {
        if to.is_empty() || components.is_empty() {
            return Ok(());
        }
        let when = components.first().and_then(when);
        let subject = if main.component_type == ICalendarComponentType::VTodo {
            Subject::Task
        } else {
            Subject::Event
        };
        let ics = message(source, method, components)?;
        self.messages.push(ItipMessage {
            method,
            subject,
            organizer: organizer.to_owned(),
            to,
            ics,
            title: text_of(main, &ICalendarProperty::Summary).unwrap_or_default(),
            when,
            comment: None,
        });
        Ok(())
    }

    /// Every component of `ical` cancelled, to `to` (all attendees if
    /// `None`), each one with its next `SEQUENCE`.
    fn cancel_all(
        &mut self,
        ical: &ICalendar,
        organizer: &str,
        to: Option<Vec<Party>>,
    ) -> Result<(), ConvertError> {
        let comps: Vec<&ICalendarComponent> = events(ical)
            .into_iter()
            .filter_map(|i| component(ical, i))
            .collect();
        let Some(main) = comps.first().copied() else {
            return Ok(());
        };
        let to =
            to.unwrap_or_else(|| union(comps.iter().map(|c| recipients(c, self.own, organizer))));
        let list: Vec<ICalendarComponent> = comps
            .iter()
            .map(|c| {
                cancel_component(
                    c,
                    sequence(c).saturating_add(1),
                    &self.stamp,
                    Some(&to),
                    None,
                )
            })
            .collect();
        self.push(ical, Method::Cancel, organizer, main, list, to)
    }
}

/// What the organizer sends after a write: `old` is the stored text before
/// (`None` for a new event), `new` the text after (`None` when it is
/// deleted). Nothing unless the user (`own`) organizes the event.
///
/// # Errors
/// [`ConvertError`] if a text does not parse or a message can't be
/// written.
pub fn organizer_messages(
    old: Option<&[u8]>,
    new: Option<&[u8]>,
    own: &[String],
    now: i64,
) -> Result<ItipPlan, ConvertError> {
    let old = old.map(parse).transpose()?;
    let new = new.map(parse).transpose()?;
    let mut builder = Builder {
        own,
        stamp: PartialDateTime::from_utc_timestamp(now),
        messages: Vec::new(),
    };
    match (old, new) {
        (None, None) => Ok(ItipPlan::default()),
        (Some(old), None) => {
            if role_in(&old, own) == Role::Organizer {
                let organizer = events(&old)
                    .first()
                    .and_then(|i| component(&old, *i))
                    .and_then(organizer_of)
                    .unwrap_or_default();
                builder.cancel_all(&old, &organizer, None)?;
            }
            Ok(ItipPlan {
                text: None,
                messages: builder.messages,
            })
        }
        (old, Some(mut new)) => {
            if role_in(&new, own) != Role::Organizer {
                return Ok(ItipPlan::default());
            }
            let changed = match &old {
                None => {
                    request_all(&mut builder, &new)?;
                    false
                }
                Some(old) => update(&mut builder, old, &mut new)?,
            };
            let text = if changed {
                Some(guarded(Format::ICalendar, || {
                    write_text(&new, Format::ICalendar)
                })?)
            } else {
                None
            };
            Ok(ItipPlan {
                text,
                messages: builder.messages,
            })
        }
    }
}

/// The whole event to every attendee.
fn request_all(builder: &mut Builder<'_>, ical: &ICalendar) -> Result<(), ConvertError> {
    let comps: Vec<&ICalendarComponent> = events(ical)
        .into_iter()
        .filter_map(|i| component(ical, i))
        .collect();
    let Some(main) = comps.first().copied() else {
        return Ok(());
    };
    let organizer = organizer_of(main).unwrap_or_default();
    let to = union(comps.iter().map(|c| recipients(c, builder.own, &organizer)));
    let list = comps
        .iter()
        .map(|c| request_component(c, &builder.stamp))
        .collect();
    builder.push(ical, Method::Request, &organizer, main, list, to)
}

/// The messages of a change, raising `SEQUENCE` in `new`. Returns whether
/// `new` changed.
fn update(
    builder: &mut Builder<'_>,
    old: &ICalendar,
    new: &mut ICalendar,
) -> Result<bool, ConvertError> {
    let old_ids = events(old);
    let new_ids = events(new);
    let (Some(old_main), Some(new_main)) = (
        old_ids.first().and_then(|i| component(old, *i)),
        new_ids.first().and_then(|i| component(new, *i)),
    ) else {
        return Ok(false);
    };
    let organizer = organizer_of(new_main).unwrap_or_default();
    let main_change = compare(old_main, new_main);
    let old_exdates = exdates(old_main);
    let new_exdates = exdates(new_main);
    let added_exdates: Vec<ICalendarEntry> = new_exdates
        .iter()
        .filter(|e| !old_exdates.contains(e))
        .cloned()
        .collect();
    // An EXDATE taken away: that occurrence is back (compared by its form,
    // `TZID` and value: another client that writes the same date in
    // another form makes it look restored and added, which costs a
    // REQUEST and a CANCEL of a date already cancelled, never a missed
    // one).
    let restored = old_exdates.iter().any(|e| !new_exdates.contains(e));
    let (changed_overrides, lost_override) =
        override_changes(old, new, &old_ids, &new_ids, &added_exdates);
    let old_recipients = union(
        old_ids
            .iter()
            .filter_map(|i| component(old, *i))
            .map(|c| recipients(c, builder.own, &organizer)),
    );
    let new_recipients = union(
        new_ids
            .iter()
            .filter_map(|i| component(new, *i))
            .map(|c| recipients(c, builder.own, &organizer)),
    );
    let removed = minus(old_recipients, &new_recipients);
    let mut bumps: Vec<(usize, i64)> = Vec::new();
    let next = |old_seq: i64, new_seq: i64| old_seq.max(new_seq).saturating_add(1);
    // RFC 5546 §2.1.4: a significant change anywhere in this write (an
    // EXDATE added or taken away counts) raises SEQUENCE, also when it
    // comes with other changes.
    let major = main_change.significant
        || lost_override
        || restored
        || !added_exdates.is_empty()
        || changed_overrides
            .iter()
            .any(|(_, change, _)| change.significant);
    if main_change.any || lost_override || restored {
        // The whole event again, to everyone (an occurrence back in the
        // series is a change of the series).
        if major {
            for index in &new_ids {
                if let Some(comp) = component(new, *index) {
                    bumps.push((*index, next(sequence(old_main), sequence(comp))));
                }
            }
        }
        apply_bumps(new, &bumps);
        request_all(builder, new)?;
    } else {
        if !added_exdates.is_empty() {
            let seq = next(sequence(old_main), sequence(new_main));
            bumps.push((new_ids.first().copied().unwrap_or(0), seq));
            let to = recipients(new_main, builder.own, &organizer);
            let list: Vec<ICalendarComponent> = added_exdates
                .iter()
                .map(|date| cancel_component(new_main, seq, &builder.stamp, None, Some(date)))
                .collect();
            let main = new_main.clone();
            builder.push(old, Method::Cancel, &organizer, &main, list, to)?;
        }
        for (index, change, _) in &changed_overrides {
            let Some(comp) = component(new, *index) else {
                continue;
            };
            if change.significant {
                bumps.push((*index, next(sequence(old_main), sequence(comp))));
            }
        }
        apply_bumps(new, &bumps);
        for (index, _, _) in &changed_overrides {
            let Some(comp) = component(new, *index).cloned() else {
                continue;
            };
            let to = recipients(&comp, builder.own, &organizer);
            let cancelled = matches!(comp.status(), Some(ICalendarStatus::Cancelled));
            let (method, part) = if cancelled {
                (
                    Method::Cancel,
                    cancel_component(&comp, sequence(&comp), &builder.stamp, None, None),
                )
            } else {
                (Method::Request, request_component(&comp, &builder.stamp))
            };
            let source = new.clone();
            builder.push(&source, method, &organizer, &comp, vec![part], to)?;
        }
    }
    left_occurrences(builder, old, new, old_main, &changed_overrides, &removed)?;
    if !removed.is_empty() {
        builder.cancel_all(old, &organizer, Some(removed))?;
    }
    Ok(!bumps.is_empty())
}

/// RFC 5546 §3.2.5: who was invited to an occurrence that an override now
/// leaves them out of (but who stays in the event: those get the whole
/// CANCEL) gets a CANCEL of that occurrence alone. Before, the
/// occurrence's attendees were the old override's, or the series' for a
/// new one.
fn left_occurrences(
    builder: &mut Builder<'_>,
    old: &ICalendar,
    new: &ICalendar,
    old_main: &ICalendarComponent,
    changed_overrides: &[(usize, Change, Option<usize>)],
    removed: &[Party],
) -> Result<(), ConvertError> {
    let organizer = organizer_of(old_main).unwrap_or_default();
    for (index, _, before) in changed_overrides {
        let Some(comp) = component(new, *index) else {
            continue;
        };
        let Some(rid) = recurrence_id(comp) else {
            continue;
        };
        let before = before.and_then(|i| component(old, i));
        let had = recipients(before.unwrap_or(old_main), builder.own, &organizer);
        let left = minus(
            minus(had, &recipients(comp, builder.own, &organizer)),
            removed,
        );
        if left.is_empty() {
            continue;
        }
        // The old component names them as attendees; the occurrence and
        // SEQUENCE are the new override's.
        let part = match before {
            Some(before) => {
                cancel_component(before, sequence(comp), &builder.stamp, Some(&left), None)
            }
            None => cancel_component(
                old_main,
                sequence(comp),
                &builder.stamp,
                Some(&left),
                Some(rid),
            ),
        };
        let main = comp.clone();
        builder.push(old, Method::Cancel, &organizer, &main, vec![part], left)?;
    }
    Ok(())
}

/// Overrides that are new or changed in `new` (with the index of the old
/// one, if there was one), and whether one of `old` is gone without an
/// `EXDATE` (the occurrence went back to the series).
fn override_changes(
    old: &ICalendar,
    new: &ICalendar,
    old_ids: &[usize],
    new_ids: &[usize],
    added_exdates: &[ICalendarEntry],
) -> (Vec<(usize, Change, Option<usize>)>, bool) {
    let excluded = |rid: &ICalendarEntry| {
        added_exdates
            .iter()
            .any(|e| e.values.first().is_some_and(|v| same_date(e, v, rid)))
    };
    let mut changed_overrides: Vec<(usize, Change, Option<usize>)> = Vec::new();
    let mut lost_override = false;
    for index in new_ids.iter().skip(1) {
        let Some(comp) = component(new, *index) else {
            continue;
        };
        let Some(rid) = recurrence_id(comp) else {
            continue;
        };
        let before = old_ids.iter().skip(1).copied().find(|i| {
            component(old, *i)
                .and_then(recurrence_id)
                .is_some_and(|r| r == rid)
        });
        let change = before.and_then(|i| component(old, i)).map_or(
            Change {
                significant: true,
                any: true,
            },
            |before| compare(before, comp),
        );
        if change.any {
            changed_overrides.push((*index, change, before));
        }
    }
    for comp in old_ids.iter().skip(1).filter_map(|i| component(old, *i)) {
        let Some(rid) = recurrence_id(comp) else {
            continue;
        };
        let kept = new_ids
            .iter()
            .skip(1)
            .filter_map(|i| component(new, *i))
            .any(|c| recurrence_id(c) == Some(rid));
        if !kept && !excluded(rid) {
            lost_override = true;
        }
    }
    (changed_overrides, lost_override)
}

fn apply_bumps(ical: &mut ICalendar, bumps: &[(usize, i64)]) {
    for (index, seq) in bumps {
        if let Some(comp) = ical.components.get_mut(*index) {
            set_sequence(comp, *seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: &[&str] = &["alice@example.com"];

    fn own() -> Vec<String> {
        OWN.iter().map(|s| (*s).to_owned()).collect()
    }

    fn event(extra: &str, attendees: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
             UID:e@example.com\r\nDTSTAMP:20261001T000000Z\r\nSEQUENCE:1\r\n\
             DTSTART;TZID=Europe/Madrid:20261019T100000\r\nDURATION:PT1H\r\n\
             SUMMARY:Sync\r\nORGANIZER;CN=Alice:mailto:alice@example.com\r\n\
             ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.com\r\n{attendees}{extra}\
             END:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    const BOB: &str = "ATTENDEE;CN=Bob;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:bob@example.com\r\n";
    const CAROL: &str = "ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:carol@example.com\r\n";

    #[test]
    fn roles() {
        let organized = event("", BOB);
        assert_eq!(role(organized.as_bytes(), &own()).unwrap(), Role::Organizer);
        let bob = vec!["bob@example.com".to_owned()];
        assert_eq!(role(organized.as_bytes(), &bob).unwrap(), Role::Attendee);
        let carol = vec!["carol@example.com".to_owned()];
        assert_eq!(role(organized.as_bytes(), &carol).unwrap(), Role::Neither);
        let plain = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:x\r\n\
                     DTSTART:20261019T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(role(plain.as_bytes(), &own()).unwrap(), Role::Neither);
    }

    /// RFC 6638 §7.1: on the attendees of the organizer's copy, on the
    /// organizer of an attendee's copy; `NONE` stays.
    #[test]
    fn schedule_agent_placement() {
        let none = "ATTENDEE;SCHEDULE-AGENT=NONE:mailto:dave@example.com\r\n";
        let text = event("", &format!("{BOB}{none}"));
        let marked = set_schedule_agent(text.as_bytes(), Role::Organizer, Agent::Client)
            .unwrap()
            .unwrap();
        let attendee_lines: Vec<&str> = marked
            .split("\r\n")
            .filter(|l| l.starts_with("ATTENDEE"))
            .collect();
        assert_eq!(attendee_lines.len(), 3);
        assert_eq!(
            marked.matches("SCHEDULE-AGENT=CLIENT").count(),
            2,
            "{marked}"
        );
        assert!(marked.contains("SCHEDULE-AGENT=NONE"));
        assert!(
            !marked
                .lines()
                .any(|l| l.starts_with("ORGANIZER") && l.contains("SCHEDULE-AGENT"))
        );
        // Again: nothing to do.
        assert!(
            set_schedule_agent(marked.as_bytes(), Role::Organizer, Agent::Client)
                .unwrap()
                .is_none()
        );
        let cleared = set_schedule_agent(marked.as_bytes(), Role::Organizer, Agent::Server)
            .unwrap()
            .unwrap();
        assert!(!cleared.contains("SCHEDULE-AGENT=CLIENT"));
        assert!(cleared.contains("SCHEDULE-AGENT=NONE"));
        // RFC 4791 §4.1: a METHOD another client stored goes when we write.
        let with_method = text.replacen("VERSION:2.0\r\n", "VERSION:2.0\r\nMETHOD:REQUEST\r\n", 1);
        assert!(with_method.contains("METHOD:REQUEST"));
        let marked = set_schedule_agent(with_method.as_bytes(), Role::Organizer, Agent::Client)
            .unwrap()
            .unwrap();
        assert!(!marked.contains("METHOD"), "{marked}");
        // An attendee's copy: the ORGANIZER, in every component.
        let series = format!(
            "{}BEGIN:VEVENT\r\nUID:e@example.com\r\nRECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n\
             DTSTART;TZID=Europe/Madrid:20261026T120000\r\nDURATION:PT1H\r\n\
             ORGANIZER:mailto:alice@example.com\r\n{BOB}END:VEVENT\r\nEND:VCALENDAR\r\n",
            event("RRULE:FREQ=WEEKLY;COUNT=3\r\n", BOB).trim_end_matches("END:VCALENDAR\r\n")
        );
        let bob = set_schedule_agent(series.as_bytes(), Role::Attendee, Agent::Client)
            .unwrap()
            .unwrap();
        let organizers: Vec<&str> = bob
            .split("\r\n")
            .filter(|l| l.starts_with("ORGANIZER"))
            .collect();
        assert_eq!(organizers.len(), 2);
        assert!(
            organizers
                .iter()
                .all(|l| l.contains("SCHEDULE-AGENT=CLIENT"))
        );
        assert!(!bob.contains("ATTENDEE;CN=Bob;PARTSTAT=NEEDS-ACTION;RSVP=TRUE;SCHEDULE"));
        assert!(
            set_schedule_agent(series.as_bytes(), Role::Neither, Agent::Client)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn create_and_delete() {
        let text = event("", &format!("{BOB}{CAROL}"));
        let plan = organizer_messages(None, Some(text.as_bytes()), &own(), 0).unwrap();
        assert!(plan.text.is_none());
        assert_eq!(plan.messages.len(), 1);
        let request = &plan.messages[0];
        assert_eq!(request.method, Method::Request);
        assert_eq!(plan.recipients(), ["bob@example.com", "carol@example.com"]);
        assert_eq!(request.to[0].name.as_deref(), Some("Bob"));
        assert!(request.ics.contains("METHOD:REQUEST"), "{}", request.ics);
        assert!(request.ics.contains("BEGIN:VTIMEZONE"), "{}", request.ics);
        assert_eq!(request.title, "Sync");
        assert_eq!(
            request.when.as_deref(),
            Some("2026-10-19 10:00 (Europe/Madrid)")
        );
        let gone = organizer_messages(Some(text.as_bytes()), None, &own(), 0).unwrap();
        let cancel = &gone.messages[0];
        assert_eq!(cancel.method, Method::Cancel);
        assert!(cancel.ics.contains("STATUS:CANCELLED") && cancel.ics.contains("SEQUENCE:2"));
        // Not the organizer: nothing.
        let bob = vec!["bob@example.com".to_owned()];
        assert!(
            organizer_messages(None, Some(text.as_bytes()), &bob, 0)
                .unwrap()
                .messages
                .is_empty()
        );
    }

    #[test]
    fn changes() {
        let before = event("", &format!("{BOB}{CAROL}"));
        // A new title: an update, same SEQUENCE.
        let renamed = before.replace("SUMMARY:Sync", "SUMMARY:Sync 2");
        let plan = organizer_messages(Some(before.as_bytes()), Some(renamed.as_bytes()), &own(), 0)
            .unwrap();
        assert!(plan.text.is_none());
        assert_eq!(plan.messages.len(), 1);
        assert!(plan.messages[0].ics.contains("SEQUENCE:1"));
        // A new time: SEQUENCE up, also in the stored text.
        let moved = before.replace("20261019T100000", "20261019T110000");
        let plan =
            organizer_messages(Some(before.as_bytes()), Some(moved.as_bytes()), &own(), 0).unwrap();
        assert!(plan.text.as_deref().unwrap().contains("SEQUENCE:2"));
        assert!(plan.messages[0].ics.contains("SEQUENCE:2"));
        // Only an answer recorded: nothing.
        let answered = before.replace(
            "PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:bob",
            "PARTSTAT=ACCEPTED;RSVP=TRUE:mailto:bob",
        );
        let plan = organizer_messages(
            Some(before.as_bytes()),
            Some(answered.as_bytes()),
            &own(),
            0,
        )
        .unwrap();
        assert!(plan.messages.is_empty() && plan.text.is_none());
        // Carol removed: a REQUEST to Bob, a CANCEL to Carol only.
        let without = event("", BOB);
        let plan = organizer_messages(Some(before.as_bytes()), Some(without.as_bytes()), &own(), 0)
            .unwrap();
        let methods: Vec<(Method, Vec<&str>)> = plan
            .messages
            .iter()
            .map(|m| (m.method, m.to.iter().map(|p| p.address.as_str()).collect()))
            .collect();
        assert_eq!(
            methods,
            [
                (Method::Request, vec!["bob@example.com"]),
                (Method::Cancel, vec!["carol@example.com"])
            ]
        );
        let cancel = &plan.messages[1].ics;
        assert!(!cancel.contains("bob@example.com"), "{cancel}");
    }

    #[test]
    fn one_occurrence() {
        let series = event("RRULE:FREQ=WEEKLY;COUNT=4\r\n", BOB);
        // Excluded: a CANCEL of that occurrence only.
        let excluded = series.replace(
            "RRULE:FREQ=WEEKLY;COUNT=4\r\n",
            "RRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Madrid:20261026T100000\r\n",
        );
        let plan = organizer_messages(
            Some(series.as_bytes()),
            Some(excluded.as_bytes()),
            &own(),
            0,
        )
        .unwrap();
        assert_eq!(plan.messages.len(), 1);
        let cancel = &plan.messages[0];
        assert_eq!(cancel.method, Method::Cancel);
        assert!(
            cancel
                .ics
                .contains("RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000"),
            "{}",
            cancel.ics
        );
        assert!(!cancel.ics.contains("FREQ=WEEKLY"), "{}", cancel.ics);
        assert!(plan.text.as_deref().unwrap().contains("SEQUENCE:2"));
        // A moved occurrence: a REQUEST with that override alone.
        let moved = format!(
            "{}BEGIN:VEVENT\r\nUID:e@example.com\r\nDTSTAMP:20261001T000000Z\r\nSEQUENCE:1\r\n\
             RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n\
             DTSTART;TZID=Europe/Madrid:20261026T120000\r\nDURATION:PT1H\r\nSUMMARY:Sync\r\n\
             ORGANIZER:mailto:alice@example.com\r\n{BOB}END:VEVENT\r\nEND:VCALENDAR\r\n",
            series.trim_end_matches("END:VCALENDAR\r\n")
        );
        let plan =
            organizer_messages(Some(series.as_bytes()), Some(moved.as_bytes()), &own(), 0).unwrap();
        assert_eq!(plan.messages.len(), 1);
        let request = &plan.messages[0].ics;
        assert_eq!(request.matches("BEGIN:VEVENT").count(), 1, "{request}");
        assert!(request.contains("RECURRENCE-ID") && request.contains("SEQUENCE:2"));
        assert!(!request.contains("FREQ=WEEKLY"));
    }

    /// The line of `text` that starts with `prefix`, with its CRLF.
    fn line_of(text: &str, prefix: &str) -> String {
        let found = text.split("\r\n").find(|l| l.starts_with(prefix)).unwrap();
        format!("{found}\r\n")
    }

    fn address(attendee: &str) -> String {
        attendee
            .trim_end()
            .rsplit_once("mailto:")
            .unwrap()
            .1
            .to_owned()
    }

    /// `series` with an override of 26 Oct at 12:00 for `attendees`.
    fn with_override(series: &str, attendees: &str) -> String {
        format!(
            "{}BEGIN:VEVENT\r\n{}DTSTAMP:20261001T000000Z\r\nSEQUENCE:1\r\n\
             RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000\r\n\
             DTSTART;TZID=Europe/Madrid:20261026T120000\r\nDURATION:PT1H\r\nSUMMARY:Sync\r\n\
             {}{attendees}END:VEVENT\r\nEND:VCALENDAR\r\n",
            series.trim_end_matches("END:VCALENDAR\r\n"),
            line_of(series, "UID:"),
            line_of(series, "ORGANIZER"),
        )
    }

    fn sent(plan: &ItipPlan) -> Vec<(Method, Vec<String>)> {
        plan.messages
            .iter()
            .map(|m| (m.method, m.to.iter().map(|p| p.address.clone()).collect()))
            .collect()
    }

    /// An EXDATE taken away: the occurrence is back, everyone gets the
    /// whole event with a new SEQUENCE.
    #[test]
    fn a_restored_occurrence_is_sent() {
        let series = event("RRULE:FREQ=WEEKLY;COUNT=4\r\n", &format!("{BOB}{CAROL}"));
        let excluded = series.replace(
            "RRULE:FREQ=WEEKLY;COUNT=4\r\n",
            "RRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Madrid:20261026T100000\r\n",
        );
        let plan = organizer_messages(
            Some(excluded.as_bytes()),
            Some(series.as_bytes()),
            &own(),
            0,
        )
        .unwrap();
        assert_eq!(
            sent(&plan),
            [(Method::Request, vec![address(BOB), address(CAROL)])]
        );
        let ics = &plan.messages[0].ics;
        assert!(ics.contains("SEQUENCE:2"), "{ics}");
        assert!(!ics.contains("EXDATE"), "{ics}");
        assert!(plan.text.as_deref().unwrap().contains("SEQUENCE:2"));
    }

    /// Bob left out of one occurrence (a new override, or one he was in):
    /// a CANCEL of that occurrence to him, the override to the others.
    #[test]
    fn left_out_of_one_occurrence() {
        let series = event("RRULE:FREQ=WEEKLY;COUNT=4\r\n", &format!("{BOB}{CAROL}"));
        let only_carol = with_override(&series, CAROL);
        let check = |plan: &ItipPlan| {
            assert_eq!(
                sent(plan),
                [
                    (Method::Request, vec![address(CAROL)]),
                    (Method::Cancel, vec![address(BOB)]),
                ]
            );
            let cancel = &plan.messages[1].ics;
            assert!(
                cancel.contains("RECURRENCE-ID;TZID=Europe/Madrid:20261026T100000"),
                "{cancel}"
            );
            assert!(cancel.contains(&address(BOB)), "{cancel}");
            assert!(!cancel.contains(&address(CAROL)), "{cancel}");
            assert!(!cancel.contains("FREQ=WEEKLY"), "{cancel}");
            assert_eq!(cancel.matches("BEGIN:VEVENT").count(), 1, "{cancel}");
        };
        let plan = organizer_messages(
            Some(series.as_bytes()),
            Some(only_carol.as_bytes()),
            &own(),
            0,
        )
        .unwrap();
        check(&plan);
        let both = with_override(&series, &format!("{BOB}{CAROL}"));
        let plan = organizer_messages(
            Some(both.as_bytes()),
            Some(only_carol.as_bytes()),
            &own(),
            0,
        )
        .unwrap();
        check(&plan);
    }

    /// A significant change together with another one still raises
    /// SEQUENCE; a title alone does not.
    #[test]
    fn mixed_changes_raise_the_sequence() {
        let series = event("RRULE:FREQ=WEEKLY;COUNT=4\r\n", BOB);
        let renamed_and_excluded = series.replace("SUMMARY:Sync", "SUMMARY:Sync 2").replace(
            "RRULE:FREQ=WEEKLY;COUNT=4\r\n",
            "RRULE:FREQ=WEEKLY;COUNT=4\r\nEXDATE;TZID=Europe/Madrid:20261026T100000\r\n",
        );
        let plan = organizer_messages(
            Some(series.as_bytes()),
            Some(renamed_and_excluded.as_bytes()),
            &own(),
            0,
        )
        .unwrap();
        assert_eq!(plan.messages[0].method, Method::Request);
        assert!(plan.messages[0].ics.contains("SEQUENCE:2"));
        assert!(plan.text.as_deref().unwrap().contains("SEQUENCE:2"));

        let renamed_and_moved =
            with_override(&series, BOB).replacen("SUMMARY:Sync", "SUMMARY:Sync 2", 1);
        let plan = organizer_messages(
            Some(series.as_bytes()),
            Some(renamed_and_moved.as_bytes()),
            &own(),
            0,
        )
        .unwrap();
        assert!(plan.messages[0].ics.contains("SEQUENCE:2"));
        assert!(plan.text.as_deref().unwrap().contains("SEQUENCE:2"));

        let renamed = series.replace("SUMMARY:Sync", "SUMMARY:Sync 2");
        let plan = organizer_messages(Some(series.as_bytes()), Some(renamed.as_bytes()), &own(), 0)
            .unwrap();
        assert!(plan.messages[0].ics.contains("SEQUENCE:1"));
        assert!(plan.text.is_none());
    }

    #[test]
    fn no_schedule_params_or_alarms_go_out() {
        let text = event(
            "BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT5M\r\nEND:VALARM\r\n",
            "ATTENDEE;SCHEDULE-AGENT=CLIENT;SCHEDULE-STATUS=1.2:mailto:bob@example.com\r\n",
        );
        let plan = organizer_messages(None, Some(text.as_bytes()), &own(), 0).unwrap();
        let ics = &plan.messages[0].ics;
        assert!(!ics.contains("SCHEDULE-"), "{ics}");
        assert!(!ics.contains("VALARM"), "{ics}");
        assert_eq!(plan.recipients(), ["bob@example.com"]);
    }

    fn task(extra: &str, attendees: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VTODO\r\n\
             UID:t@example.com\r\nDTSTAMP:20261001T000000Z\r\nSEQUENCE:0\r\n\
             DUE;VALUE=DATE:20261106\r\nSUMMARY:Informe\r\nSTATUS:NEEDS-ACTION\r\n\
             ORGANIZER;CN=Alice:mailto:alice@example.com\r\n\
             ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.com\r\n{attendees}{extra}\
             END:VTODO\r\nEND:VCALENDAR\r\n"
        )
    }

    /// Assigning a task: a REQUEST with
    /// the VTODO, its due date as the time; an update; taking it back from
    /// one assignee; deleting it.
    #[test]
    fn tasks_are_assigned() {
        let text = task("", &format!("{BOB}{CAROL}"));
        assert_eq!(role(text.as_bytes(), &own()).unwrap(), Role::Organizer);
        let plan = organizer_messages(None, Some(text.as_bytes()), &own(), 0).unwrap();
        assert_eq!(plan.messages.len(), 1);
        let request = &plan.messages[0];
        assert_eq!(
            (request.method, request.subject),
            (Method::Request, Subject::Task)
        );
        assert!(request.ics.contains("BEGIN:VTODO") && !request.ics.contains("VEVENT"));
        assert_eq!(request.when.as_deref(), Some("2026-11-06"));
        assert_eq!(plan.recipients(), ["bob@example.com", "carol@example.com"]);
        // A new due date: SEQUENCE up.
        let moved = text.replace("20261106", "20261107");
        let plan =
            organizer_messages(Some(text.as_bytes()), Some(moved.as_bytes()), &own(), 0).unwrap();
        assert!(plan.text.as_deref().unwrap().contains("SEQUENCE:1"));
        assert!(plan.messages[0].ics.contains("DUE;VALUE=DATE:20261107"));
        // Bob completing it (his answer recorded): nothing goes out.
        let done = text.replace(
            "PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:bob",
            "PARTSTAT=COMPLETED;RSVP=TRUE:mailto:bob",
        );
        let plan =
            organizer_messages(Some(text.as_bytes()), Some(done.as_bytes()), &own(), 0).unwrap();
        assert!(plan.messages.is_empty());
        // Carol taken off: a CANCEL to her, with the task's due date.
        let without = task("", BOB);
        let plan =
            organizer_messages(Some(text.as_bytes()), Some(without.as_bytes()), &own(), 0).unwrap();
        let cancel = plan
            .messages
            .iter()
            .find(|m| m.method == Method::Cancel)
            .unwrap();
        assert_eq!(cancel.to[0].address, "carol@example.com");
        assert!(cancel.ics.contains("BEGIN:VTODO") && cancel.ics.contains("DUE;VALUE=DATE"));
        assert!(!cancel.ics.contains("bob@example.com"), "{}", cancel.ics);
        // Deleted: a CANCEL to everyone.
        let gone = organizer_messages(Some(text.as_bytes()), None, &own(), 0).unwrap();
        assert_eq!(gone.messages[0].method, Method::Cancel);
        assert_eq!(gone.recipients().len(), 2);
        // bob's copy: he is an attendee (the web sends his REPLY).
        let bob = vec!["bob@example.com".to_owned()];
        assert_eq!(role(text.as_bytes(), &bob).unwrap(), Role::Attendee);
        let marked = set_schedule_agent(text.as_bytes(), Role::Attendee, Agent::Client)
            .unwrap()
            .unwrap();
        assert!(
            marked.contains("ORGANIZER;CN=Alice;SCHEDULE-AGENT=CLIENT"),
            "{marked}"
        );
    }
}
