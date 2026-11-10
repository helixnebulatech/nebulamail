// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP ids of calendars, events, occurrences and participant identities.
//!
//! - Calendar: `c` and the collection row (`c3`).
//! - Event: `v` and the resource row (`v17`).
//! - Occurrence (`expandRecurrences`, only of a series, as Stalwart 1.0):
//!   the event's id, `_` and its original start as a wall-clock reading in
//!   the event's zone (`v17_20261019T100000`). Only `[A-Za-z0-9_]`, and
//!   stable: the key is the `RECURRENCE-ID`, which does not change when
//!   the occurrence is moved.
//! - Participant identity: `p` and the hash of its address
//!   (`nebula_groupware::model::calendar::participant_number`).
//!
//! Parsing is strict (only the form we write), so each id names one thing.

use nebula_groupware::model::calendar::RecurrenceKey;
use nebula_jmap_proto::{CalendarEventId, CalendarId, ParticipantIdentityId};
use nebula_store::{CollectionId, ResourceId};
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

use crate::core::ids::parse_number;
use crate::error::Bug;

/// What a `CalendarEvent` id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventRef {
    /// The stored event (a series, or an event without repetition).
    Base(ResourceId),
    /// One occurrence of it.
    Occurrence(ResourceId, RecurrenceKey),
}

impl EventRef {
    pub(crate) const fn resource(self) -> ResourceId {
        match self {
            Self::Base(resource) | Self::Occurrence(resource, _) => resource,
        }
    }
}

fn parse<T: std::str::FromStr>(text: &str) -> Result<T, Bug> {
    text.parse().map_err(|_| Bug("id"))
}

pub(crate) fn calendar_id(collection: CollectionId) -> Result<CalendarId, Bug> {
    parse(&format!("c{}", collection.get()))
}

/// A change-log row id as an event id.
pub(crate) fn event_row(raw: i64) -> Result<CalendarEventId, Bug> {
    ResourceId::from_raw(raw)
        .ok_or(Bug("event row"))
        .and_then(event_id)
}

pub(crate) fn store_calendar(id: &CalendarId) -> Option<CollectionId> {
    parse_number(id.as_str(), 'c').and_then(CollectionId::from_raw)
}

pub(crate) fn event_id(resource: ResourceId) -> Result<CalendarEventId, Bug> {
    parse(&format!("v{}", resource.get()))
}

/// `YYYYMMDDTHHMMSS` of a wall-clock reading (years 1 to 9999).
fn key_text(key: RecurrenceKey) -> Option<String> {
    let at = OffsetDateTime::from_unix_timestamp(key.naive()).ok()?;
    let year = at.year();
    if !(1..=9999).contains(&year) {
        return None;
    }
    Some(format!(
        "{year:04}{:02}{:02}T{:02}{:02}{:02}",
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    ))
}

fn key_from_text(text: &str) -> Option<RecurrenceKey> {
    let bytes = text.as_bytes();
    let (date, time) = text.split_once('T')?;
    if bytes.len() != 15
        || date.len() != 8
        || !date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let number = |part: &str| part.parse::<u16>().ok();
    let (year, rest) = date.split_at_checked(4)?;
    let (month, day) = rest.split_at_checked(2)?;
    let (hour, rest) = time.split_at_checked(2)?;
    let (minute, second) = rest.split_at_checked(2)?;
    let date = Date::from_calendar_date(
        i32::from(number(year)?),
        Month::try_from(u8::try_from(number(month)?).ok()?).ok()?,
        u8::try_from(number(day)?).ok()?,
    )
    .ok()?;
    let time = Time::from_hms(
        u8::try_from(number(hour)?).ok()?,
        u8::try_from(number(minute)?).ok()?,
        u8::try_from(number(second)?).ok()?,
    )
    .ok()?;
    let key = RecurrenceKey::from_naive(
        PrimitiveDateTime::new(date, time)
            .assume_utc()
            .unix_timestamp(),
    )?;
    // Only the canonical spelling.
    (key_text(key)? == text).then_some(key)
}

pub(crate) fn occurrence_id(
    resource: ResourceId,
    key: RecurrenceKey,
) -> Result<CalendarEventId, Bug> {
    let text = key_text(key).ok_or(Bug("occurrence date"))?;
    parse(&format!("v{}_{text}", resource.get()))
}

pub(crate) fn parse_event(id: &CalendarEventId) -> Option<EventRef> {
    let text = id.as_str();
    match text.split_once('_') {
        None => parse_number(text, 'v')
            .and_then(ResourceId::from_raw)
            .map(EventRef::Base),
        Some((base, key)) => {
            let resource = parse_number(base, 'v').and_then(ResourceId::from_raw)?;
            Some(EventRef::Occurrence(resource, key_from_text(key)?))
        }
    }
}

pub(crate) fn identity_id(number: i64) -> Result<ParticipantIdentityId, Bug> {
    parse(&format!("p{number}"))
}

pub(crate) fn identity_number(id: &ParticipantIdentityId) -> Option<i64> {
    parse_number(id.as_str(), 'p')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_occurrences_round_trip() {
        let resource = ResourceId::from_raw(17).unwrap();
        assert_eq!(event_id(resource).unwrap().as_str(), "v17");
        // 2026-10-19T10:00:00 as a wall-clock reading.
        let key = RecurrenceKey::from_naive(1_792_404_000).unwrap();
        let id = occurrence_id(resource, key).unwrap();
        assert_eq!(id.as_str(), "v17_20261019T100000");
        assert!(
            id.as_str()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        );
        assert_eq!(parse_event(&id), Some(EventRef::Occurrence(resource, key)));
        assert_eq!(
            parse_event(&"v17".parse().unwrap()),
            Some(EventRef::Base(resource))
        );
        let calendar = CollectionId::from_raw(3).unwrap();
        assert_eq!(calendar_id(calendar).unwrap().as_str(), "c3");
        assert_eq!(store_calendar(&"c3".parse().unwrap()), Some(calendar));
        assert_eq!(identity_id(42).unwrap().as_str(), "p42");
        assert_eq!(identity_number(&"p42".parse().unwrap()), Some(42));
    }

    /// Every key a [`RecurrenceKey`] can hold has an id: the first and the
    /// last ones too.
    #[test]
    fn keys_at_the_limits_have_ids() {
        let resource = ResourceId::from_raw(5).unwrap();
        for (key, text) in [
            (RecurrenceKey::MIN, "v5_00010101T000000"),
            (RecurrenceKey::MAX, "v5_99991231T235959"),
        ] {
            let id = occurrence_id(resource, key).unwrap();
            assert_eq!(id.as_str(), text);
            assert_eq!(parse_event(&id), Some(EventRef::Occurrence(resource, key)));
        }
    }

    #[test]
    fn strict_parsing() {
        for bad in [
            "v0",
            "v017",
            "c17",
            "v17_",
            "v17_2026101910000",
            "v17_20261019T1000000",
            "v17_20261319T100000",
            "v17_20261019T250000",
            "v17_20261019t100000",
            "v17_20261019T100000_1",
            "v_20261019T100000",
            "v17_00001019T100000",
        ] {
            assert_eq!(parse_event(&bad.parse().unwrap()), None, "{bad}");
        }
        assert_eq!(store_calendar(&"v3".parse().unwrap()), None);
    }
}
