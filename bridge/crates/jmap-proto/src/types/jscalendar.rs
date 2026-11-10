// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JSCalendar primitive types (RFC 8984 §1.4) that JMAP Calendars uses in
//! method arguments: `LocalDateTime`, `TimeZoneId` and `Duration`.

use std::fmt;
use std::str::FromStr;

use time::{Date as CalendarDate, Month, PrimitiveDateTime, Time};

use crate::macros::serde_via_str;

/// Why a string is not a valid JSCalendar value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum JsCalendarValueError {
    /// Not `YYYY-MM-DDTHH:MM:SS[.fraction]` without an offset.
    #[error("not a LocalDateTime (YYYY-MM-DDTHH:MM:SS, no offset)")]
    LocalDateTime,
    /// Not a time zone id.
    #[error("not a time zone id")]
    TimeZoneId,
    /// Not an RFC 8984 `Duration`.
    #[error("not a Duration (e.g. P1W2DT3H)")]
    Duration,
}

/// A `LocalDateTime` (RFC 8984 §1.4.4): a date and time without a time zone
/// or offset, e.g. `2026-10-26T10:00:00`, read in some time zone.
///
/// The `T` must be upper case, seconds are required and a zero fraction is
/// dropped when writing it back (`10:00:00.000` → `10:00:00`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalDateTime(PrimitiveDateTime);

impl LocalDateTime {
    /// Wraps a `time` value.
    #[must_use]
    pub fn new(value: PrimitiveDateTime) -> Option<Self> {
        (0..=9999).contains(&value.year()).then_some(Self(value))
    }

    /// The date and time as a `time` value.
    #[must_use]
    pub fn to_primitive(self) -> PrimitiveDateTime {
        self.0
    }
}

/// `n` ASCII digits at the start of `bytes`, as a number.
fn digits(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    bytes.iter().try_fold(0_u32, |acc, b| {
        acc.checked_mul(10)?
            .checked_add(u32::from(b.checked_sub(b'0')?))
    })
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction]`, or `None`.
fn parse_local(bytes: &[u8]) -> Option<PrimitiveDateTime> {
    let (main, fraction) = match bytes.split_at_checked(19)? {
        (main, []) => (main, None),
        (main, [b'.', rest @ ..]) if (1..=9).contains(&rest.len()) => (main, Some(rest)),
        _ => return None,
    };
    for (index, separator) in [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')] {
        if main.get(index) != Some(&separator) {
            return None;
        }
    }
    let field = |from: usize, to: usize| main.get(from..to).and_then(digits);
    let small = |from: usize| field(from, from.checked_add(2)?).and_then(|n| u8::try_from(n).ok());
    let nanos = match fraction {
        None => 0,
        Some(fraction) => {
            // Pad to nine digits: ".5" is 500 000 000 ns.
            let missing = 9_u32.checked_sub(u32::try_from(fraction.len()).ok()?)?;
            digits(fraction)?.checked_mul(10_u32.checked_pow(missing)?)?
        }
    };
    let date = CalendarDate::from_calendar_date(
        i32::try_from(field(0, 4)?).ok()?,
        Month::try_from(small(5)?).ok()?,
        small(8)?,
    )
    .ok()?;
    let time = Time::from_hms_nano(small(11)?, small(14)?, small(17)?, nanos).ok()?;
    Some(PrimitiveDateTime::new(date, time))
}

impl FromStr for LocalDateTime {
    type Err = JsCalendarValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_local(s.as_bytes())
            .map(Self)
            .ok_or(JsCalendarValueError::LocalDateTime)
    }
}

impl fmt::Display for LocalDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            value.year(),
            u8::from(value.month()),
            value.day(),
            value.hour(),
            value.minute(),
            value.second()
        )?;
        let nanos = value.nanosecond();
        if nanos == 0 {
            return Ok(());
        }
        let text = format!("{nanos:09}");
        write!(f, ".{}", text.trim_end_matches('0'))
    }
}

impl fmt::Debug for LocalDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LocalDateTime({self})")
    }
}

serde_via_str!(LocalDateTime);

/// A `TimeZoneId` (RFC 8984 §1.4.8): an IANA name (`Europe/Madrid`,
/// `Etc/GMT+5`, `EST5EDT`) or a custom id that starts with `/`.
///
/// Checked by shape only (1 to 64 characters from `A-Z a-z 0-9 / _ - +`,
/// no empty segments, no dots): whether the zone exists is the
/// implementation's business, which answers `invalidArguments` then.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeZoneId(String);

impl TimeZoneId {
    /// Longest id accepted. The longest IANA name has 32 characters.
    pub const MAX_LEN: usize = 64;

    /// `Etc/UTC`, the default `timeZone` of the calendar methods.
    #[must_use]
    pub fn utc() -> Self {
        Self("Etc/UTC".to_owned())
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for TimeZoneId {
    type Err = JsCalendarValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s.strip_prefix('/').unwrap_or(s);
        let valid = (1..=Self::MAX_LEN).contains(&s.len())
            && body.split('/').all(|segment| {
                !segment.is_empty()
                    && segment
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+'))
            });
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(JsCalendarValueError::TimeZoneId)
        }
    }
}

impl fmt::Display for TimeZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for TimeZoneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TimeZoneId({})", self.0)
    }
}

serde_via_str!(TimeZoneId);

/// A `Duration` (RFC 8984 §1.4.6), e.g. `PT1H`, `P1D`, `P52W1D`.
///
/// Kept as written; [`Duration::seconds`] gives its length counting a day
/// as 24 hours (what the limits of the calendar capability need).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Duration {
    text: String,
    seconds: u64,
}

impl Duration {
    /// Seconds, with a day of 86 400 s and a week of 7 days (fractions of
    /// a second are dropped).
    #[must_use]
    pub fn seconds(&self) -> u64 {
        self.seconds
    }

    /// The duration as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// A number with its unit.
type Unit<'a> = (Option<u64>, &'a [u8]);

/// Reads `<digits><unit>` at the start of `rest` if the unit matches
/// (`Some((None, rest))` if it doesn't; `None` if the number is too long).
fn take_unit(rest: &[u8], unit: u8) -> Option<Unit<'_>> {
    let count = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    match rest.get(count) {
        Some(found) if *found == unit && count > 0 => {
            // At most 9 digits: no overflow below, and no real duration
            // needs more.
            if count > 9 {
                return None;
            }
            let (number, tail) = rest.split_at(count);
            let value = digits(number)?;
            Some((Some(u64::from(value)), tail.get(1..).unwrap_or_default()))
        }
        _ => Some((None, rest)),
    }
}

fn parse_duration(s: &str) -> Option<u64> {
    let rest = s.as_bytes().strip_prefix(b"P")?;
    let (weeks, rest) = take_unit(rest, b'W')?;
    let (days, rest) = take_unit(rest, b'D')?;
    let mut total = weeks
        .unwrap_or(0)
        .checked_mul(7)?
        .checked_add(days.unwrap_or(0))?
        .checked_mul(86_400)?;
    let has_date = weeks.is_some() || days.is_some();
    let Some(time) = rest.strip_prefix(b"T") else {
        return (rest.is_empty() && has_date).then_some(total);
    };
    let (hours, time) = take_unit(time, b'H')?;
    let (minutes, time) = take_unit(time, b'M')?;
    // Seconds may have a fraction: `1.5S`.
    let count = time.iter().take_while(|b| b.is_ascii_digit()).count();
    let (seconds, time) = if count == 0 {
        (None, time)
    } else {
        let (whole, tail) = time.split_at(count);
        let tail = match tail.strip_prefix(b".") {
            Some(fraction) => {
                let n = fraction.iter().take_while(|b| b.is_ascii_digit()).count();
                if n == 0 || n > 9 {
                    return None;
                }
                fraction.get(n..)?
            }
            None => tail,
        };
        let tail = tail.strip_prefix(b"S")?;
        if count > 9 {
            return None;
        }
        (Some(u64::from(digits(whole)?)), tail)
    };
    if !time.is_empty() || (hours.is_none() && minutes.is_none() && seconds.is_none()) {
        return None;
    }
    total = total
        .checked_add(hours.unwrap_or(0).checked_mul(3600)?)?
        .checked_add(minutes.unwrap_or(0).checked_mul(60)?)?
        .checked_add(seconds.unwrap_or(0))?;
    Some(total)
}

impl FromStr for Duration {
    type Err = JsCalendarValueError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_duration(s)
            .map(|seconds| Self {
                text: s.to_owned(),
                seconds,
            })
            .ok_or(JsCalendarValueError::Duration)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Duration({})", self.text)
    }
}

serde_via_str!(Duration);

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn local_date_time() {
        for (text, back) in [
            ("2026-10-26T10:00:00", "2026-10-26T10:00:00"),
            ("2026-10-19T00:00:00", "2026-10-19T00:00:00"),
            ("2024-02-29T23:59:59.500", "2024-02-29T23:59:59.5"),
            ("0001-01-01T00:00:00.000", "0001-01-01T00:00:00"),
        ] {
            let parsed: LocalDateTime = text.parse().unwrap();
            assert_eq!(parsed.to_string(), back);
        }
        for bad in [
            "",
            "2026-10-26",
            "2026-10-26T10:00",
            "2026-10-26T10:00:00Z",
            "2026-10-26T10:00:00+02:00",
            "2026-10-26t10:00:00",
            "2026-10-26 10:00:00",
            "2026-13-01T00:00:00",
            "2025-02-29T00:00:00",
            "2026-10-26T24:00:00",
            "2026-10-26T10:00:60",
            "2026-10-26T10:00:00.",
            "2026-10-26T10:00:00.1234567890",
            "+026-10-26T10:00:00",
            "２０２６-10-26T10:00:00",
        ] {
            assert!(bad.parse::<LocalDateTime>().is_err(), "{bad:?}");
        }
        let a: LocalDateTime = "2026-10-19T00:00:00".parse().unwrap();
        let b: LocalDateTime = "2026-11-03T00:00:00".parse().unwrap();
        assert!(a < b);
    }

    #[test]
    fn time_zone_id() {
        for good in [
            "Europe/Madrid",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+5",
            "Etc/UTC",
            "EST5EDT",
            "/custom/zone-1",
        ] {
            assert_eq!(good.parse::<TimeZoneId>().unwrap().as_str(), good);
        }
        for bad in [
            "",
            "/",
            "Europe//Madrid",
            "Europe/",
            "../etc/passwd",
            "Europe/Ma drid",
            "Europe/Madrid\n",
            &"A".repeat(65),
        ] {
            assert!(bad.parse::<TimeZoneId>().is_err(), "{bad:?}");
        }
        assert_eq!(TimeZoneId::utc().as_str(), "Etc/UTC");
    }

    #[test]
    fn duration() {
        for (text, seconds) in [
            ("P52W1D", 365 * 86_400),
            ("PT1H", 3600),
            ("PT45M", 2700),
            ("P2D", 2 * 86_400),
            ("PT1H30M", 5400),
            ("P1DT1S", 86_401),
            ("PT1.5S", 1),
            ("P1W", 7 * 86_400),
        ] {
            let parsed: Duration = text.parse().unwrap();
            assert_eq!(parsed.seconds(), seconds, "{text}");
            assert_eq!(parsed.to_string(), text);
        }
        for bad in [
            "",
            "P",
            "PT",
            "1D",
            "P1H",
            "PT1D",
            "P1D1W",
            "-PT1H",
            "P1.5D",
            "PT1.S",
            "PT1M1H",
            "P1DT",
            "p1d",
            "P9999999999D",
        ] {
            assert!(bad.parse::<Duration>().is_err(), "{bad:?}");
        }
    }

    proptest! {
        #[test]
        fn local_date_time_round_trips(text in "[0-9]{4}-[01][0-9]-[0-3][0-9]T[0-2][0-9]:[0-5][0-9]:[0-5][0-9]") {
            if let Ok(parsed) = text.parse::<LocalDateTime>() {
                prop_assert_eq!(parsed.to_string(), text);
            }
        }

        #[test]
        fn parsers_never_panic(text in "\\PC{0,40}") {
            let _ = text.parse::<LocalDateTime>();
            let _ = text.parse::<TimeZoneId>();
            let _ = text.parse::<Duration>();
        }
    }
}
