// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduler task of an account (see the module docs of
//! [`super`]). It runs next to the live sync and stops with it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use nebula_store::{DataType, UnixTime};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{broadcast, watch};

use super::plan::{Planned, plan};
use super::{AlertDue, MIN_REPLAN_GAP, secs};
use crate::GroupwareEvent;
use crate::core::account::GroupwareAccount;
use crate::core::live::or_stop;

/// After a failed plan, try again this soon.
const RETRY: Duration = Duration::from_mins(1);

/// What the loop knows.
pub(super) struct Schedule {
    /// Sorted by time.
    pub(super) queue: VecDeque<Planned>,
    /// When the plan was made (`None`: never, make it now).
    planned_at: Option<i64>,
    /// Make it again at this time.
    next_plan: i64,
}

impl Schedule {
    pub(super) const fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            planned_at: None,
            next_plan: 0,
        }
    }

    fn due_for_plan(&self, now: i64) -> bool {
        self.planned_at.is_none() || now >= self.next_plan
    }

    /// The calendars changed: plan again soon, but not before
    /// [`MIN_REPLAN_GAP`] after the last plan (a burst of changes makes
    /// one plan, not one each).
    fn changed(&mut self, now: i64) {
        let soonest = self
            .planned_at
            .map_or(now, |at| at.saturating_add(MIN_REPLAN_GAP));
        self.next_plan = self.next_plan.min(soonest);
    }
}

/// Takes every event already queued; whether one of them (or a lost one)
/// changes the account's calendars, and whether the channel is closed.
pub(super) fn drain_changes(
    events: &mut broadcast::Receiver<GroupwareEvent>,
    account: &GroupwareAccount,
) -> (bool, bool) {
    let mut changed = false;
    loop {
        match events.try_recv() {
            Ok(event) => changed |= changes_calendars(&event, account),
            Err(TryRecvError::Lagged(_)) => changed = true,
            Err(TryRecvError::Empty) => return (changed, false),
            Err(TryRecvError::Closed) => return (changed, true),
        }
    }
}

fn changes_calendars(event: &GroupwareEvent, account: &GroupwareAccount) -> bool {
    match event {
        GroupwareEvent::StateChanged(changed) => {
            changed.account == account.id
                && changed
                    .data_types
                    .iter()
                    .any(|t| matches!(t, DataType::Calendar | DataType::CalendarEvent))
        }
        _ => false,
    }
}

/// Sends what is due. Kept while nobody listens (until the grace ends).
#[cfg(test)]
pub(super) async fn fire_due(account: &GroupwareAccount, schedule: &mut Schedule, now: i64) {
    fire_due_until(account, schedule, now, || false).await;
}

/// [`fire_due`], looking at `stop` between one alert and the next: it is
/// never cut between recording an alert as sent and sending it.
async fn fire_due_until(
    account: &GroupwareAccount,
    schedule: &mut Schedule,
    now: i64,
    stop: impl Fn() -> bool,
) {
    let grace = secs(account.config().alerts.grace);
    while let Some(first) = schedule.queue.front() {
        if stop() {
            return;
        }
        if first.fire_at > now {
            return;
        }
        if first.fire_at < now.saturating_sub(grace) {
            tracing::debug!(account = %account.id, "calendar alert too late, dropped");
            schedule.queue.pop_front();
            continue;
        }
        if !account.shared.alerts_heard(account.id) {
            // Nobody would see it: wait for an event source to open (or a
            // Web Push subscription to appear).
            return;
        }
        let Some(due) = schedule.queue.pop_front() else {
            return;
        };
        match account
            .store()
            .record_fired_alert(account.id, due.fired(), UnixTime::from_secs(now))
            .await
        {
            Ok(true) => {
                let _ = account
                    .shared
                    .events
                    .send(GroupwareEvent::CalendarAlert(AlertDue {
                        account: account.id,
                        resource: due.resource,
                        uid: due.uid,
                        alert_id: due.alert_id,
                        recurrence: due.recurrence,
                    }));
            }
            // Already sent (before a restart, or the event is gone).
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(account = %account.id, %error, "calendar alert not recorded, not sent");
            }
        }
    }
}

/// How long to wait before looking again.
fn wait_for(account: &GroupwareAccount, schedule: &Schedule, now: i64) -> Duration {
    let tick = account.config().alerts.tick;
    let until = |at: i64| Duration::from_secs(u64::try_from(at.saturating_sub(now)).unwrap_or(0));
    let mut wait = tick.min(until(schedule.next_plan).max(Duration::from_secs(1)));
    if let Some(first) = schedule.queue.front()
        && first.fire_at > now
    {
        wait = wait.min(until(first.fire_at));
    }
    wait
}

pub(crate) async fn run(account: Arc<GroupwareAccount>, mut stopped: watch::Receiver<bool>) {
    let config = account.config().alerts.clone();
    if !config.enabled {
        return;
    }
    let mut events = account.shared.events.subscribe();
    let mut schedule = Schedule::new();
    loop {
        if *stopped.borrow() {
            return;
        }
        let now = config.clock.now();
        if schedule.due_for_plan(now) {
            if account.availability().calendars {
                let Some(planned) = or_stop(&mut stopped, plan(&account, now)).await else {
                    return;
                };
                match planned {
                    Ok(queue) => {
                        schedule.queue = queue.into();
                        schedule.next_plan = now.saturating_add(secs(config.replan));
                    }
                    Err(error) => {
                        tracing::warn!(account = %account.id, %error, "planning calendar alerts failed");
                        schedule.next_plan = now.saturating_add(secs(RETRY));
                    }
                }
                let keep = UnixTime::from_secs(now).saturating_sub(config.keep_fired);
                let Some(purged) = or_stop(
                    &mut stopped,
                    account.store().purge_fired_alerts(account.id, keep),
                )
                .await
                else {
                    return;
                };
                if let Err(error) = purged {
                    tracing::warn!(account = %account.id, %error, "purging sent calendar alerts failed");
                }
            } else {
                // Not discovered yet, or calendars are off: nothing rings.
                schedule.queue.clear();
                schedule.next_plan = now.saturating_add(5);
            }
            schedule.planned_at = Some(now);
        }
        fire_due_until(&account, &mut schedule, now, || *stopped.borrow()).await;
        let wait = wait_for(&account, &schedule, now);
        tokio::select! {
            _ = stopped.changed() => return,
            () = tokio::time::sleep(wait) => {}
            () = account.alerts_wake.notified() => {}
            event = events.recv() => {
                let first = match event {
                    Ok(event) => changes_calendars(&event, &account),
                    Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => return,
                };
                let (more, closed) = drain_changes(&mut events, &account);
                if first || more {
                    schedule.changed(config.clock.now());
                }
                if closed {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A change plans again at most [`MIN_REPLAN_GAP`] after the last
    /// plan, and more changes do not bring it closer.
    #[test]
    fn changes_wait_for_the_gap() {
        let mut schedule = Schedule::new();
        assert!(schedule.due_for_plan(0));
        schedule.planned_at = Some(100);
        schedule.next_plan = 3700;
        schedule.changed(101);
        assert!(!schedule.due_for_plan(101));
        assert!(!schedule.due_for_plan(104));
        assert!(schedule.due_for_plan(105));
        for now in 101..111 {
            schedule.changed(now);
        }
        assert_eq!(schedule.next_plan, 105);
        // Long after the last plan: at once.
        schedule.changed(1000);
        assert!(schedule.due_for_plan(1000));
    }
}
