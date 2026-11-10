// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! RFC 4865 FUTURERELEASE parameters of `MAIL FROM`: `HOLDFOR` (seconds,
//! `1*9DIGIT`) and `HOLDUNTIL` (an RFC 3339 date-time), parsed strictly and
//! turned into the time the bridge releases the message.
//!
//! As Stalwart: a `HOLDUNTIL` that isn't in the future, or a hold past the
//! maximum, is `forbiddenMailFrom` with Stalwart's wording ("…exceeds
//! maximum of <date>."), which the web app reads to learn the limit.
//! `HOLDFOR=0` means no hold (sent at once). Both at once, or a value that
//! doesn't parse, is `invalidProperties`. The parameters never reach the
//! SMTP server: the bridge holds the message itself.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_jmap_proto::{SetError, SetErrorType};
use nebula_store::UnixTime;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The longest `HOLDFOR` value: 9 digits (RFC 4865 §3).
const MAX_HOLDFOR_DIGITS: usize = 9;

/// What the client asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldRequest {
    /// `HOLDFOR`: this many seconds from now.
    For(u32),
    /// `HOLDUNTIL`: at this time (Unix seconds, rounded up).
    Until(i64),
}

/// Why a hold can't be done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum HoldError {
    #[error("invalid value for SMTP parameter {0}")]
    Malformed(&'static str),
    #[error("HOLDFOR and HOLDUNTIL can't be used together")]
    Both,
    #[error("delayed sending is not supported (maxDelayedSend is 0)")]
    Disabled,
    #[error("HOLDUNTIL must be a date and time in the future")]
    Past,
    /// Past the maximum, which is this time (Unix seconds).
    #[error("Requested release time exceeds maximum of {}.", format_utc(*.0))]
    TooFar(i64),
}

impl HoldError {
    /// The JMAP error (RFC 8621 §7.5).
    pub(crate) fn set_error(self) -> SetError {
        let kind = match self {
            Self::Malformed(_) | Self::Both | Self::Disabled => SetErrorType::InvalidProperties,
            Self::Past | Self::TooFar(_) => SetErrorType::ForbiddenMailFrom,
        };
        let error = if kind == SetErrorType::InvalidProperties {
            SetError::invalid_properties(["envelope"])
        } else {
            SetError::new(kind)
        };
        error.with_description(self.to_string())
    }
}

/// `2026-11-09T20:06:18Z`.
fn format_utc(secs: i64) -> String {
    OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|at| {
            at.format(time::macros::format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
            ))
            .ok()
        })
        .unwrap_or_default()
}

impl HoldRequest {
    /// `HOLDFOR`'s value: 1 to 9 ASCII digits, nothing else (no sign).
    pub(crate) fn parse_for(value: &str) -> Result<Self, HoldError> {
        let malformed = HoldError::Malformed("HOLDFOR");
        if value.is_empty()
            || value.len() > MAX_HOLDFOR_DIGITS
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(malformed);
        }
        value.parse().map(Self::For).map_err(|_| malformed)
    }

    /// `HOLDUNTIL`'s value: an RFC 3339 date-time with its offset. A
    /// fraction of a second rounds up, so the message never leaves early.
    pub(crate) fn parse_until(value: &str) -> Result<Self, HoldError> {
        let at = OffsetDateTime::parse(value, &Rfc3339)
            .map_err(|_| HoldError::Malformed("HOLDUNTIL"))?;
        let secs = at.unix_timestamp();
        let secs = if at.nanosecond() > 0 {
            secs.saturating_add(1)
        } else {
            secs
        };
        Ok(Self::Until(secs))
    }

    /// When to release the message, asked at `now` with holds up to
    /// `max`; `None` to send it at once (`HOLDFOR=0`).
    pub(crate) fn release_at(self, now: Now, max: Duration) -> Result<Option<UnixTime>, HoldError> {
        if max.is_zero() {
            return Err(HoldError::Disabled);
        }
        let max_secs = i64::try_from(max.as_secs()).unwrap_or(i64::MAX);
        let latest = now.ceil.saturating_add(max_secs);
        let at = match self {
            Self::For(0) => return Ok(None),
            Self::For(secs) => now.ceil.saturating_add(i64::from(secs)),
            Self::Until(at) if at <= now.floor => return Err(HoldError::Past),
            Self::Until(at) => at,
        };
        if at > latest {
            return Err(HoldError::TooFar(latest));
        }
        Ok(Some(UnixTime::from_secs(at)))
    }
}

/// The time a hold counts from, in whole seconds both ways: a `HOLDFOR`
/// starts at the next second (never early), a `HOLDUNTIL` must be after
/// the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Now {
    floor: i64,
    ceil: i64,
}

impl Now {
    /// The system clock.
    pub(crate) fn system() -> Option<Self> {
        let since = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
        let floor = i64::try_from(since.as_secs()).ok()?;
        let ceil = if since.subsec_nanos() > 0 {
            floor.saturating_add(1)
        } else {
            floor
        };
        Some(Self { floor, ceil })
    }

    #[cfg(test)]
    pub(crate) const fn at(secs: i64) -> Self {
        Self {
            floor: secs,
            ceil: secs,
        }
    }

    /// The current second, rounded down.
    pub(crate) const fn floor(self) -> UnixTime {
        UnixTime::from_secs(self.floor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK: Duration = Duration::from_hours(7 * 24);
    /// 2026-11-03T12:00:00Z
    const NOW: i64 = 1_793_707_200;

    #[test]
    fn holdfor_is_digits_only() {
        assert_eq!(HoldRequest::parse_for("10"), Ok(HoldRequest::For(10)));
        assert_eq!(HoldRequest::parse_for("0"), Ok(HoldRequest::For(0)));
        assert_eq!(
            HoldRequest::parse_for("999999999"),
            Ok(HoldRequest::For(999_999_999))
        );
        for bad in ["", "+5", "-5", "1e3", "5 ", "0x10", "1000000000", "١٠"] {
            assert_eq!(
                HoldRequest::parse_for(bad),
                Err(HoldError::Malformed("HOLDFOR")),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn holduntil_is_rfc3339() {
        assert_eq!(
            HoldRequest::parse_until("2026-11-03T12:00:00Z"),
            Ok(HoldRequest::Until(NOW))
        );
        assert_eq!(
            HoldRequest::parse_until("2026-11-03T13:00:00+01:00"),
            Ok(HoldRequest::Until(NOW))
        );
        // A fraction rounds up: never early.
        assert_eq!(
            HoldRequest::parse_until("2026-11-03T12:00:00.001Z"),
            Ok(HoldRequest::Until(NOW + 1))
        );
        for bad in [
            "",
            "2026-11-03",
            "2026-11-03 12:00:00",
            "2026-11-03T12:00:00",
            "1793793600",
            "2026-13-03T12:00:00Z",
            "tomorrow",
        ] {
            assert_eq!(
                HoldRequest::parse_until(bad),
                Err(HoldError::Malformed("HOLDUNTIL")),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn release_times_and_limits() {
        let now = Now::at(NOW);
        let at = |secs| Ok(Some(UnixTime::from_secs(secs)));
        assert_eq!(HoldRequest::For(10).release_at(now, WEEK), at(NOW + 10));
        assert_eq!(HoldRequest::For(0).release_at(now, WEEK), Ok(None));
        assert_eq!(
            HoldRequest::Until(NOW + 60).release_at(now, WEEK),
            at(NOW + 60)
        );
        let week = i64::try_from(WEEK.as_secs()).unwrap();
        assert_eq!(
            HoldRequest::Until(NOW + week).release_at(now, WEEK),
            at(NOW + week)
        );
        assert_eq!(
            HoldRequest::Until(NOW + week + 1).release_at(now, WEEK),
            Err(HoldError::TooFar(NOW + week))
        );
        assert_eq!(
            HoldRequest::For(999_999_999).release_at(now, WEEK),
            Err(HoldError::TooFar(NOW + week))
        );
        assert_eq!(
            HoldRequest::Until(NOW).release_at(now, WEEK),
            Err(HoldError::Past)
        );
        assert_eq!(
            HoldRequest::Until(NOW - 3600).release_at(now, WEEK),
            Err(HoldError::Past)
        );
        assert_eq!(
            HoldRequest::For(10).release_at(now, Duration::ZERO),
            Err(HoldError::Disabled)
        );
    }

    #[test]
    fn errors_read_like_stalwart() {
        let error = HoldError::TooFar(NOW).set_error();
        assert_eq!(error.kind(), SetErrorType::ForbiddenMailFrom);
        assert_eq!(
            error.description(),
            Some("Requested release time exceeds maximum of 2026-11-03T12:00:00Z.")
        );
        let error = HoldError::Past.set_error();
        assert_eq!(error.kind(), SetErrorType::ForbiddenMailFrom);
        assert_eq!(
            HoldError::Both.set_error().kind(),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            HoldError::Disabled.set_error().kind(),
            SetErrorType::InvalidProperties
        );
    }

    #[test]
    fn now_rounds_both_ways() {
        let now = Now::system().unwrap();
        assert!(now.ceil - now.floor <= 1);
        assert_eq!(now.floor(), UnixTime::from_secs(now.floor));
    }
}
