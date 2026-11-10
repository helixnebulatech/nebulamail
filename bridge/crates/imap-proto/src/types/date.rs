// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! IMAP dates: `date` (SEARCH) and `date-time` (INTERNALDATE, APPEND).

use std::fmt;
use std::str::FromStr;

use crate::ValueError;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A calendar date (`1-Feb-2024`), as used by SEARCH SINCE/BEFORE/ON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImapDate {
    year: u16,
    month: u8,
    day: u8,
}

impl ImapDate {
    /// Validates a date (years 1 to 9999).
    ///
    /// # Errors
    ///
    /// Impossible dates such as 30 February.
    pub fn new(year: u16, month: u8, day: u8) -> Result<Self, ValueError> {
        if !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
            return Err(ValueError::OutOfRange("date"));
        }
        if day == 0 || day > days_in_month(year, month) {
            return Err(ValueError::OutOfRange("date"));
        }
        Ok(Self { year, month, day })
    }

    /// The year.
    #[must_use]
    pub fn year(self) -> u16 {
        self.year
    }

    /// The month (1 to 12).
    #[must_use]
    pub fn month(self) -> u8 {
        self.month
    }

    /// The day of the month.
    #[must_use]
    pub fn day(self) -> u8 {
        self.day
    }

    fn month_name(self) -> &'static str {
        MONTHS
            .get(usize::from(self.month.saturating_sub(1)))
            .copied()
            .unwrap_or("Jan")
    }

    /// Days since 1970-01-01 (negative before).
    fn days_from_epoch(self) -> Option<i64> {
        // Howard Hinnant's `days_from_civil`; years are >= 1 here.
        let month = i64::from(self.month);
        let year = i64::from(self.year).checked_sub(i64::from(month <= 2))?;
        let era = year.checked_div(400)?;
        let year_of_era = year.checked_sub(era.checked_mul(400)?)?;
        let shifted_month = if month > 2 {
            month.checked_sub(3)?
        } else {
            month.checked_add(9)?
        };
        let day_of_year = shifted_month
            .checked_mul(153)?
            .checked_add(2)?
            .checked_div(5)?
            .checked_add(i64::from(self.day))?
            .checked_sub(1)?;
        let day_of_era = year_of_era
            .checked_mul(365)?
            .checked_add(year_of_era.checked_div(4)?)?
            .checked_sub(year_of_era.checked_div(100)?)?
            .checked_add(day_of_year)?;
        era.checked_mul(146_097)?
            .checked_add(day_of_era)?
            .checked_sub(719_468)
    }

    fn from_days_since_epoch(days: i64) -> Option<Self> {
        // Howard Hinnant's `civil_from_days`.
        let shifted = days.checked_add(719_468)?;
        let era = if shifted >= 0 {
            shifted
        } else {
            shifted.checked_sub(146_096)?
        }
        .checked_div(146_097)?;
        let day_of_era = shifted.checked_sub(era.checked_mul(146_097)?)?;
        let year_of_era = day_of_era
            .checked_sub(day_of_era.checked_div(1460)?)?
            .checked_add(day_of_era.checked_div(36_524)?)?
            .checked_sub(day_of_era.checked_div(146_096)?)?
            .checked_div(365)?;
        let day_of_year = day_of_era.checked_sub(
            year_of_era
                .checked_mul(365)?
                .checked_add(year_of_era.checked_div(4)?)?
                .checked_sub(year_of_era.checked_div(100)?)?,
        )?;
        let shifted_month = day_of_year
            .checked_mul(5)?
            .checked_add(2)?
            .checked_div(153)?;
        let day = day_of_year
            .checked_sub(
                shifted_month
                    .checked_mul(153)?
                    .checked_add(2)?
                    .checked_div(5)?,
            )?
            .checked_add(1)?;
        let month = if shifted_month < 10 {
            shifted_month.checked_add(3)?
        } else {
            shifted_month.checked_sub(9)?
        };
        let year = year_of_era
            .checked_add(era.checked_mul(400)?)?
            .checked_add(i64::from(month <= 2))?;
        Self::new(
            u16::try_from(year).ok()?,
            u8::try_from(month).ok()?,
            u8::try_from(day).ok()?,
        )
        .ok()
    }
}

fn is_leap(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl fmt::Display for ImapDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{:04}", self.day, self.month_name(), self.year)
    }
}

impl FromStr for ImapDate {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = ValueError::Invalid("date");
        let mut parts = text.trim_start().splitn(3, '-');
        let (Some(day), Some(month), Some(year)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid);
        };
        let day = parse_number(day, 1, 2).ok_or(invalid.clone())?;
        let year = parse_number(year, 4, 4).ok_or(invalid.clone())?;
        let month = MONTHS
            .iter()
            .position(|name| name.eq_ignore_ascii_case(month))
            .and_then(|index| u8::try_from(index).ok())
            .and_then(|index| index.checked_add(1))
            .ok_or(invalid.clone())?;
        Self::new(
            u16::try_from(year).map_err(|_| invalid.clone())?,
            month,
            u8::try_from(day).map_err(|_| invalid)?,
        )
    }
}

fn parse_number(text: &str, min_len: usize, max_len: usize) -> Option<u32> {
    if text.len() < min_len || text.len() > max_len || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A date and time with a UTC offset (`17-Jul-1996 02:44:25 -0700`), as in
/// INTERNALDATE and APPEND.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InternalDate {
    date: ImapDate,
    hour: u8,
    minute: u8,
    second: u8,
    /// Minutes east of UTC.
    offset_minutes: i16,
    /// The same instant in seconds since 1970-01-01 UTC, always between
    /// [`MIN_UNIX`] and [`MAX_UNIX`].
    unix: i64,
}

/// Largest UTC offset of the `zone` rule: ±23:59.
const MAX_OFFSET_MINUTES: i16 = 23 * 60 + 59;

/// `0001-01-01T00:00:00Z`
const MIN_UNIX: i64 = -62_135_596_800;
/// `9999-12-31T23:59:59Z`
const MAX_UNIX: i64 = 253_402_300_799;

impl InternalDate {
    /// Validates a date-time.
    ///
    /// The instant in UTC must fall in the years 1 to 9999 too, so every
    /// value has a Unix time and an RFC 3339 form (`31-Dec-9999 23:59:59
    /// -0001` is in the year 10000 in UTC: refused).
    ///
    /// # Errors
    ///
    /// Out-of-range time, offset or instant.
    pub fn new(
        date: ImapDate,
        hour: u8,
        minute: u8,
        second: u8,
        offset_minutes: i16,
    ) -> Result<Self, ValueError> {
        // Second 60 is a leap second, which the grammar allows.
        if hour > 23 || minute > 59 || second > 60 {
            return Err(ValueError::OutOfRange("time"));
        }
        if !(-MAX_OFFSET_MINUTES..=MAX_OFFSET_MINUTES).contains(&offset_minutes) {
            return Err(ValueError::OutOfRange("time zone"));
        }
        let unix = unix_seconds(date, hour, minute, second, offset_minutes)
            .filter(|unix| (MIN_UNIX..=MAX_UNIX).contains(unix))
            .ok_or(ValueError::OutOfRange("date"))?;
        Ok(Self {
            date,
            hour,
            minute,
            second,
            offset_minutes,
            unix,
        })
    }

    /// The UTC date-time of a Unix timestamp (seconds).
    ///
    /// # Errors
    ///
    /// Timestamps outside the years 1 to 9999.
    pub fn from_unix_timestamp(seconds: i64) -> Result<Self, ValueError> {
        let out_of_range = ValueError::OutOfRange("timestamp");
        let days = seconds.div_euclid(86_400);
        let in_day = seconds.rem_euclid(86_400);
        let date = ImapDate::from_days_since_epoch(days).ok_or(out_of_range.clone())?;
        let hour = in_day.checked_div(3600).and_then(|h| u8::try_from(h).ok());
        let minute = in_day
            .checked_rem(3600)
            .and_then(|r| r.checked_div(60))
            .and_then(|m| u8::try_from(m).ok());
        let second = in_day.checked_rem(60).and_then(|s| u8::try_from(s).ok());
        match (hour, minute, second) {
            (Some(hour), Some(minute), Some(second)) => Self::new(date, hour, minute, second, 0),
            _ => Err(out_of_range),
        }
    }

    /// Seconds since 1970-01-01 UTC (years 1 to 9999).
    #[must_use]
    pub fn unix_timestamp(&self) -> i64 {
        self.unix
    }

    /// The calendar date (in the given offset).
    #[must_use]
    pub fn date(&self) -> ImapDate {
        self.date
    }

    /// Minutes east of UTC.
    #[must_use]
    pub fn offset_minutes(&self) -> i16 {
        self.offset_minutes
    }
}

fn unix_seconds(
    date: ImapDate,
    hour: u8,
    minute: u8,
    second: u8,
    offset_minutes: i16,
) -> Option<i64> {
    date.days_from_epoch()?
        .checked_mul(86_400)?
        .checked_add(i64::from(hour).checked_mul(3600)?)?
        .checked_add(i64::from(minute).checked_mul(60)?)?
        .checked_add(i64::from(second))?
        .checked_sub(i64::from(offset_minutes).checked_mul(60)?)
}

impl fmt::Display for InternalDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.offset_minutes < 0 { '-' } else { '+' };
        let offset = self.offset_minutes.unsigned_abs();
        write!(
            f,
            "{:02}-{}-{:04} {:02}:{:02}:{:02} {sign}{:02}{:02}",
            self.date.day,
            self.date.month_name(),
            self.date.year,
            self.hour,
            self.minute,
            self.second,
            offset / 60,
            offset % 60,
        )
    }
}

impl FromStr for InternalDate {
    type Err = ValueError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = ValueError::Invalid("date-time");
        // The day may be padded with a space (" 1-Jan-2024 …").
        let mut parts = text.trim_start().split(' ');
        let (Some(date), Some(time), Some(zone), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid);
        };
        let date: ImapDate = date.parse()?;
        let mut clock = time.split(':');
        let (Some(hour), Some(minute), Some(second), None) =
            (clock.next(), clock.next(), clock.next(), clock.next())
        else {
            return Err(invalid);
        };
        let field = |text: &str| {
            parse_number(text, 2, 2)
                .and_then(|value| u8::try_from(value).ok())
                .ok_or(invalid.clone())
        };
        let (hour, minute, second) = (field(hour)?, field(minute)?, field(second)?);
        let offset = parse_zone(zone).ok_or(invalid.clone())?;
        Self::new(date, hour, minute, second, offset)
    }
}

fn parse_zone(zone: &str) -> Option<i16> {
    let (negative, digits) = match zone.split_at_checked(1)? {
        ("+", digits) => (false, digits),
        ("-", digits) => (true, digits),
        _ => return None,
    };
    let (hours, minutes) = digits.split_at_checked(2)?;
    let hours = i16::try_from(parse_number(hours, 2, 2)?).ok()?;
    let minutes = i16::try_from(parse_number(minutes, 2, 2)?).ok()?;
    if minutes > 59 {
        return None;
    }
    let total = hours.checked_mul(60)?.checked_add(minutes)?;
    if negative {
        total.checked_neg()
    } else {
        Some(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_and_format() {
        let date: InternalDate = "17-Jul-1996 02:44:25 -0700".parse().unwrap();
        assert_eq!(date.to_string(), "17-Jul-1996 02:44:25 -0700");
        assert_eq!(date.unix_timestamp(), 837_596_665);
        let padded: InternalDate = " 1-Jan-2024 00:00:00 +0000".parse().unwrap();
        assert_eq!(padded.unix_timestamp(), 1_704_067_200);
        assert_eq!(ImapDate::new(2024, 2, 1).unwrap().to_string(), "1-Feb-2024");
        assert!("30-Feb-2024".parse::<ImapDate>().is_err());
        assert!("1-Foo-2024".parse::<ImapDate>().is_err());
        assert!(
            "01-Jan-2024 24:00:00 +0000"
                .parse::<InternalDate>()
                .is_err()
        );
        assert!(
            "01-Jan-2024 10:00:00 +0099"
                .parse::<InternalDate>()
                .is_err()
        );
    }

    #[test]
    fn internal_date_rejects_utc_overflow() {
        let parse = |text: &str| text.parse::<InternalDate>();
        assert!(parse("31-Dec-9999 23:59:59 -0001").is_err());
        assert!(parse("31-Dec-9999 23:59:60 +0000").is_err());
        assert!(parse("01-Jan-0001 00:00:00 +0001").is_err());
        let last = parse("31-Dec-9999 23:59:59 +0001").unwrap();
        assert_eq!(last.unix_timestamp(), MAX_UNIX - 60);
        let first = parse("01-Jan-0001 00:00:00 -0001").unwrap();
        assert_eq!(first.unix_timestamp(), MIN_UNIX + 60);
        assert_eq!(
            InternalDate::from_unix_timestamp(MAX_UNIX)
                .unwrap()
                .unix_timestamp(),
            MAX_UNIX
        );
        assert_eq!(
            InternalDate::from_unix_timestamp(MIN_UNIX)
                .unwrap()
                .unix_timestamp(),
            MIN_UNIX
        );
        assert!(InternalDate::from_unix_timestamp(MAX_UNIX + 1).is_err());
    }

    proptest! {
        #[test]
        fn unix_round_trip(seconds in -62_135_596_800i64..253_402_300_799) {
            let date = InternalDate::from_unix_timestamp(seconds).unwrap();
            prop_assert_eq!(date.unix_timestamp(), seconds);
            let again: InternalDate = date.to_string().parse().unwrap();
            prop_assert_eq!(again, date);
        }
    }
}
