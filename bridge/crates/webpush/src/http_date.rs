// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP dates (RFC 9110 §5.6.7), for `Retry-After` (§10.2.3). A
//! recipient MUST accept the three formats:
//!
//! - IMF-fixdate: `Sun, 06 Nov 1994 08:49:37 GMT` (the one senders use),
//! - rfc850-date: `Sunday, 06-Nov-94 08:49:37 GMT` (obsolete),
//! - asctime-date: `Sun Nov  6 08:49:37 1994` (obsolete).

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time};

const MONTHS: [(&str, Month); 12] = [
    ("Jan", Month::January),
    ("Feb", Month::February),
    ("Mar", Month::March),
    ("Apr", Month::April),
    ("May", Month::May),
    ("Jun", Month::June),
    ("Jul", Month::July),
    ("Aug", Month::August),
    ("Sep", Month::September),
    ("Oct", Month::October),
    ("Nov", Month::November),
    ("Dec", Month::December),
];

const SHORT_DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const LONG_DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

fn month(name: &str) -> Option<Month> {
    MONTHS
        .iter()
        .find(|(short, _)| *short == name)
        .map(|(_, month)| *month)
}

fn number<T: std::str::FromStr>(digits: &str, len: usize) -> Option<T> {
    (digits.len() == len && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// `HH:MM:SS` (`time-of-day`).
fn time_of_day(text: &str) -> Option<Time> {
    let mut parts = text.split(':');
    let hour = number(parts.next()?, 2)?;
    let minute = number(parts.next()?, 2)?;
    // `second` may be 60 (leap second, §5.6.7); `time` has none: 59.
    let second: u8 = number(parts.next()?, 2)?;
    if parts.next().is_some() {
        return None;
    }
    Time::from_hms(hour, minute, second.min(59)).ok()
}

fn at(date: Date, time: Time) -> i64 {
    PrimitiveDateTime::new(date, time)
        .assume_utc()
        .unix_timestamp()
}

/// `06 Nov 1994 08:49:37 GMT`, after `Sun, `.
fn imf_fixdate(rest: &str) -> Option<i64> {
    let [day, mon, year, time, "GMT"] = words(rest)? else {
        return None;
    };
    let date = Date::from_calendar_date(number(year, 4)?, month(mon)?, number(day, 2)?).ok()?;
    Some(at(date, time_of_day(time)?))
}

/// `06-Nov-94 08:49:37 GMT`, after `Sunday, `. A two-digit year more than
/// 50 years in the future is the most recent past year with those digits
/// (§5.6.7).
fn rfc850_date(rest: &str, now: OffsetDateTime) -> Option<i64> {
    let [date, time, "GMT"] = words(rest)? else {
        return None;
    };
    let mut parts = date.split('-');
    let day = number(parts.next()?, 2)?;
    let mon = month(parts.next()?)?;
    let yy: i32 = number(parts.next()?, 2)?;
    if parts.next().is_some() {
        return None;
    }
    let century = now.year() - now.year().rem_euclid(100);
    let mut year = century + yy;
    if year > now.year() + 50 {
        year -= 100;
    }
    let date = Date::from_calendar_date(year, mon, day).ok()?;
    Some(at(date, time_of_day(time)?))
}

/// `Sun Nov  6 08:49:37 1994` (the day is padded with a space).
fn asctime_date(text: &str) -> Option<i64> {
    let [dow, mon, day, time, year] = words(text)?;
    if !SHORT_DAYS.contains(&dow) {
        return None;
    }
    let day: u8 = match day.len() {
        1 => number(day, 1)?,
        _ => number(day, 2)?,
    };
    let date = Date::from_calendar_date(number(year, 4)?, month(mon)?, day).ok()?;
    Some(at(date, time_of_day(time)?))
}

/// The words of a date, up to five.
fn words<const N: usize>(text: &str) -> Option<[&str; N]> {
    let words: Vec<&str> = text.split_ascii_whitespace().collect();
    words.try_into().ok()
}

/// The Unix time of an HTTP date in any of the three formats; `now`
/// places rfc850's two-digit years.
pub(crate) fn parse(text: &str, now: OffsetDateTime) -> Option<i64> {
    let text = text.trim();
    match text.split_once(',') {
        Some((dow, rest)) if SHORT_DAYS.contains(&dow) => imf_fixdate(rest),
        Some((dow, rest)) if LONG_DAYS.contains(&dow) => rfc850_date(rest, now),
        Some(_) => None,
        None => asctime_date(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1994-11-06T08:49:37Z, the RFC's example.
    const EXAMPLE: i64 = 784_111_777;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_793_000_000).unwrap() // 2026
    }

    #[test]
    fn the_three_formats() {
        let now = now();
        assert_eq!(parse("Sun, 06 Nov 1994 08:49:37 GMT", now), Some(EXAMPLE));
        assert_eq!(parse("Sunday, 06-Nov-94 08:49:37 GMT", now), Some(EXAMPLE));
        assert_eq!(parse("Sun Nov  6 08:49:37 1994", now), Some(EXAMPLE));
        assert_eq!(parse("Sun Nov 06 08:49:37 1994", now), Some(EXAMPLE));
    }

    /// In 2026, `76` is 2076 (50 years ahead, not more) and `77` is 1977.
    #[test]
    fn rfc850_two_digit_years() {
        let now = now();
        let y2076 = parse("Monday, 01-Jan-76 00:00:00 GMT", now).unwrap();
        let y1977 = parse("Monday, 01-Jan-77 00:00:00 GMT", now).unwrap();
        assert_eq!(y2076, 3_345_062_400);
        assert_eq!(y1977, 220_924_800);
        assert_eq!(
            parse("Monday, 01-Jan-26 00:00:00 GMT", now),
            Some(1_767_225_600)
        );
    }

    #[test]
    fn rejects_anything_else() {
        let now = now();
        for bad in [
            "",
            "120",
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 6 Nov 1994 08:49:37 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 31 Feb 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 25:49:37 GMT",
            "Sun, 06 Nov 1994 08:49 GMT",
            "Sunday, 06-Nov-1994 08:49:37 GMT",
            "Xyz, 06 Nov 1994 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994 extra",
        ] {
            assert_eq!(parse(bad, now), None, "{bad:?}");
        }
    }

    #[test]
    fn leap_second() {
        assert_eq!(
            parse("Sun, 06 Nov 1994 08:49:60 GMT", now()),
            Some(EXAMPLE + 22)
        );
    }
}
