// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! iCalendar ↔ JSCalendar and vCard ↔ JSContact with calcard
//! and recurrence expansion.
//!
//! Everything a DAV server sends is hostile: each function checks the size
//! first and runs calcard inside [`std::panic::catch_unwind`], so a panic in
//! the parser (`overflow-checks` are on in release) only fails that resource
//! with [`ConvertError::Panicked`].
//!
//! The JSON is what the web expects from `CalendarEvent/get` and
//! `ContactCard/get` (JSCalendar "bis" and JSContact property names:
//! `@type`, `start`, `timeZone`, `recurrenceRules`, `participants`,
//! `alerts`, unknown iCalendar properties under `iCalendar`…). The JMAP
//! properties that depend on the account (`id`, `calendarIds`, `utcStart`,
//! `baseEventId`, `isOrigin`…) are added by the JMAP layer, not here.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use calcard::common::timezone::Tz;
use calcard::icalendar::{
    ICalendar, ICalendarComponentType, ICalendarParameterName, ICalendarProperty,
};
use calcard::jscalendar::JSCalendar;
use calcard::jscontact::JSContact;
use calcard::vcard::{
    VCard, VCardEntry, VCardParameterName, VCardProperty, VCardValue, VCardVersion,
};
use serde_json::Value;

use crate::ConvertError;
use crate::model::apple_groups;

/// Largest iCalendar object accepted.
pub const MAX_ICALENDAR_BYTES: usize = 1024 * 1024;
/// Largest vCard accepted: bigger than iCalendar for inline `PHOTO`s.
pub const MAX_VCARD_BYTES: usize = 4 * 1024 * 1024;
/// Largest JSCalendar JSON accepted: twice the iCalendar limit, since JSON
/// with ids and `@type`s is longer than the same event in iCalendar.
pub const MAX_JSCALENDAR_BYTES: usize = 2 * MAX_ICALENDAR_BYTES;
/// Largest JSContact JSON accepted.
pub const MAX_JSCONTACT_BYTES: usize = 2 * MAX_VCARD_BYTES;
/// Most occurrences one expansion returns, whatever the caller asks for.
/// calcard also caps its own work per rule, so a rule that never matches
/// (e.g. `FREQ=SECONDLY;BYMONTH=2;BYMONTHDAY=30`) still ends.
pub const MAX_OCCURRENCES: usize = 10_000;

// calcard's id types (JMAP ids and blob ids): plain strings, the JMAP layer
// maps them to its newtypes.
type CalendarObject<'x> = JSCalendar<'x, String, String>;
type ContactObject<'x> = JSContact<'x, String, String>;

/// The format an error is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    ICalendar,
    VCard,
    JsCalendar,
    JsContact,
}

impl Format {
    /// The size limit for this format.
    #[must_use]
    pub const fn limit(self) -> usize {
        match self {
            Self::ICalendar => MAX_ICALENDAR_BYTES,
            Self::VCard => MAX_VCARD_BYTES,
            Self::JsCalendar => MAX_JSCALENDAR_BYTES,
            Self::JsContact => MAX_JSCONTACT_BYTES,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ICalendar => "iCalendar",
            Self::VCard => "vCard",
            Self::JsCalendar => "JSCalendar",
            Self::JsContact => "JSContact",
        })
    }
}

/// One occurrence of an expanded calendar object, in seconds since the Unix
/// epoch (UTC). `component` is calcard's index of the `VEVENT`/`VTODO` it
/// comes from (an override has its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Occurrence {
    pub utc_start: i64,
    pub utc_end: i64,
    pub component: u32,
}

/// The result of [`expand_icalendar`]. `complete` is false when a limit was
/// reached or a component could not be expanded (bad `DTSTART` or `RRULE`):
/// the occurrences found are still returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub occurrences: Vec<Occurrence>,
    pub complete: bool,
}

/// Whether a name calcard keeps as it came (`Other`: an unknown
/// property, parameter or component, or a vCard group) can be written
/// without breaking the line. calcard writes these names as they are
/// (values and parameter values it escapes), so a CR/LF, `:` or `;` from
/// the JSON (`iCalendar.properties`, `vCard.properties`) would add
/// properties or components of its own. Only what breaks the format is
/// refused, not every name outside RFC 5545's token: servers and other
/// clients write `X-FOO_BAR` or non-ASCII names, and those events must
/// stay editable. `dot`: a `.` breaks it too (vCard: it separates the
/// group).
fn writable_name(name: &str, dot: bool) -> bool {
    !name.is_empty()
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, ':' | ';' | ',' | '=' | '"' | ' ') || (dot && c == '.')
        })
}

/// [`writable_name`] for every `Other` name of an iCalendar object about
/// to be written.
///
/// # Errors
/// [`ConvertError::Export`] (as JSCalendar) for a name that is not.
pub(crate) fn check_ical_names(ical: &ICalendar) -> Result<(), ConvertError> {
    // As calcard's own refusal (`write_text`): the object can't be written.
    let bad = || ConvertError::Export {
        format: Format::JsCalendar,
    };
    for component in &ical.components {
        if let ICalendarComponentType::Other(name) = &component.component_type
            && !writable_name(name, false)
        {
            return Err(bad());
        }
        for entry in &component.entries {
            if let ICalendarProperty::Other(name) = &entry.name
                && !writable_name(name, false)
            {
                return Err(bad());
            }
            for param in &entry.params {
                if let ICalendarParameterName::Other(name) = &param.name
                    && !writable_name(name, false)
                {
                    return Err(bad());
                }
            }
        }
    }
    Ok(())
}

/// [`writable_name`] for every `Other` name and group of a vCard about to
/// be written.
///
/// # Errors
/// [`ConvertError::Export`] (as JSContact) for a name that is not.
pub(crate) fn check_vcard_names(card: &VCard) -> Result<(), ConvertError> {
    // As calcard's own refusal (`write_text`): the object can't be written.
    let bad = || ConvertError::Export {
        format: Format::JsContact,
    };
    for entry in &card.entries {
        if entry
            .group
            .as_deref()
            .is_some_and(|group| !writable_name(group, true))
        {
            return Err(bad());
        }
        if let VCardProperty::Other(name) = &entry.name
            && !writable_name(name, true)
        {
            return Err(bad());
        }
        for param in &entry.params {
            if let VCardParameterName::Other(name) = &param.name
                && !writable_name(name, true)
            {
                return Err(bad());
            }
        }
    }
    Ok(())
}

/// iCalendar text → JSCalendar JSON (a `Group` with the events in
/// `entries`, as calcard returns it for a `VCALENDAR`).
///
/// # Errors
///
/// [`ConvertError::TooLarge`], [`ConvertError::NotUtf8`],
/// [`ConvertError::Invalid`] (not one `VCALENDAR`), or
/// [`ConvertError::Panicked`].
pub fn icalendar_to_jscalendar(input: &[u8]) -> Result<Value, ConvertError> {
    let format = Format::ICalendar;
    let text = checked_text(input, format)?;
    guarded(format, || {
        let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        let js: CalendarObject<'static> = ical.into_jscalendar();
        serde_json::to_value(&js.0).map_err(|_| ConvertError::Invalid { format })
    })
}

/// JSCalendar JSON → iCalendar text.
///
/// # Errors
///
/// [`ConvertError::TooLarge`], [`ConvertError::Invalid`] (not JSON),
/// [`ConvertError::Export`] (JSON that is not a JSCalendar object calcard
/// can write), or [`ConvertError::Panicked`].
pub fn jscalendar_to_icalendar(json: &str) -> Result<String, ConvertError> {
    let format = Format::JsCalendar;
    check_size(json.len(), format)?;
    guarded(format, || {
        let js = CalendarObject::parse(json).map_err(|_| ConvertError::Invalid { format })?;
        let ical = js
            .into_icalendar()
            .map_err(|_| ConvertError::Export { format })?;
        check_ical_names(&ical)?;
        write_text(&ical, format)
    })
}

/// vCard text (3.0 or 4.0) → JSContact JSON (a `Card`). Apple's groups
/// (`X-ADDRESSBOOKSERVER-KIND` and `-MEMBER`) read as `kind` and
/// `members`.
///
/// # Errors
///
/// [`ConvertError::TooLarge`], [`ConvertError::NotUtf8`],
/// [`ConvertError::Invalid`] (not one `VCARD`), or
/// [`ConvertError::Panicked`].
pub fn vcard_to_jscontact(input: &[u8]) -> Result<Value, ConvertError> {
    let format = Format::VCard;
    let text = checked_text(input, format)?;
    guarded(format, || {
        let mut card = VCard::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        apple_groups::read(&mut card);
        let js: ContactObject<'static> = card.into_jscontact();
        serde_json::to_value(&js.0).map_err(|_| ConvertError::Invalid { format })
    })
}

/// JSContact JSON → vCard 4.0 text.
///
/// # Errors
///
/// [`ConvertError::TooLarge`], [`ConvertError::Invalid`] (not JSON),
/// [`ConvertError::Export`], or [`ConvertError::Panicked`].
pub fn jscontact_to_vcard(json: &str) -> Result<String, ConvertError> {
    let format = Format::JsContact;
    check_size(json.len(), format)?;
    guarded(format, || {
        let js = ContactObject::parse(json).map_err(|_| ConvertError::Invalid { format })?;
        let card = js
            .into_vcard()
            .map_err(|_| ConvertError::Export { format })?;
        check_vcard_names(&card)?;
        write_text(&card, format)
    })
}

/// A vCard version the bridge writes (RFC 2426 or RFC 6350).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VcardVersion {
    /// vCard 3.0 (Apple, older servers).
    V3,
    /// vCard 4.0.
    V4,
}

impl VcardVersion {
    const fn calcard(self) -> VCardVersion {
        match self {
            Self::V3 => VCardVersion::V3_0,
            Self::V4 => VCardVersion::V4_0,
        }
    }
}

/// How a vCard is written: its version. A 3.0 card always follows RFC
/// 2426 ([`to_rfc2426`]): groups the Apple way, never 4.0-only `KIND`,
/// `MEMBER` or `DERIVED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VcardForm {
    version: VcardVersion,
}

impl VcardForm {
    /// A card of `version`.
    #[must_use]
    pub const fn standard(version: VcardVersion) -> Self {
        Self { version }
    }

    /// The version.
    #[must_use]
    pub const fn version(self) -> VcardVersion {
        self.version
    }
}

/// What a 3.0 card gets before it is written, so that it is RFC 2426
/// (the JSContact conversion is RFC 9555's, for 4.0):
///
/// - `KIND` and `MEMBER` (RFC 6350 §6.1.4, §6.6.5: 4.0 only) become
///   Apple's `X-ADDRESSBOOKSERVER-KIND`/`-MEMBER`, an individual nothing
///   ([`apple_groups::write`]). RFC 2426 has no groups; these X- lines
///   (§3.8 allows them) are what iCloud, macOS/iOS and sabre/dav
///   (Nextcloud, Baïkal: it turns a 4.0 group into them when it serves
///   3.0) read and write, and the bridge reads them back as a group.
/// - `DERIVED` (RFC 9554 §4.4, a 4.0 extension) goes.
/// - `N`, which RFC 2426 §3.1.2 requires ("MUST be present"), empty if
///   the card has none (`FN` is always there: calcard writes it).
fn to_rfc2426(card: &mut VCard) {
    apple_groups::write(card);
    for entry in &mut card.entries {
        entry
            .params
            .retain(|param| param.name != VCardParameterName::Derived);
    }
    if !card
        .entries
        .iter()
        .any(|entry| entry.name == VCardProperty::N)
    {
        card.entries.push(VCardEntry {
            group: None,
            name: VCardProperty::N,
            params: Vec::new(),
            values: std::iter::repeat_with(|| VCardValue::Text(String::new()))
                .take(5)
                .collect(),
        });
    }
}

/// JSContact JSON → vCard text of `version`: what a CardDAV PUT sends.
/// Unconverted vCard properties (`vCard.properties`, calcard's RFC 9555
/// leftovers: `X-ABUID`, `item1.X-ABLabel`…) are written back as they
/// were, so a card from another client keeps what the JMAP side does not
/// know.
///
/// # Errors
///
/// As [`jscontact_to_vcard`].
pub fn jscontact_to_vcard_as(json: &str, version: VcardVersion) -> Result<String, ConvertError> {
    jscontact_to_vcard_in(json, VcardForm::standard(version))
}

/// [`jscontact_to_vcard_as`] in `form`: an edit of a stored card is
/// written in the stored card's [`vcard_form`].
///
/// # Errors
///
/// As [`jscontact_to_vcard`].
pub fn jscontact_to_vcard_in(json: &str, form: VcardForm) -> Result<String, ConvertError> {
    let format = Format::JsContact;
    check_size(json.len(), format)?;
    guarded(format, || {
        let js = ContactObject::parse(json).map_err(|_| ConvertError::Invalid { format })?;
        let mut card = js
            .into_vcard()
            .map_err(|_| ConvertError::Export { format })?;
        check_vcard_names(&card)?;
        if form.version == VcardVersion::V3 {
            to_rfc2426(&mut card);
        }
        let mut text = String::new();
        card.write_to(&mut text, form.version.calcard())
            .map_err(|_| ConvertError::Export { format })?;
        // Defense in depth: calcard refuses names that would start a new
        // line, but a second `BEGIN:VCARD` would turn one PUT into two
        // cards on servers that accept that.
        let begins = text
            .split("\r\n")
            .filter(|line| line.eq_ignore_ascii_case("BEGIN:VCARD"))
            .count();
        if begins != 1 {
            return Err(ConvertError::Export { format });
        }
        Ok(text)
    })
}

/// The `VERSION` of a vCard, if it is 3.0 or 4.0 (`None` for 2.x, none,
/// or not a vCard).
#[must_use]
pub fn vcard_version(input: &[u8]) -> Option<VcardVersion> {
    vcard_form(input).map(VcardForm::version)
}

/// How a stored vCard is written back: its version, if 3.0 or 4.0
/// (`None` for 2.x, no version, or not a vCard).
#[must_use]
pub fn vcard_form(input: &[u8]) -> Option<VcardForm> {
    let format = Format::VCard;
    let text = checked_text(input, format).ok()?;
    guarded(format, || {
        let card = VCard::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        Ok(match card.version() {
            Some(VCardVersion::V3_0) => Some(VcardForm::standard(VcardVersion::V3)),
            Some(VCardVersion::V4_0) => Some(VcardForm::standard(VcardVersion::V4)),
            _ => None,
        })
    })
    .ok()
    .flatten()
}

/// Expands the recurrences of an iCalendar object (RRULE, RDATE, EXDATE and
/// overrides by RECURRENCE-ID), at most `limit` occurrences (and never more
/// than [`MAX_OCCURRENCES`]). Floating times are read in `UTC`, sorted by
/// start.
///
/// # Errors
///
/// As [`icalendar_to_jscalendar`].
pub fn expand_icalendar(input: &[u8], limit: usize) -> Result<Expansion, ConvertError> {
    let format = Format::ICalendar;
    let text = checked_text(input, format)?;
    let limit = limit.min(MAX_OCCURRENCES);
    guarded(format, || {
        let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        let expanded = ical.expand_dates(Tz::UTC, limit);
        let mut occurrences: Vec<Occurrence> = expanded
            .events
            .iter()
            .map(|event| Occurrence {
                utc_start: event.start.timestamp(),
                utc_end: event.end.timestamp(),
                component: event.comp_id,
            })
            .collect();
        occurrences.sort_unstable();
        // calcard says when a rule stopped at `limit`; overrides and RDATEs
        // come on top of it, and the cut below drops what passes `limit`.
        let complete =
            expanded.errors.is_empty() && !expanded.truncated && occurrences.len() <= limit;
        occurrences.truncate(limit);
        Ok(Expansion {
            occurrences,
            complete,
        })
    })
}

/// The clear-text indexes of an iCalendar object
/// in seconds since the Unix epoch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventIndex {
    /// The `UID` of its components.
    pub uid: Option<String>,
    /// Earliest start of any occurrence; `None` if nothing expands.
    pub utc_start: Option<i64>,
    /// Latest end of any occurrence; `None` when open-ended: an endless
    /// rule, or more occurrences than `max_occurrences`, or a component
    /// that could not be expanded.
    pub utc_end: Option<i64>,
    /// Has `RRULE`, `RDATE` or `RECURRENCE-ID` overrides.
    pub recurring: bool,
}

/// The clear-text indexes of a vCard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardIndex {
    /// Its `UID`.
    pub uid: Option<String>,
    /// What to sort by: family and given names from `N`, else `FN`; lower
    /// case, spaces collapsed, at most [`MAX_SORT_NAME`] characters.
    pub sort_name: Option<String>,
}

/// Longest `UID` kept as an index (a longer one is cut; the full one is
/// still in the sealed text).
pub const MAX_UID: usize = 1024;
/// Longest sort name, in characters.
pub const MAX_SORT_NAME: usize = 256;

/// Computes the indexes of an iCalendar object, expanding at most
/// `max_occurrences` occurrences (and never more than
/// [`MAX_OCCURRENCES`]). Floating times count as UTC, so `utc_start` and
/// `utc_end` may be off by the user's offset (up to 14 h): a query must
/// widen its range by that much.
///
/// # Errors
///
/// As [`icalendar_to_jscalendar`]; also [`ConvertError::Invalid`] when
/// there is no event, task or journal in it.
pub fn icalendar_index(input: &[u8], max_occurrences: usize) -> Result<EventIndex, ConvertError> {
    let format = Format::ICalendar;
    let text = checked_text(input, format)?;
    let limit = max_occurrences.clamp(1, MAX_OCCURRENCES);
    guarded(format, || {
        let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        let objects: Vec<_> = ical
            .components
            .iter()
            .filter(|component| component.component_type.is_calendar_object())
            .collect();
        if objects.is_empty() {
            return Err(ConvertError::Invalid { format });
        }
        let uid = ical.uids().next().map(|uid| cut(uid, MAX_UID));
        let recurring = objects
            .iter()
            .any(|component| component.is_recurrent_or_override());
        let expanded = ical.expand_dates(Tz::UTC, limit);
        // A task without DTSTART, DUE or RECURRENCE-ID has no date
        // (calcard would place it at its CREATED or COMPLETED).
        let placed = || {
            expanded.events.iter().filter(|event| {
                usize::try_from(event.comp_id)
                    .ok()
                    .and_then(|index| ical.components.get(index))
                    .is_some_and(super::calendar::is_placed)
            })
        };
        let starts = placed().map(|event| event.start.timestamp());
        let ends = placed().map(|event| event.end.timestamp());
        let utc_start = starts.min();
        // calcard stops each rule at `limit` (counting excluded dates too):
        // reaching it means "maybe more", so the end is open.
        let complete = expanded.errors.is_empty() && !expanded.truncated;
        let utc_end = if complete { ends.max() } else { None };
        Ok(EventIndex {
            uid,
            utc_start,
            utc_end,
            recurring,
        })
    })
}

/// Computes the indexes of a vCard.
///
/// # Errors
///
/// As [`vcard_to_jscontact`].
pub fn vcard_index(input: &[u8]) -> Result<CardIndex, ConvertError> {
    let format = Format::VCard;
    let text = checked_text(input, format)?;
    guarded(format, || {
        let card = VCard::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        let uid = card.uid().map(|uid| cut(uid, MAX_UID));
        let structured = card.property(&VCardProperty::N).map(|entry| {
            let part = |index: usize| entry.values.get(index).map(card_text).unwrap_or_default();
            format!("{} {}", part(0), part(1))
        });
        let formatted = card
            .property(&VCardProperty::Fn)
            .and_then(|entry| entry.values.first())
            .map(card_text);
        let sort_name = [structured, formatted]
            .into_iter()
            .flatten()
            .map(|name| sort_key(&name))
            .find(|name| !name.is_empty());
        Ok(CardIndex { uid, sort_name })
    })
}

/// The `TZID` of a `calendar-timezone` property (a `VCALENDAR` with one
/// `VTIMEZONE`), if it has one.
#[must_use]
pub fn timezone_id(calendar_timezone: &str) -> Option<String> {
    let format = Format::ICalendar;
    let text = checked_text(calendar_timezone.as_bytes(), format).ok()?;
    guarded(format, || {
        let ical = ICalendar::parse(text).map_err(|_| ConvertError::Invalid { format })?;
        Ok(ical
            .components
            .iter()
            .filter(|c| c.component_type == ICalendarComponentType::VTimezone)
            .find_map(|c| {
                c.property(&ICalendarProperty::Tzid)
                    .and_then(|entry| entry.values.first())
                    .and_then(|value| value.as_text())
                    .map(|tzid| cut(tzid, MAX_SORT_NAME))
            }))
    })
    .ok()
    .flatten()
}

fn card_text(value: &VCardValue) -> String {
    match value {
        VCardValue::Component(parts) => parts.join(" "),
        other => other.as_text().unwrap_or_default().to_owned(),
    }
}

fn sort_key(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    cut(&words.join(" ").to_lowercase(), MAX_SORT_NAME)
}

/// At most `max` characters of `text`.
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn check_size(size: usize, format: Format) -> Result<(), ConvertError> {
    let limit = format.limit();
    if size > limit {
        return Err(ConvertError::TooLarge {
            format,
            size,
            limit,
        });
    }
    Ok(())
}

pub(crate) fn checked_text(input: &[u8], format: Format) -> Result<&str, ConvertError> {
    check_size(input.len(), format)?;
    std::str::from_utf8(input).map_err(|_| ConvertError::NotUtf8 { format })
}

/// `value` (an `ICalendar` or a `VCard`) as text. calcard's writer fails
/// (instead of escaping) on what it can't write, such as a property name
/// with a line break taken from JSON (`vendor/calcard/PATCHES.md`);
/// `to_string()` would panic on that, this gives [`ConvertError::Export`].
pub(crate) fn write_text(
    value: &impl fmt::Display,
    format: Format,
) -> Result<String, ConvertError> {
    use fmt::Write as _;
    let mut text = String::new();
    write!(text, "{value}").map_err(|_| ConvertError::Export { format })?;
    Ok(text)
}

/// Removes every `METHOD` (RFC 5545 §3.7.2): RFC 4791 §4.1 says a
/// calendar object resource MUST NOT carry one (it belongs to iTIP
/// messages, RFC 5546 §1.4).
pub(crate) fn strip_method(ical: &mut ICalendar) {
    for component in &mut ical.components {
        component
            .entries
            .retain(|entry| entry.name != ICalendarProperty::Method);
    }
}

/// Runs calcard, turning a panic into [`ConvertError::Panicked`]. calcard
/// keeps no state between calls (every value is built inside `f`), so
/// nothing half-updated survives the unwind: `AssertUnwindSafe` is sound.
pub(crate) fn guarded<T>(
    format: Format,
    f: impl FnOnce() -> Result<T, ConvertError>,
) -> Result<T, ConvertError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(Err(ConvertError::Panicked { format }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vCard 3.0 from Apple keeps its version and the properties JMAP
    /// does not know through JSContact; a group can be written as 3.0.
    #[test]
    fn vcard_versions_round_trip() {
        let apple = "BEGIN:VCARD\r\nVERSION:3.0\r\nPRODID:-//Apple Inc.//iPhone OS 17.0//EN\r\n\
                     N:Pérez;Ana;;;\r\nFN:Ana Pérez\r\n\
                     item1.EMAIL;type=INTERNET;type=pref:ana@example.com\r\n\
                     item1.X-ABLabel:_$!<Other>!$_\r\nX-ABUID:1234-ABCD\r\n\
                     UID:apple-1\r\nEND:VCARD\r\n";
        assert_eq!(vcard_version(apple.as_bytes()), Some(VcardVersion::V3));
        let mut json = vcard_to_jscontact(apple.as_bytes()).unwrap();
        json["name"]["full"] = Value::from("Ana P. Pérez");
        let back = jscontact_to_vcard_as(&json.to_string(), VcardVersion::V3).unwrap();
        assert!(back.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n"), "{back}");
        for kept in [
            "X-ABUID:1234-ABCD",
            "item1.X-ABLABEL:_$!<Other>!$_",
            "item1.EMAIL;TYPE=INTERNET,PREF:ana@example.com",
            "PRODID:-//Apple Inc.//iPhone OS 17.0//EN",
            "UID:apple-1",
        ] {
            assert!(back.contains(kept), "{kept} missing in {back}");
        }
        assert!(back.contains("Ana P. Pérez"));
        assert_eq!(vcard_version(back.as_bytes()), Some(VcardVersion::V3));

        let group = r#"{"@type":"Card","version":"1.0","kind":"group","uid":"urn:uuid:g",
            "name":{"full":"Equipo"},"members":{"urn:uuid:a":true,"urn:uuid:b":true}}"#;
        for version in [VcardVersion::V3, VcardVersion::V4] {
            let text = jscontact_to_vcard_as(group, version).unwrap();
            assert_eq!(vcard_version(text.as_bytes()), Some(version));
            let again = vcard_to_jscontact(text.as_bytes()).unwrap();
            assert_eq!(again["kind"], "group");
            assert_eq!(again["members"]["urn:uuid:a"], true);
            assert_eq!(again["members"]["urn:uuid:b"], true);
        }
        assert_eq!(
            vcard_version(b"BEGIN:VCARD\r\nVERSION:2.1\r\nEND:VCARD\r\n"),
            None
        );
        assert_eq!(vcard_version(b"not a card"), None);
    }

    /// A 3.0 card is RFC 2426: no `KIND`/`MEMBER` (Apple's X- lines for
    /// a group), no `DERIVED`, no `CHARSET`, always an `N`; read back the
    /// same. 4.0 keeps `KIND`, `MEMBER` and `DERIVED` (RFC 6350, RFC 9554).
    #[test]
    fn vcard_30_is_rfc_2426() {
        let person = r#"{"@type":"Card","version":"1.0","kind":"individual","uid":"urn:uuid:p",
            "name":{"components":[{"kind":"given","value":"Ánxela"}]}}"#;
        let text = jscontact_to_vcard_as(person, VcardVersion::V3).unwrap();
        for absent in ["KIND", "DERIVED", "CHARSET", "MEMBER"] {
            assert!(!text.contains(absent), "{absent}:\n{text}");
        }
        assert!(text.contains("FN:Ánxela\r\n"), "{text}");
        assert!(text.contains("N:;Ánxela;;;\r\n"), "{text}");
        let four = jscontact_to_vcard_as(person, VcardVersion::V4).unwrap();
        assert!(
            four.contains("KIND:INDIVIDUAL") && four.contains("FN;DERIVED=TRUE:Ánxela"),
            "{four}"
        );

        let nameless = r#"{"@type":"Card","version":"1.0","uid":"urn:uuid:n",
            "emails":{"e":{"address":"n@example.org"}}}"#;
        let text = jscontact_to_vcard_as(nameless, VcardVersion::V3).unwrap();
        assert!(
            text.contains("\r\nN:;;;;\r\n") && text.contains("\r\nFN:"),
            "{text}"
        );

        let group = r#"{"@type":"Card","version":"1.0","kind":"group","uid":"urn:uuid:g",
            "name":{"full":"Equipo"},"members":{"urn:uuid:a":true}}"#;
        let text = jscontact_to_vcard_as(group, VcardVersion::V3).unwrap();
        assert!(
            text.contains("X-ADDRESSBOOKSERVER-KIND:group\r\n"),
            "{text}"
        );
        assert!(
            text.contains("X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:a\r\n"),
            "{text}"
        );
        assert!(
            !text.contains("\r\nKIND") && !text.contains("\r\nMEMBER"),
            "{text}"
        );
        let again = vcard_to_jscontact(text.as_bytes()).unwrap();
        assert_eq!(again["kind"], "group");
        assert_eq!(again["members"]["urn:uuid:a"], true);

        // A 3.0 card with KIND and Apple's lines: KIND won on reading, so
        // the X- lines it ignored are not written next to the new ones.
        let both = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:g\r\nFN:G\r\nN:G;;;;\r\nKIND:group\r\n\
            MEMBER:urn:uuid:b\r\nX-ADDRESSBOOKSERVER-KIND:individual\r\n\
            X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:old\r\nEND:VCARD\r\n";
        let json = vcard_to_jscontact(both.as_bytes()).unwrap();
        let text = jscontact_to_vcard_as(&json.to_string(), VcardVersion::V3).unwrap();
        assert_eq!(
            text.matches("X-ADDRESSBOOKSERVER-KIND").count(),
            1,
            "{text}"
        );
        assert!(text.contains("X-ADDRESSBOOKSERVER-KIND:group"), "{text}");
        assert!(
            text.contains("X-ADDRESSBOOKSERVER-MEMBER:urn:uuid:b"),
            "{text}"
        );
        assert!(!text.contains("urn:uuid:old"), "{text}");
    }

    /// `vCard.properties` names (and groups) are written as they are: one
    /// that breaks the line is refused; Apple's `item1.X-ABLabel` and
    /// names with `_` stay.
    #[test]
    fn vcard_names_that_break_the_format_are_refused() {
        let card = |name: &str| {
            serde_json::json!({"@type": "Card", "version": "1.0", "uid": "u1",
                "name": {"full": "Ana"},
                "vCard": {"properties": [[name, {}, "text", "z"]]}})
            .to_string()
        };
        for bad in ["x-a\r\nEND:VCARD", "x-a:b", "x-a;b", "x a", "grp.x-a.b"] {
            for version in [VcardVersion::V3, VcardVersion::V4] {
                assert!(
                    matches!(
                        jscontact_to_vcard_as(&card(bad), version),
                        Err(ConvertError::Export { .. })
                    ),
                    "{bad:?}"
                );
            }
            assert!(jscontact_to_vcard(&card(bad)).is_err(), "{bad:?}");
        }
        let text = jscontact_to_vcard_as(&card("x-foo_bar"), VcardVersion::V4).unwrap();
        assert!(text.contains("X-FOO_BAR:z"), "{text}");
        let mut entry =
            VCard::parse("BEGIN:VCARD\r\nVERSION:4.0\r\nFN:A\r\nEND:VCARD\r\n").unwrap();
        entry.entries[1].group = Some("item1\r\nX-EVIL".to_owned());
        assert!(check_vcard_names(&entry).is_err());
        entry.entries[1].group = Some("item1".to_owned());
        assert!(check_vcard_names(&entry).is_ok());
    }

    #[test]
    fn a_panic_becomes_an_error() {
        let result: Result<(), _> = guarded(Format::VCard, || panic!("boom"));
        assert_eq!(
            result,
            Err(ConvertError::Panicked {
                format: Format::VCard
            })
        );
    }

    #[test]
    fn size_and_utf8_are_checked_first() {
        let big = vec![b'A'; MAX_ICALENDAR_BYTES + 1];
        assert!(matches!(
            icalendar_to_jscalendar(&big),
            Err(ConvertError::TooLarge {
                format: Format::ICalendar,
                ..
            })
        ));
        assert_eq!(
            vcard_to_jscontact(b"BEGIN:VCARD\r\nFN:\xff\r\nEND:VCARD\r\n"),
            Err(ConvertError::NotUtf8 {
                format: Format::VCard
            })
        );
        assert_eq!(
            icalendar_to_jscalendar(b"BEGIN:VCARD\r\nEND:VCARD\r\n"),
            Err(ConvertError::Invalid {
                format: Format::ICalendar
            })
        );
        assert_eq!(
            jscalendar_to_icalendar("{not json"),
            Err(ConvertError::Invalid {
                format: Format::JsCalendar
            })
        );
    }
}
