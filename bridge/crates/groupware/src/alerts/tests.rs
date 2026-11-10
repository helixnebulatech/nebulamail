// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The scheduler with a manual clock, on events put straight into the
//! store (no DAV server: the account is "discovered" by hand).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nebula_crypto::{AccountId as KeyId, AccountKey, DataKey};
use nebula_dav::proto::Href;
use nebula_dav::{DavClient, DavServer, ResourceKind};
use nebula_store::{
    CollectionId, CollectionKind, CollectionProps, DataType, DavHref, NewAccount, Recorded, Store,
    UnixTime,
};
use tokio::sync::{broadcast, watch};
use url::Url;

use super::task::{Schedule, fire_due};
use super::*;
use crate::core::account::{Discovered, GroupwareAccount, Shared};
use crate::model::index::resource_write;
use crate::{GroupwareConfig, GroupwareEvent, StateChanged};

/// 2026-10-19T08:00:00Z (10:00 in Madrid).
const T0: i64 = 1_792_396_800;
const BASE: &str = "http://127.0.0.1:9/dav/cal/alice/work/";

struct Setup {
    store: Store,
    account: Arc<GroupwareAccount>,
    collection: CollectionId,
    clock: ManualClock,
    events: broadcast::Receiver<GroupwareEvent>,
    _dir: tempfile::TempDir,
}

fn key() -> AccountKey {
    AccountKey::new(
        KeyId::new(b"login:alice").unwrap(),
        DataKey::generate().unwrap(),
    )
}

async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id = store
        .get_or_create_account(
            NewAccount {
                login: "alice",
                email: "alice@example.com",
                wrapped_dek: b"k",
            },
            UnixTime::from_secs(T0),
        )
        .await
        .unwrap()
        .id;
    let (_, live) = store
        .apply_collections(
            id,
            vec![CollectionProps {
                kind: CollectionKind::Calendar,
                href: DavHref::new(BASE).unwrap(),
                name: Some("Work".to_owned()),
                color: None,
                description: None,
                sort_order: None,
                time_zone: Some("Europe/Madrid".to_owned()),
                default_alarms_datetime: None,
                default_alarms_date: None,
                rights: None,
                components: None,
                vcard_versions: None,
                supports_sync: true,
                sharing: nebula_store::CollectionSharing::default(),
                extras: nebula_store::CalendarExtras::default(),
            }],
            UnixTime::from_secs(T0),
        )
        .await
        .unwrap();
    let clock = ManualClock::new(T0);
    let server = DavServer::new("http://127.0.0.1:9/dav/").unwrap();
    let mut config = GroupwareConfig::new(server.clone());
    config.alerts.clock = Arc::new(clock.clone());
    config.alerts.tick = Duration::from_millis(10);
    let (events, receiver) = broadcast::channel(64);
    let shared = Arc::new(Shared {
        store: store.clone(),
        config,
        events,
        watchers: Mutex::new(HashMap::new()),
        alert_listeners: Mutex::new(HashMap::new()),
    });
    let resolver = Arc::new(net_guard::SystemResolver::new().unwrap());
    let client = DavClient::with_resolver(server, None, resolver).unwrap();
    let account = Arc::new(GroupwareAccount::new(id, shared, client, None, key()));
    account.set_discovered(Discovered {
        calendar_homes: vec![Href::from_url(&Url::parse(BASE).unwrap()).unwrap()],
        addressbook_homes: Vec::new(),
        auto_schedule: false,
        user_addresses: Vec::new(),
        principal: Href::from_url(&Url::parse(BASE).unwrap()).unwrap(),
        principal_name: None,
        principal_type: None,
        principal_collections: Vec::new(),
        schedule_outbox: None,
        sharing: crate::sharing::SharingSupport::default(),
        contacts_principal: Href::from_url(&Url::parse(BASE).unwrap()).unwrap(),
        contacts_sharing: crate::sharing::SharingSupport::default(),
        nextcloud: crate::core::account::NextcloudSupport::default(),
        at: UnixTime::from_secs(T0),
    });
    Setup {
        store,
        account,
        collection: live[0].id,
        clock,
        events: receiver,
        _dir: dir,
    }
}

impl Setup {
    /// Puts (or replaces) `name` with these `VEVENT` lines, and says so
    /// like a sync round would.
    async fn put(&self, name: &str, lines: &str) {
        self.put_in(self.collection, BASE, name, lines).await;
    }

    /// [`Self::put`] in another collection (at `base`).
    async fn put_in(&self, collection: CollectionId, base: &str, name: &str, lines: &str) {
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\nUID:{name}@example.com\r\n\
             DTSTAMP:20261001T000000Z\r\n{}END:VEVENT\r\nEND:VCALENDAR\r\n",
            lines.replace('\n', "\r\n")
        );
        let href = Href::from_url(&Url::parse(&format!("{base}{name}.ics")).unwrap()).unwrap();
        let write = resource_write(
            &self.account.key,
            collection,
            ResourceKind::Calendar,
            &href,
            Some(format!("\"{}\"", text.len())),
            &text,
            1000,
        )
        .unwrap();
        let recorded: Recorded = self
            .store
            .apply_resources(
                self.account.id,
                collection,
                vec![write],
                vec![],
                UnixTime::from_secs(self.clock.now()),
            )
            .await
            .unwrap();
        self.account.emit(&recorded);
    }

    /// `count` event sources that asked for `CalendarAlert` (watchers
    /// and alert listeners).
    fn watch(&self, count: usize) {
        self.watch_only(count);
        self.account
            .shared
            .alert_listeners
            .lock()
            .unwrap()
            .insert(self.account.id, count);
        self.account.alerts_wake.notify_one();
    }

    /// `count` event sources that did not ask for `CalendarAlert`.
    fn watch_only(&self, count: usize) {
        self.account
            .shared
            .watchers
            .lock()
            .unwrap()
            .insert(self.account.id, count);
    }

    /// The alerts broadcast so far.
    fn sent(&mut self) -> Vec<AlertDue> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            if let GroupwareEvent::CalendarAlert(due) = event {
                out.push(due);
            }
        }
        out
    }

    async fn schedule(&self) -> Schedule {
        let mut schedule = Schedule::new();
        schedule.queue = plan::plan(&self.account, self.clock.now())
            .await
            .unwrap()
            .into();
        schedule
    }
}

const TEN: &str = "DTSTART;TZID=Europe/Madrid:20261019T103000\nDURATION:PT30M\nSUMMARY:x\n\
    BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT15M\nEND:VALARM\n\
    BEGIN:VALARM\nACTION:EMAIL\nDESCRIPTION:x\nSUMMARY:x\nATTENDEE:mailto:a@example.com\n\
    TRIGGER:-PT10M\nEND:VALARM\n";

#[tokio::test(flavor = "multi_thread")]
async fn fires_once_and_not_again_after_a_restart() {
    let mut s = setup().await;
    s.put("ten", TEN).await;
    let mut schedule = s.schedule().await;
    // One display alert at 08:15 UTC (10:15 Madrid); the email one is not
    // the bridge's.
    assert_eq!(schedule.queue.len(), 1);
    assert_eq!(schedule.queue[0].fire_at, T0 + 15 * 60);
    fire_due(&s.account, &mut schedule, T0).await;
    assert!(s.sent().is_empty());
    // Due, but nobody listens: it waits.
    let due = T0 + 15 * 60;
    fire_due(&s.account, &mut schedule, due).await;
    assert!(s.sent().is_empty());
    assert_eq!(schedule.queue.len(), 1);
    // An event source opens a minute later: out it goes.
    s.watch(1);
    fire_due(&s.account, &mut schedule, due + 60).await;
    let sent = s.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].alert_id, "k1");
    assert_eq!(sent[0].uid, "ten@example.com");
    assert_eq!(sent[0].recurrence, None);
    assert!(schedule.queue.is_empty());
    // A new plan (a restart) does not have it any more.
    let mut again = s.schedule().await;
    assert!(again.queue.is_empty());
    fire_due(&s.account, &mut again, due + 61).await;
    assert!(s.sent().is_empty());
}

/// A Web Push listener is enough for an alert to go out (with every tab
/// closed there is no event source).
#[tokio::test(flavor = "multi_thread")]
async fn web_push_listeners_hear_alerts() {
    let mut s = setup().await;
    s.put("ten", TEN).await;
    let mut schedule = s.schedule().await;
    let due = T0 + 15 * 60;
    fire_due(&s.account, &mut schedule, due).await;
    assert!(s.sent().is_empty());
    s.account
        .shared
        .alert_listeners
        .lock()
        .unwrap()
        .insert(s.account.id, 1);
    fire_due(&s.account, &mut schedule, due + 1).await;
    assert_eq!(s.sent().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn late_alerts_only_within_the_grace() {
    let mut s = setup().await;
    s.put("ten", TEN).await;
    s.put(
        "later",
        "DTSTART:20261019T090000Z\nDURATION:PT1H\nSUMMARY:x\n\
         BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT5M\nEND:VALARM\n",
    )
    .await;
    s.watch(1);
    // The bridge comes back at 08:40: the 08:15 alert is 25 min late
    // (dropped), the 08:55 one is ahead.
    s.clock.set(T0 + 40 * 60);
    let mut schedule = s.schedule().await;
    assert_eq!(schedule.queue.len(), 1);
    assert_eq!(schedule.queue[0].fire_at, T0 + 55 * 60);
    // At 09:04 (9 min late, within the 10 min): sent.
    fire_due(&s.account, &mut schedule, T0 + 64 * 60).await;
    assert_eq!(s.sent().len(), 1);

    // 11 min late: dropped.
    let mut s = setup().await;
    s.put("ten", TEN).await;
    s.watch(1);
    let mut schedule = s.schedule().await;
    fire_due(&s.account, &mut schedule, T0 + 26 * 60).await;
    assert!(s.sent().is_empty());
    assert!(schedule.queue.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_task_follows_the_clock_and_the_changes() {
    let mut s = setup().await;
    s.watch(1);
    s.put(
        "weekly",
        "DTSTART;TZID=Europe/Madrid:20261019T103000\nDURATION:PT30M\nRRULE:FREQ=DAILY;COUNT=3\n\
         SUMMARY:x\nBEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT15M\nEND:VALARM\n",
    )
    .await;
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(Arc::clone(&s.account), stopped));
    let mut events = s.account.shared.events.subscribe();
    let next = async |events: &mut broadcast::Receiver<GroupwareEvent>| loop {
        if let GroupwareEvent::CalendarAlert(due) =
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap()
        {
            return due;
        }
    };
    s.clock.set(T0 + 15 * 60);
    let first = next(&mut events).await;
    assert_eq!(first.alert_id, "k1");
    // The occurrence of a series: its wall-clock start in Madrid.
    assert_eq!(
        first.recurrence.map(RecurrenceKey::naive),
        Some(T0 + 2 * 3600 + 30 * 60)
    );
    // The user snoozes it (as the web does): acknowledged now, and a new
    // alert in 5 minutes. The next plan sees it.
    s.put(
        "weekly",
        "DTSTART;TZID=Europe/Madrid:20261019T103000\nDURATION:PT30M\nRRULE:FREQ=DAILY;COUNT=3\n\
         SUMMARY:x\nBEGIN:VALARM\nUID:a1\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT15M\n\
         ACKNOWLEDGED:20261019T081600Z\nEND:VALARM\n\
         BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER;VALUE=DATE-TIME:20261019T082100Z\n\
         RELATED-TO:a1\nEND:VALARM\n",
    )
    .await;
    s.clock.set(T0 + 21 * 60);
    let snoozed = next(&mut events).await;
    assert_eq!(snoozed.alert_id, "k2");
    assert_eq!(snoozed.recurrence, first.recurrence);
    // A day later: the next occurrence.
    s.clock.set(T0 + 86_400 + 15 * 60);
    // (the hourly plan: nudge it as a sync would)
    let _ = s
        .account
        .shared
        .events
        .send(GroupwareEvent::StateChanged(StateChanged {
            account: s.account.id,
            data_types: vec![DataType::CalendarEvent],
        }));
    let day_two = next(&mut events).await;
    assert_eq!(day_two.alert_id, "k1");
    assert_ne!(day_two.recurrence, first.recurrence);
    let _ = stop.send(true);
    task.await.unwrap();
    // Nothing was sent twice.
    let sent = s.sent();
    assert_eq!(sent.len(), 3, "{sent:?}");
}

const REPEATING: &str = "DTSTART:20261019T090000Z\nDURATION:PT30M\nSUMMARY:x\n\
    BEGIN:VALARM\nACTION:DISPLAY\nDESCRIPTION:x\nTRIGGER:-PT15M\nREPEAT:2\nDURATION:PT5M\nEND:VALARM\n";

/// Each ring of a repeating alarm goes out once, also after a restart.
#[tokio::test(flavor = "multi_thread")]
async fn repeated_alarms_ring_once_each() {
    let mut s = setup().await;
    s.put("rep", REPEATING).await;
    s.watch(1);
    let mut schedule = s.schedule().await;
    let times: Vec<(i64, u32)> = schedule
        .queue
        .iter()
        .map(|p| (p.fire_at, p.repetition))
        .collect();
    let first = T0 + 45 * 60;
    assert_eq!(times, [(first, 0), (first + 300, 1), (first + 600, 2)]);
    fire_due(&s.account, &mut schedule, first + 300).await;
    let sent = s.sent();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|due| due.alert_id == "k1"));
    // A restart before the last ring: only that one is left.
    let mut again = s.schedule().await;
    assert_eq!(again.queue.len(), 1);
    assert_eq!(again.queue[0].repetition, 2);
    fire_due(&s.account, &mut again, first + 600).await;
    assert_eq!(s.sent().len(), 1);
    assert!(s.schedule().await.queue.is_empty());
}

/// `max_repeats = 0`: each alarm rings once.
#[tokio::test(flavor = "multi_thread")]
async fn repeats_can_be_turned_off() {
    let s = setup().await;
    s.put("rep", REPEATING).await;
    let mut config = s.account.shared.config.alerts.clone();
    config.max_repeats = 0;
    let limits = crate::model::alarm::AlarmLimits::new(config.max_per_event, config.max_repeats);
    let text = s
        .account
        .resource_text(
            s.store
                .dav_resources(s.account.id, s.collection)
                .await
                .unwrap()[0]
                .id,
        )
        .await
        .unwrap()
        .unwrap()
        .text;
    let parsed = crate::model::calendar::ParsedCalendar::parse(text.as_bytes()).unwrap();
    let due = crate::model::alarm::alerts_in(
        &parsed,
        crate::model::calendar::Zone::UTC,
        (T0, T0 + 86_400),
        limits,
    )
    .unwrap();
    assert_eq!(due.len(), 1);
}

fn calendar(
    href: &str,
    components: Option<nebula_store::Components>,
    owner: Option<&str>,
) -> CollectionProps {
    CollectionProps {
        kind: CollectionKind::Calendar,
        href: DavHref::new(href).unwrap(),
        name: Some(href.to_owned()),
        color: None,
        description: None,
        sort_order: None,
        time_zone: Some("Europe/Madrid".to_owned()),
        default_alarms_datetime: None,
        default_alarms_date: None,
        rights: None,
        components,
        vcard_versions: None,
        supports_sync: true,
        sharing: nebula_store::CollectionSharing {
            owner: owner.map(|o| DavHref::new(o).unwrap()),
            ..nebula_store::CollectionSharing::default()
        },
        extras: nebula_store::CalendarExtras::default(),
    }
}

/// The user's own free-busy from the local copy: every own calendar with
/// events (not only the default one), not what others share with them.
#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one account with four calendars, step by step"
)]
async fn own_free_busy_from_the_local_copy() {
    use crate::model::freebusy::BusyStatus;
    const HOME: &str = "http://127.0.0.1:9/dav/cal/alice/";

    let s = setup().await;
    let other = format!("{HOME}other/");
    let tasks = format!("{HOME}tasks/");
    let shared = "http://127.0.0.1:9/dav/cal/bob/team/";
    let (_, live) = s
        .store
        .apply_collections(
            s.account.id,
            vec![
                calendar(BASE, None, None),
                calendar(&other, Some(nebula_store::Components::EVENT), None),
                calendar(&tasks, Some(nebula_store::Components::TODO), None),
                calendar(shared, None, Some("http://127.0.0.1:9/dav/pal/bob/")),
            ],
            UnixTime::from_secs(T0),
        )
        .await
        .unwrap();
    let id = |href: &str| {
        live.iter()
            .find(|c| c.props.href.as_str() == href)
            .unwrap()
            .id
    };
    // Work (default): 10:30–11:00 Madrid and a free (transparent) one.
    s.put("ten", TEN).await;
    s.put(
        "free",
        "DTSTART:20261019T120000Z\nDURATION:PT1H\nTRANSP:TRANSPARENT\nSUMMARY:x\n",
    )
    .await;
    // Another own calendar: a daily series, one day declined.
    s.put_in(
        id(&other),
        &other,
        "daily",
        "DTSTART;TZID=Europe/Madrid:20261019T150000\nDURATION:PT1H\nRRULE:FREQ=DAILY;COUNT=3\n\
         ORGANIZER:mailto:bob@example.com\nATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.com\n\
         SUMMARY:x\n",
    )
    .await;
    s.put_in(
        id(&other),
        &other,
        "declined",
        "DTSTART:20261020T070000Z\nDURATION:PT1H\nORGANIZER:mailto:bob@example.com\n\
         ATTENDEE;PARTSTAT=DECLINED:mailto:alice@example.com\nSUMMARY:x\n",
    )
    .await;
    // An event in a tasks-only list (odd, but possible) and bob's shared
    // calendar: not alice's time.
    s.put_in(
        id(&tasks),
        &tasks,
        "t",
        "DTSTART:20261019T060000Z\nDURATION:PT1H\n",
    )
    .await;
    s.put_in(
        id(shared),
        shared,
        "bob",
        "DTSTART:20261019T050000Z\nDURATION:PT1H\n",
    )
    .await;

    let own = ["alice@example.com".to_owned()];
    let busy = s
        .account
        .own_free_busy(&own, T0 - 3600, T0 + 2 * 86_400)
        .await
        .unwrap();
    let got: Vec<(i64, i64, BusyStatus)> =
        busy.iter().map(|p| (p.start, p.end, p.status)).collect();
    let hour = 3600;
    assert_eq!(
        got,
        [
            // 10:30 Madrid = 08:30 UTC.
            (T0 + 30 * 60, T0 + hour, BusyStatus::Confirmed),
            // 15:00 Madrid = 13:00 UTC, each day in the range.
            (T0 + 5 * hour, T0 + 6 * hour, BusyStatus::Confirmed),
            (
                T0 + 86_400 + 5 * hour,
                T0 + 86_400 + 6 * hour,
                BusyStatus::Confirmed
            ),
        ]
    );
    // Cut to the range; an empty or reversed range is empty.
    let cut = s
        .account
        .own_free_busy(&own, T0 + 45 * 60, T0 + 2 * hour)
        .await
        .unwrap();
    assert_eq!(cut.len(), 1);
    assert_eq!((cut[0].start, cut[0].end), (T0 + 45 * 60, T0 + hour));
    assert!(
        s.account
            .own_free_busy(&own, T0, T0)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        s.account
            .own_free_busy(&own, T0 + 1, T0)
            .await
            .unwrap()
            .is_empty()
    );
}

/// `showDetails` (draft-ietf-jmap-calendars): each own busy period with
/// its occurrence as JSCalendar, but not for a private or confidential
/// event, and nothing when not asked for.
#[tokio::test(flavor = "multi_thread")]
async fn own_availability_with_details() {
    let s = setup().await;
    s.put(
        "public",
        "DTSTART:20261019T080000Z\nDURATION:PT1H\nSUMMARY:Planning\nDESCRIPTION:Q4\n\
         LOCATION:Room 1\nRRULE:FREQ=DAILY;COUNT=2\n",
    )
    .await;
    s.put(
        "private",
        "DTSTART:20261019T100000Z\nDURATION:PT1H\nSUMMARY:Doctor\nCLASS:PRIVATE\n",
    )
    .await;
    s.put(
        "confidential",
        "DTSTART:20261019T120000Z\nDURATION:PT1H\nSUMMARY:Interview\nCLASS:CONFIDENTIAL\n",
    )
    .await;
    let own = ["alice@example.com".to_owned()];
    let range = (T0 - 3600, T0 + 2 * 86_400);
    let found = s.account.own_availability(&own, range, true).await.unwrap();
    let hour = 3600;
    let starts: Vec<i64> = found.iter().map(|p| p.period.start).collect();
    assert_eq!(
        starts,
        [T0, T0 + 2 * hour, T0 + 4 * hour, T0 + 86_400],
        "{found:?}"
    );
    // The series: each day its own occurrence, with the event's details.
    let first = found[0].event.as_ref().expect("public: details");
    assert_eq!(first.members["title"], "Planning");
    assert_eq!(first.members["description"], "Q4");
    assert_eq!(first.calendar, s.collection);
    let second = found[3].event.as_ref().expect("public: details");
    assert_eq!(second.resource, first.resource);
    assert!(first.occurrence.is_some());
    assert_ne!(second.occurrence, first.occurrence);
    assert_eq!(second.members["title"], "Planning");
    assert!(
        !second.members.contains_key("recurrenceRule"),
        "an occurrence, not the series: {:?}",
        second.members
    );
    // Private and confidential: busy, no details.
    assert!(found[1].event.is_none(), "{:?}", found[1]);
    assert!(found[2].event.is_none(), "{:?}", found[2]);
    // Without showDetails: the same periods, no events.
    let plain = s
        .account
        .own_availability(&own, range, false)
        .await
        .unwrap();
    assert_eq!(
        plain.iter().map(|p| p.period).collect::<Vec<_>>(),
        found.iter().map(|p| p.period).collect::<Vec<_>>()
    );
    assert!(plain.iter().all(|p| p.event.is_none()));
}

/// 2000 hostile objects (many occurrences near now, 64 alarms each that
/// never ring in the window) share one budget: the plan stops at it.
#[test]
fn hostile_objects_share_the_plan_budget() {
    use crate::model::alarm::{AlarmLimits, MAX_ALARM_EVALUATIONS};
    let alarms: String = (0..64)
        .map(|_| "BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:x\r\nTRIGGER:-PT200H\r\nEND:VALARM\r\n")
        .collect();
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\nUID:h@x\r\n\
         DTSTAMP:20261001T000000Z\r\nDTSTART:20261019T080000Z\r\nDURATION:PT1M\r\n\
         RRULE:FREQ=MINUTELY;COUNT=200\r\n{alarms}END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let texts: Vec<_> = (0..2000)
        .map(|n| {
            (
                ResourceId::from_raw(n + 1).unwrap(),
                format!("h{n}@x"),
                crate::model::calendar::Zone::UTC,
                text.clone(),
            )
        })
        .collect();
    let limits = AlarmLimits::new(64, 0);
    let (planned, left) = plan::batch_alerts(texts, (T0, T0 + 86_400), limits, plan::PLAN_WORK);
    assert!(planned.is_empty());
    assert_eq!(left, 0);
    // Each object used at most its own share (200 × 64 > 10 000).
    const { assert!(MAX_ALARM_EVALUATIONS < 200 * 64) };
}

/// An event source that did not ask for `CalendarAlert` does not hear
/// alerts: a due one stays queued (not recorded as sent) until someone
/// who listens for them comes.
#[tokio::test(flavor = "multi_thread")]
async fn only_alert_listeners_hear_alerts() {
    let mut s = setup().await;
    s.put("ten", TEN).await;
    s.watch_only(1);
    let mut schedule = s.schedule().await;
    let due = T0 + 15 * 60;
    fire_due(&s.account, &mut schedule, due).await;
    assert!(s.sent().is_empty());
    assert_eq!(schedule.queue.len(), 1);
    // Not recorded: a new plan still has it.
    assert_eq!(s.schedule().await.queue.len(), 1);
    s.watch(1);
    fire_due(&s.account, &mut schedule, due + 1).await;
    assert_eq!(s.sent().len(), 1);
}

/// A burst of changes queued while the task was busy is taken at once:
/// one replan, not one per change.
#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_changes_is_one() {
    let s = setup().await;
    let mut events = s.account.shared.events.subscribe();
    for _ in 0..50 {
        let _ = s
            .account
            .shared
            .events
            .send(GroupwareEvent::StateChanged(StateChanged {
                account: s.account.id,
                data_types: vec![DataType::CalendarEvent],
            }));
    }
    let (changed, closed) = task::drain_changes(&mut events, &s.account);
    assert!(changed && !closed);
    assert!(events.try_recv().is_err());
}
