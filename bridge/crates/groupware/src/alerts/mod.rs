// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendar alerts: the bridge decides when an
//! event's `display` alert is due, like Stalwart, and says so with a
//! [`GroupwareEvent::CalendarAlert`](crate::GroupwareEvent::CalendarAlert)
//! (the HTTP layer sends it on the event source as `calendarAlert`).
//!
//! Each running account has a scheduler next to its live sync:
//!
//! - **Plan**: from the local copy, the alerts that ring in the next
//!   [`AlertConfig::horizon`] (24 h), with [`crate::model::alarm`]: offsets
//!   from the start or end of each occurrence, absolute triggers (snooze),
//!   `ACKNOWLEDGED`, `RELATED-TO` and repeats (`REPEAT`/`DURATION`, at
//!   most [`AlertConfig::max_repeats`]). Only events whose indexed range is
//!   near enough are opened ([`AlertConfig::max_lead`] before,
//!   [`AlertConfig::max_lag`] after), at most [`AlertConfig::max_events`],
//!   [`AlertConfig::max_per_event`] alerts each. Made again when the
//!   account's calendars change (sync, own writes) and every
//!   [`AlertConfig::replan`].
//! - **Fire**: when its time comes, an alert is recorded in the store
//!   (`calendar_alerts_fired`) and, only if that row is new, sent: at most
//!   once per event, alert, occurrence and repeat, also across restarts. Rows go
//!   after [`AlertConfig::keep_fired`] (2 days).
//! - **Late**: an alert is only sent up to [`AlertConfig::grace`] (10 min)
//!   after its time: after a restart, or when the account had no event
//!   source asking for `CalendarAlert` open then, nor Web Push (it waits
//!   for one, nobody would see it). Later ones are dropped: a reminder an
//!   hour late is noise.
//! - **Changes** come in bursts (a sync applies batches, each one an
//!   event): the plan is made again at most once every
//!   [`MIN_REPLAN_GAP`] seconds, for all the changes queued meanwhile.
//!
//! Time comes from a [`Clock`], so tests can move it.

mod plan;
mod task;
#[cfg(test)]
mod tests;

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_store::{AccountId, ResourceId};

use crate::model::calendar::RecurrenceKey;

pub(crate) use task::run;

/// Shortest time between two plans made for changes, in seconds.
pub const MIN_REPLAN_GAP: i64 = 5;

/// Where the scheduler reads the time (UTC seconds).
pub trait Clock: Debug + Send + Sync {
    /// Now, in seconds since the epoch.
    fn now(&self) -> i64;
}

/// The system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
            .unwrap_or(0)
    }
}

/// A clock that only moves when told (tests).
#[derive(Debug, Clone, Default)]
pub struct ManualClock(Arc<AtomicI64>);

impl ManualClock {
    /// A clock at `now`.
    #[must_use]
    pub fn new(now: i64) -> Self {
        Self(Arc::new(AtomicI64::new(now)))
    }

    /// Sets the time.
    pub fn set(&self, now: i64) {
        self.0.store(now, Ordering::Release);
    }

    /// Moves the time forward.
    pub fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::AcqRel);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> i64 {
        self.0.load(Ordering::Acquire)
    }
}

/// Settings of the alert scheduler.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AlertConfig {
    /// Alerts ring at all (`false`: never sent).
    pub enabled: bool,
    /// How far ahead the plan looks: 24 h.
    pub horizon: Duration,
    /// How late an alert may still be sent: 10 min.
    pub grace: Duration,
    /// How long a sent alert is remembered: 2 days.
    pub keep_fired: Duration,
    /// How often the plan is made again without changes: 1 h.
    pub replan: Duration,
    /// Longest wait between two looks at the clock: 30 s.
    pub tick: Duration,
    /// Events opened per plan, at most: 2000 (the newest).
    pub max_events: usize,
    /// Alerts per event and plan, at most: 64 (each repeat counts).
    pub max_per_event: usize,
    /// Repeats of one alarm (`REPEAT`/`DURATION` of a `VALARM`), at most:
    /// 10. 0: each alarm rings once.
    pub max_repeats: u32,
    /// How long before its start an event is looked at: 8 days (an alert
    /// a week before still rings).
    pub max_lead: Duration,
    /// How long after its end: 1 day.
    pub max_lag: Duration,
    /// The time.
    pub clock: Arc<dyn Clock>,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            horizon: Duration::from_hours(24),
            grace: Duration::from_mins(10),
            keep_fired: Duration::from_hours(48),
            replan: Duration::from_hours(1),
            tick: Duration::from_secs(30),
            max_events: 2000,
            max_per_event: 64,
            max_repeats: crate::model::alarm::DEFAULT_MAX_REPEATS,
            max_lead: Duration::from_hours(8 * 24),
            max_lag: Duration::from_hours(24),
            clock: Arc::new(SystemClock),
        }
    }
}

/// An alert that is due now: send a `CalendarAlert`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertDue {
    /// The account.
    pub account: AccountId,
    /// The event (the series for a recurring one).
    pub resource: ResourceId,
    /// The event's `UID`.
    pub uid: String,
    /// The key of the alert in the event's `alerts`.
    pub alert_id: String,
    /// The occurrence of a recurring event: its `RECURRENCE-ID` as a
    /// wall-clock reading in the event's zone (Stalwart sends it as a
    /// `LocalDateTime`).
    pub recurrence: Option<RecurrenceKey>,
}

/// Seconds of a duration, saturating.
fn secs(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}
