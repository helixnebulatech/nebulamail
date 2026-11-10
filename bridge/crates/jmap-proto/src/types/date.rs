// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Date` and `UTCDate` (RFC 8620 §1.4): RFC 3339 `date-time` strings.

use std::fmt;
use std::str::FromStr;

use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use crate::macros::serde_via_str;

/// Why a string is not a valid JMAP date.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DateError {
    /// Not an RFC 3339 `date-time`.
    #[error("not an RFC 3339 date-time")]
    Syntax,
    /// A `UTCDate` must end in `Z`.
    #[error("UTCDate must be in UTC and end with \"Z\"")]
    NotUtc,
    /// The year is outside 0000..=9999 (RFC 3339 has 4-digit years).
    #[error("year out of range")]
    OutOfRange,
    /// The offset has seconds, which RFC 3339 can't write.
    #[error("time zone offset has seconds")]
    OffsetSeconds,
}

/// A `UTCDate` (RFC 8620 §1.4): a date-time in UTC, written with `Z`.
///
/// Parsing accepts fractional seconds, even `.000` (JavaScript's
/// `toISOString` sends them, although RFC 8620 says to omit a zero
/// fraction). Serializing omits a zero fraction and trims trailing zeros:
/// `2014-10-30T06:12:00Z`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcDate(OffsetDateTime);

impl UtcDate {
    /// The date for `time`, converted to UTC.
    ///
    /// # Errors
    /// [`DateError::OutOfRange`] if the year is not 0000..=9999.
    pub fn new(time: OffsetDateTime) -> Result<Self, DateError> {
        let utc = time
            .checked_to_offset(UtcOffset::UTC)
            .ok_or(DateError::OutOfRange)?;
        check_year(utc)?;
        Ok(Self(utc))
    }

    /// The date `seconds` after 1970-01-01T00:00:00Z.
    ///
    /// # Errors
    /// [`DateError::OutOfRange`] if it is not between years 0000 and 9999.
    pub fn from_unix_timestamp(seconds: i64) -> Result<Self, DateError> {
        OffsetDateTime::from_unix_timestamp(seconds)
            .map_err(|_| DateError::OutOfRange)
            .and_then(Self::new)
    }

    /// Seconds since 1970-01-01T00:00:00Z.
    #[must_use]
    pub fn unix_timestamp(self) -> i64 {
        self.0.unix_timestamp()
    }

    /// The date as a `time` value (offset UTC).
    #[must_use]
    pub fn to_offset_date_time(self) -> OffsetDateTime {
        self.0
    }
}

impl FromStr for UtcDate {
    type Err = DateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !s.ends_with(['Z', 'z']) {
            return Err(DateError::NotUtc);
        }
        parse_rfc3339(s).and_then(Self::new)
    }
}

impl fmt::Display for UtcDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_date_time(f, self.0)?;
        f.write_str("Z")
    }
}

impl fmt::Debug for UtcDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UtcDate({self})")
    }
}

serde_via_str!(UtcDate);

/// A `Date` (RFC 8620 §1.4): a date-time with its time zone offset, e.g. an
/// Email's `sentAt` (`2014-10-30T14:12:00+08:00`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(OffsetDateTime);

impl Date {
    /// Wraps a date-time, keeping its offset.
    ///
    /// # Errors
    /// [`DateError::OutOfRange`] if the year is not 0000..=9999 (in UTC and
    /// in its own offset); [`DateError::OffsetSeconds`] if the offset is not
    /// a whole number of minutes.
    pub fn new(time: OffsetDateTime) -> Result<Self, DateError> {
        if time.offset().seconds_past_minute() != 0 {
            return Err(DateError::OffsetSeconds);
        }
        check_year(time)?;
        check_year(
            time.checked_to_offset(UtcOffset::UTC)
                .ok_or(DateError::OutOfRange)?,
        )?;
        Ok(Self(time))
    }

    /// The date as a `time` value, with its offset.
    #[must_use]
    pub fn to_offset_date_time(self) -> OffsetDateTime {
        self.0
    }

    /// The same instant in UTC.
    #[must_use]
    pub fn to_utc(self) -> UtcDate {
        // `new` checked that the UTC year is in range, so this can't fail;
        // the fallback keeps it panic-free all the same.
        UtcDate(
            self.0
                .checked_to_offset(UtcOffset::UTC)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
        )
    }
}

impl FromStr for Date {
    type Err = DateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_rfc3339(s).and_then(Self::new)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_date_time(f, self.0)?;
        let offset = self.0.offset();
        if offset.is_utc() {
            return f.write_str("Z");
        }
        let sign = if offset.is_negative() { '-' } else { '+' };
        let (hours, minutes, _) = offset.as_hms();
        write!(
            f,
            "{sign}{:02}:{:02}",
            hours.unsigned_abs(),
            minutes.unsigned_abs()
        )
    }
}

impl fmt::Debug for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Date({self})")
    }
}

serde_via_str!(Date);

fn parse_rfc3339(s: &str) -> Result<OffsetDateTime, DateError> {
    // `time` also accepts a space between date and time (RFC 3339 §5.6
    // note); JMAP requires the `T` (RFC 8620 §1.4).
    if !matches!(s.as_bytes().get(10), Some(b'T' | b't')) {
        return Err(DateError::Syntax);
    }
    OffsetDateTime::parse(s, &Rfc3339).map_err(|_| DateError::Syntax)
}

fn check_year(time: OffsetDateTime) -> Result<(), DateError> {
    if (0..=9999).contains(&time.year()) {
        Ok(())
    } else {
        Err(DateError::OutOfRange)
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction]`, without the offset. Seconds are never
/// 60: `time` folds leap seconds into :59.
fn write_date_time(f: &mut fmt::Formatter<'_>, time: OffsetDateTime) -> fmt::Result {
    write!(
        f,
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second()
    )?;
    let nanos = time.nanosecond();
    if nanos == 0 {
        return Ok(());
    }
    let digits = format!("{nanos:09}");
    write!(f, ".{}", digits.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn edges_out_of_range_do_not_panic() {
        // 9999-12-31T23:59:59-01:00 is year 10000 in UTC.
        assert_eq!(
            "9999-12-31T23:59:59-01:00".parse::<Date>(),
            Err(DateError::OutOfRange)
        );
        let local = time::macros::datetime!(9999-12-31 23:59:59);
        let late = local.assume_offset(UtcOffset::from_hms(-1, 0, 0).unwrap());
        assert_eq!(Date::new(late), Err(DateError::OutOfRange));
        assert_eq!(UtcDate::new(late), Err(DateError::OutOfRange));
        let fine = local.assume_offset(UtcOffset::from_hms(1, 0, 0).unwrap());
        assert!(Date::new(fine).is_ok());
        assert_eq!(
            UtcDate::new(fine).unwrap().to_string(),
            "9999-12-31T22:59:59Z"
        );
    }

    #[test]
    fn utc_date_parses_and_normalizes() {
        let date: UtcDate = "2014-10-30T06:12:00Z".parse().unwrap();
        assert_eq!(date.to_string(), "2014-10-30T06:12:00Z");
        let js: UtcDate = "2026-10-24T10:00:00.000Z".parse().unwrap();
        assert_eq!(js.to_string(), "2026-10-24T10:00:00Z");
        let frac: UtcDate = "2026-10-24T10:00:00.120z".parse().unwrap();
        assert_eq!(frac.to_string(), "2026-10-24T10:00:00.12Z");
    }

    #[test]
    fn utc_date_rejects_offsets_and_junk() {
        for bad in [
            "2014-10-30T06:12:00+00:00",
            "2014-10-30T06:12:00+01:00",
            "2014-10-30 06:12:00Z",
            "2014-10-30Z",
            "",
            "Z",
        ] {
            assert!(bad.parse::<UtcDate>().is_err(), "{bad}");
        }
    }

    #[test]
    fn date_keeps_its_offset() {
        let date: Date = "2014-10-30T14:12:00+08:00".parse().unwrap();
        assert_eq!(date.to_string(), "2014-10-30T14:12:00+08:00");
        assert_eq!(date.to_utc().to_string(), "2014-10-30T06:12:00Z");
        let negative: Date = "2014-10-30T01:12:00-03:30".parse().unwrap();
        assert_eq!(negative.to_string(), "2014-10-30T01:12:00-03:30");
        let utc: Date = "2014-10-30T01:12:00Z".parse().unwrap();
        assert_eq!(utc.to_string(), "2014-10-30T01:12:00Z");
    }

    #[test]
    fn serde_uses_strings() {
        let date: UtcDate = serde_json::from_str("\"2014-10-30T06:12:00Z\"").unwrap();
        assert_eq!(
            serde_json::to_string(&date).unwrap(),
            "\"2014-10-30T06:12:00Z\""
        );
        assert!(serde_json::from_str::<UtcDate>("1414649520").is_err());
    }

    proptest! {
        #[test]
        fn utc_date_round_trips(
            seconds in -62_167_219_200_i64..=253_402_300_799,
            nanos in 0_u32..1_000_000_000,
        ) {
            let time = OffsetDateTime::from_unix_timestamp(seconds).unwrap()
                .replace_nanosecond(nanos).unwrap();
            let date = UtcDate::new(time).unwrap();
            let text = date.to_string();
            prop_assert!(text.ends_with('Z'));
            prop_assert_eq!(text.parse::<UtcDate>().unwrap(), date);
        }

        #[test]
        fn date_round_trips(
            seconds in 0_i64..=253_402_200_000,
            offset_minutes in -1439_i32..=1439,
        ) {
            let hours = i8::try_from(offset_minutes / 60).unwrap();
            let minutes = i8::try_from(offset_minutes % 60).unwrap();
            let offset = UtcOffset::from_hms(hours, minutes, 0).unwrap();
            let time = OffsetDateTime::from_unix_timestamp(seconds).unwrap().to_offset(offset);
            if let Ok(date) = Date::new(time) {
                let parsed: Date = date.to_string().parse().unwrap();
                prop_assert_eq!(parsed, date);
                prop_assert_eq!(parsed.to_offset_date_time().offset(), offset);
            }
        }

        #[test]
        fn utc_date_never_panics(s in "\\PC{0,40}") {
            let _ = s.parse::<UtcDate>();
            let _ = s.parse::<Date>();
        }
    }
}
