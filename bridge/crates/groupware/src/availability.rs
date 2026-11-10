// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The user's own free-busy from the local copy
//! every calendar of the user that holds events counts
//! (JMAP `includeInAvailability: "all"`, what the bridge says for each
//! calendar), not only the default one, which is all Stalwart's
//! scheduling outbox counts. Calendars others share with the user are
//! theirs, not the user's time, and are left out.
//!
//! Each event is read with [`own_busy_occurrences`] (transparency,
//! status, the user's answer to an invitation, recurrences, time zones).
//! It is as fresh as the local copy: own writes at once, other clients'
//! changes after the next sync.
//!
//! With `showDetails` ([`GroupwareAccount::own_availability`]) each period
//! also carries its occurrence as JSCalendar, as `CalendarEvent/get` gives
//! it for an occurrence id, unless the event is `private` or `secret`
//! (iCalendar `CLASS:PRIVATE`/`CONFIDENTIAL`; draft-ietf-jmap-calendars),
//! and only for the first [`MAX_DETAILED`] tries: a private occurrence is
//! not converted at all, and every conversion counts, shown or not.

use std::collections::HashMap;
use std::time::Duration;

use nebula_store::{CollectionId, CollectionKind, Components, ResourceId, ResourceQuery, UnixTime};
use serde_json::{Map, Value};

use crate::GroupwareError;
use crate::core::account::GroupwareAccount;
use crate::model::alarm::zone_or_utc;
use crate::model::calendar::{EventOptions, ParsedCalendar, RecurrenceKey, Zone};
use crate::model::freebusy::{BusyPeriod, MAX_BUSY_PERIODS, own_busy_occurrences};

/// Events read and parsed together (one blocking task per batch).
const BATCH: usize = 64;

/// Events looked at for one request, at most (a year of a very busy
/// calendar). Past it the newest are kept and a warning is logged.
pub(crate) const MAX_EVENTS: usize = 20_000;

/// The index reads floating times as UTC: widen by the largest offset.
const FLOATING: Duration = Duration::from_hours(14);

/// Busy occurrences given with their event, at most (the rest have
/// none): enough for a year of a busy calendar, small enough for one
/// response.
pub const MAX_DETAILED: usize = 1000;

/// One busy period of the user's, with its event when asked for.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct OwnPeriod {
    /// The period.
    pub period: BusyPeriod,
    /// The occurrence as JSCalendar (`None`: not asked for, private or
    /// secret, or past [`MAX_DETAILED`]).
    pub event: Option<OwnEvent>,
}

/// The event of a busy period: one occurrence, as `CalendarEvent/get`
/// gives it for an occurrence id.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnEvent {
    /// The stored object.
    pub resource: ResourceId,
    /// Its calendar.
    pub calendar: CollectionId,
    /// Which occurrence; `None`: an event that doesn't repeat, as itself.
    pub occurrence: Option<RecurrenceKey>,
    /// The JSCalendar `Event` of the occurrence, or the event.
    pub members: Map<String, Value>,
}

/// Whether an event's details may be shown (draft-ietf-jmap-calendars,
/// `Principal/getAvailability`: not when `privacy` is `private`; `secret`,
/// iCalendar's `CLASS:CONFIDENTIAL`, neither). No `privacy` is public.
fn is_public(members: &Map<String, Value>) -> bool {
    match members.get("privacy") {
        None | Some(Value::Null) => true,
        Some(Value::String(privacy)) => privacy == "public",
        Some(_) => false,
    }
}

/// One stored object to read: where it is, its zone and its text.
type Text = (ResourceId, CollectionId, Zone, String);

/// The busy periods of a batch of objects (CPU work), the first `room`
/// with their event when public; and how many events it tried to give
/// (each try counts, given or not: the work stays within `room`). Only
/// occurrences whose `CLASS` allows it are converted.
fn read_batch(
    texts: Vec<Text>,
    own: &[String],
    range: (i64, i64),
    mut room: usize,
) -> (Vec<OwnPeriod>, usize) {
    let mut out = Vec::new();
    let mut tried = 0_usize;
    for (resource, calendar, zone, text) in texts {
        let Ok(parsed) = ParsedCalendar::parse(text.as_bytes()) else {
            continue;
        };
        let occurrences = own_busy_occurrences(&parsed, zone, range, own).unwrap_or_default();
        let series = parsed.is_series();
        for busy in occurrences {
            let options = EventOptions::default();
            let mut event = None;
            if busy.shareable && room > 0 {
                room = room.saturating_sub(1);
                tried = tried.saturating_add(1);
                event = if series {
                    parsed.occurrence(busy.key, zone, &options)
                } else {
                    parsed.base_event(zone, &options)
                }
                .ok()
                .flatten()
                .filter(|json| is_public(&json.members))
                .map(|json| OwnEvent {
                    resource,
                    calendar,
                    occurrence: series.then_some(busy.key),
                    members: json.members,
                });
            }
            out.push(OwnPeriod {
                period: busy.period,
                event,
            });
        }
    }
    (out, tried)
}

impl GroupwareAccount {
    /// The user's busy periods between `from` and `to` (UTC seconds), from
    /// the local copy of their own calendars. `own`: the user's addresses
    /// (without `mailto:`), to find their answer to invitations. Sorted, at
    /// most [`MAX_BUSY_PERIODS`]; empty when `to <= from`.
    ///
    /// # Errors
    /// [`GroupwareError::Store`] on database errors;
    /// [`GroupwareError::Task`] if a blocking task fails.
    pub async fn own_free_busy(
        &self,
        own: &[String],
        from: i64,
        to: i64,
    ) -> Result<Vec<BusyPeriod>, GroupwareError> {
        let mut periods: Vec<BusyPeriod> = self
            .own_availability(own, (from, to), false)
            .await?
            .into_iter()
            .map(|found| found.period)
            .collect();
        periods.dedup();
        Ok(periods)
    }

    /// [`Self::own_free_busy`] with, when `details`, each period's event
    /// (`showDetails`): only public ones, at most [`MAX_DETAILED`]. Sorted
    /// by period.
    ///
    /// # Errors
    /// As [`Self::own_free_busy`].
    pub async fn own_availability(
        &self,
        own: &[String],
        (from, to): (i64, i64),
        details: bool,
    ) -> Result<Vec<OwnPeriod>, GroupwareError> {
        if to <= from {
            return Ok(Vec::new());
        }
        let store = self.store();
        let zones: HashMap<_, Zone> = store
            .dav_collections(self.id)
            .await?
            .into_iter()
            .filter(|collection| {
                collection.props.kind == CollectionKind::Calendar
                    && collection.orphaned_at.is_none()
                    && collection.props.sharing.owner.is_none()
                    && collection
                        .props
                        .components
                        .is_none_or(|components| components.contains(Components::EVENT))
            })
            .map(|collection| {
                (
                    collection.id,
                    zone_or_utc(collection.props.time_zone.as_deref()),
                )
            })
            .collect();
        if zones.is_empty() {
            return Ok(Vec::new());
        }
        let near = (
            UnixTime::from_secs(from).saturating_sub(FLOATING),
            UnixTime::from_secs(to).saturating_add(FLOATING),
        );
        let mut found: Vec<_> = store
            .visible_resources(
                self.id,
                ResourceQuery {
                    overlapping: Some(near),
                    ..ResourceQuery::all(CollectionKind::Calendar)
                },
            )
            .await?
            .into_iter()
            .filter(|resource| zones.contains_key(&resource.collection))
            .collect();
        found.sort_by_key(|resource| std::cmp::Reverse(resource.id));
        if found.len() > MAX_EVENTS {
            tracing::warn!(
                account = %self.id,
                events = found.len(),
                max = MAX_EVENTS,
                "too many events for free-busy: only the newest are counted"
            );
            found.truncate(MAX_EVENTS);
        }
        let own: Vec<String> = own.iter().map(|a| a.trim().to_ascii_lowercase()).collect();
        let mut periods: Vec<OwnPeriod> = Vec::new();
        let mut detailed = 0_usize;
        for batch in found.chunks(BATCH) {
            let mut texts = Vec::with_capacity(batch.len());
            for resource in batch {
                match self.resource_text(resource.id).await {
                    Ok(Some(stored)) => texts.push((
                        resource.id,
                        resource.collection,
                        zones
                            .get(&resource.collection)
                            .copied()
                            .unwrap_or(Zone::UTC),
                        stored.text,
                    )),
                    Ok(None) => {}
                    // One event that does not open never stops the others.
                    Err(error) => tracing::debug!(%error, "event skipped for free-busy"),
                }
            }
            let own = own.clone();
            let room = if details {
                MAX_DETAILED.saturating_sub(detailed)
            } else {
                0
            };
            let (busy, tried) =
                tokio::task::spawn_blocking(move || read_batch(texts, &own, (from, to), room))
                    .await
                    .map_err(|_| GroupwareError::Task)?;
            detailed = detailed.saturating_add(tried);
            periods.extend(busy);
            if periods.len() >= MAX_BUSY_PERIODS {
                break;
            }
        }
        periods.sort_by(|a, b| {
            a.period.cmp(&b.period).then_with(|| {
                let at = |p: &OwnPeriod| p.event.as_ref().map(|e| (e.resource, e.occurrence));
                at(a).cmp(&at(b))
            })
        });
        periods.truncate(MAX_BUSY_PERIODS);
        Ok(periods)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(class: &str, count: u32) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
             UID:{class}\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:20261019T100000Z\r\n\
             DURATION:PT1H\r\nRRULE:FREQ=DAILY;COUNT={count}\r\nCLASS:{class}\r\n\
             SUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        )
    }

    fn text(n: i64, body: String) -> Text {
        (
            ResourceId::from_raw(n).unwrap(),
            CollectionId::from_raw(1).unwrap(),
            Zone::UTC,
            body,
        )
    }

    const RANGE: (i64, i64) = (1_792_000_000, 1_900_000_000);

    /// Private occurrences are counted but never converted: no try.
    #[test]
    fn private_events_are_not_converted() {
        let (periods, tried) = read_batch(vec![text(1, object("PRIVATE", 500))], &[], RANGE, 10);
        assert_eq!(periods.len(), 500);
        assert!(periods.iter().all(|p| p.event.is_none()));
        assert_eq!(tried, 0);
        let (periods, tried) = read_batch(vec![text(1, object("PUBLIC", 500))], &[], RANGE, 10);
        assert_eq!(periods.iter().filter(|p| p.event.is_some()).count(), 10);
        assert_eq!(tried, 10);
    }

    /// Each try uses up the room, whatever comes out of it.
    #[test]
    fn tries_never_pass_the_room() {
        let texts = vec![
            text(1, object("CONFIDENTIAL", 50)),
            text(2, object("PRIVATE", 50)),
            text(3, object("PUBLIC", 50)),
            text(4, object("X-OTHER", 50)),
        ];
        let (periods, tried) = read_batch(texts, &[], RANGE, 7);
        assert_eq!(periods.len(), 200);
        assert_eq!(tried, 7);
        assert_eq!(periods.iter().filter(|p| p.event.is_some()).count(), 7);
    }
}
