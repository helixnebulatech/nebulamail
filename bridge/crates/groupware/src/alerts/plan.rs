// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The plan: the alerts of an account that ring soon, from the local copy.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use nebula_store::{
    CollectionKind, FiredAlert, Parsed, ResourceId, ResourceQuery, StoredResource, UnixTime,
};

use super::{AlertConfig, secs};
use crate::GroupwareError;
use crate::core::account::GroupwareAccount;
use crate::model::alarm::{
    AlarmLimits, DueAlert, MAX_ALARM_EVALUATIONS, alerts_counted, zone_or_utc,
};
use crate::model::calendar::{ParsedCalendar, RecurrenceKey, Zone};

/// Events read and parsed together (one blocking task per batch).
const BATCH: usize = 32;

/// The index reads floating times as UTC: widen by the largest offset.
const FLOATING: Duration = Duration::from_hours(14);

/// Trigger evaluations per plan, at most, shared by all its events: the
/// events can come from anyone (invitations), and a plan runs often.
pub(super) const PLAN_WORK: usize = 2_000_000;

/// One alert to send at `fire_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Planned {
    pub(super) fire_at: i64,
    pub(super) resource: ResourceId,
    pub(super) uid: String,
    pub(super) alert_id: String,
    pub(super) occurrence: i64,
    pub(super) recurrence: Option<RecurrenceKey>,
    pub(super) repetition: u32,
}

impl Planned {
    pub(super) fn fired(&self) -> FiredAlert {
        FiredAlert {
            resource: self.resource,
            alert_id: self.alert_id.clone(),
            occurrence: UnixTime::from_secs(self.occurrence),
            repetition: self.repetition,
        }
    }
}

/// The events whose alerts may ring in `[from, to)`, newest first, at most
/// `max_events`.
fn candidates(
    mut found: Vec<StoredResource>,
    config: &AlertConfig,
) -> Vec<(StoredResource, String)> {
    found.sort_by_key(|resource| std::cmp::Reverse(resource.id));
    if found.len() > config.max_events {
        tracing::warn!(
            events = found.len(),
            max = config.max_events,
            "too many events near now: only the newest are looked at for alerts"
        );
        found.truncate(config.max_events);
    }
    found
        .into_iter()
        .filter_map(|resource| {
            let uid = match &resource.parsed {
                Parsed::Valid(index) => index.uid.clone()?,
                Parsed::Invalid => return None,
            };
            (!uid.is_empty() && !uid.chars().any(char::is_control)).then_some((resource, uid))
        })
        .collect()
}

/// The alerts of one event in `[from, to)` (off the async threads), and
/// the trigger evaluations used.
fn event_alerts(
    text: &str,
    zone: Zone,
    window: (i64, i64),
    limits: AlarmLimits,
) -> (Vec<DueAlert>, usize) {
    ParsedCalendar::parse(text.as_bytes())
        .and_then(|parsed| alerts_counted(&parsed, zone, window, limits))
        .unwrap_or_default()
}

/// The alerts of a batch of events, sharing `work` trigger evaluations
/// (off the async threads); the evaluations left.
pub(super) fn batch_alerts(
    texts: Vec<(ResourceId, String, Zone, String)>,
    window: (i64, i64),
    limits: AlarmLimits,
    mut work: usize,
) -> (Vec<Planned>, usize) {
    let mut out = Vec::new();
    for (resource, uid, zone, text) in texts {
        if work == 0 {
            break;
        }
        let limits = AlarmLimits {
            work: limits.work.min(work),
            ..limits
        };
        let (due, used) = event_alerts(&text, zone, window, limits);
        work = work.saturating_sub(used);
        out.extend(due.into_iter().map(|due| Planned {
            fire_at: due.fire_at,
            resource,
            uid: uid.clone(),
            alert_id: due.alert_id,
            occurrence: due.occurrence,
            recurrence: due.recurrence,
            repetition: due.repetition,
        }));
    }
    (out, work)
}

/// Planned alerts in the order they ring.
fn by_time(a: &Planned, b: &Planned) -> std::cmp::Ordering {
    (
        a.fire_at,
        a.resource,
        &a.alert_id,
        a.occurrence,
        a.repetition,
    )
        .cmp(&(
            b.fire_at,
            b.resource,
            &b.alert_id,
            b.occurrence,
            b.repetition,
        ))
}

/// The account's alerts that ring in `[now - grace, now + horizon)` and
/// were not sent yet, sorted by time.
pub(super) async fn plan(
    account: &GroupwareAccount,
    now: i64,
) -> Result<Vec<Planned>, GroupwareError> {
    let config = &account.config().alerts;
    let from = now.saturating_sub(secs(config.grace));
    let to = now.saturating_add(secs(config.horizon));
    let store = account.store();
    let near = (
        UnixTime::from_secs(from)
            .saturating_sub(config.max_lag)
            .saturating_sub(FLOATING),
        UnixTime::from_secs(to)
            .saturating_add(config.max_lead)
            .saturating_add(FLOATING),
    );
    let found = store
        .visible_resources(
            account.id,
            ResourceQuery {
                overlapping: Some(near),
                ..ResourceQuery::all(CollectionKind::Calendar)
            },
        )
        .await?;
    let zones: HashMap<_, Zone> = store
        .dav_collections(account.id)
        .await?
        .into_iter()
        .map(|collection| {
            (
                collection.id,
                zone_or_utc(collection.props.time_zone.as_deref()),
            )
        })
        .collect();
    let fired: HashSet<FiredAlert> = store
        .fired_alerts(
            account.id,
            UnixTime::from_secs(now).saturating_sub(config.keep_fired),
        )
        .await?
        .into_iter()
        .collect();
    let mut out = Vec::new();
    let events = candidates(found, config);
    let limits = AlarmLimits {
        max: config.max_per_event,
        repeats: config.max_repeats,
        lead: secs(config.max_lead).saturating_add(secs(FLOATING)),
        lag: secs(config.max_lag).saturating_add(secs(FLOATING)),
        work: MAX_ALARM_EVALUATIONS,
    };
    let mut work = PLAN_WORK;
    for batch in events.chunks(BATCH) {
        if work == 0 {
            break;
        }
        let mut texts = Vec::with_capacity(batch.len());
        for (resource, uid) in batch {
            match account.resource_text(resource.id).await {
                Ok(Some(found)) => texts.push((
                    resource.id,
                    uid.clone(),
                    zones
                        .get(&resource.collection)
                        .copied()
                        .unwrap_or(Zone::UTC),
                    found.text,
                )),
                Ok(None) => {}
                // One event that does not open never stops the others.
                Err(error) => tracing::debug!(%error, "event skipped for alerts"),
            }
        }
        let (planned, left) =
            tokio::task::spawn_blocking(move || batch_alerts(texts, (from, to), limits, work))
                .await
                .map_err(|_| GroupwareError::Task)?;
        work = left;
        out.extend(
            planned
                .into_iter()
                .filter(|planned| !fired.contains(&planned.fired())),
        );
    }
    if work == 0 {
        // No content in the log: the events can be anyone's.
        tracing::warn!("calendar alerts: work limit of the plan reached, later events skipped");
    }
    out.sort_by(by_time);
    Ok(out)
}
