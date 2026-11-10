// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendar defaults and queries against a real DAV server:
//! a calendar's time zone and default alerts (CalDAV
//! `calendar-timezone(-id)` and `default-alarm-vevent-*`), events created
//! with `useDefaultAlerts`, the text filters of `CalendarEvent/query` and
//! `ContactCard/query`, and both `/queryChanges`. Stalwart or Radicale
//! with `NEBULA_TEST_DAV=1`, Baïkal with `NEBULA_TEST_BAIKAL=1`:
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=radicale cargo test -p nebula-jmap --test server defaults_and_queries -- --test-threads=1
//! ```
//!
//! Nothing here has participants, so no mail is sent. Everything is in a
//! new calendar and address book with unique names, deleted at the end.

use std::time::Duration;

use nebula_dav::proto::Href;
use nebula_dav::{Credentials, DavClient, DavServer, ResourceKind};
use nebula_http::{AccountContext, ApiCall, Backend};
use nebula_jmap::JmapBackend;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_store::Store;
use nebula_tests::{DavKind, DavTestServer, ServerKind, TestServer, User};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Under {
    Stalwart,
    Radicale,
    Baikal,
    Nextcloud,
}

impl Under {
    fn selected() -> Option<Self> {
        if std::env::var("NEBULA_TEST_BAIKAL").is_ok_and(|v| v == "1") {
            return Some(Self::Baikal);
        }
        Some(match DavTestServer::selected()?.kind {
            DavKind::Stalwart => Self::Stalwart,
            DavKind::Radicale => Self::Radicale,
            DavKind::Nextcloud => Self::Nextcloud,
        })
    }

    fn base_url(self) -> String {
        match self {
            Self::Stalwart => DavTestServer {
                kind: DavKind::Stalwart,
            }
            .base_url(),
            Self::Radicale => DavTestServer {
                kind: DavKind::Radicale,
            }
            .base_url(),
            Self::Nextcloud => DavTestServer {
                kind: DavKind::Nextcloud,
            }
            .base_url(),
            Self::Baikal => format!("http://{}:5233/dav.php/", nebula_tests::HOST),
        }
    }

    fn mail(self) -> TestServer {
        TestServer::of(match self {
            Self::Stalwart => ServerKind::Stalwart,
            Self::Radicale | Self::Baikal | Self::Nextcloud => ServerKind::Dovecot,
        })
    }
}

async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let body = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:calendars",
            "urn:ietf:params:jmap:contacts"
        ],
        "methodCalls": calls,
    });
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 1_000_000,
            max_calls_in_request: 32,
        },
        &[
            Capability::Core,
            Capability::Calendars,
            Capability::Contacts,
        ],
    )
    .unwrap();
    let response = backend
        .jmap(
            account,
            ApiCall {
                request,
                // Push subscriptions are per session; nothing else looks at it.
                session: nebula_store::GrantId::from_raw(1).unwrap(),
                session_state: State::new("s"),
                languages: nebula_http::AcceptLanguage::default(),
                limits: MethodLimits {
                    max_objects_in_get: 500,
                    max_objects_in_set: 500,
                    max_size_request: 10_000_000,
                },
            },
        )
        .await
        .unwrap();
    serde_json::to_value(response).unwrap()["methodResponses"]
        .as_array()
        .unwrap()
        .clone()
}

async fn one(backend: &JmapBackend, account: &AccountContext, method: &str, args: Value) -> Value {
    let responses = call(backend, account, json!([[method, args, "0"]])).await;
    assert_eq!(responses[0][0], method, "{responses:?}");
    responses[0][1].clone()
}

/// The error of a call that must fail.
async fn error(
    backend: &JmapBackend,
    account: &AccountContext,
    method: &str,
    args: Value,
) -> String {
    let responses = call(backend, account, json!([[method, args, "0"]])).await;
    assert_eq!(responses[0][0], "error", "{responses:?}");
    responses[0][1]["type"].as_str().unwrap().to_owned()
}

fn created(response: &Value, key: &str) -> String {
    response["created"][key]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{key} not created: {response}"))
        .to_owned()
}

fn ids(response: &Value) -> Vec<String> {
    response["ids"]
        .as_array()
        .unwrap_or_else(|| panic!("no ids: {response}"))
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect()
}

async fn wait_for_groupware(backend: &JmapBackend, account: &AccountContext) {
    for _ in 0..200 {
        let capabilities = backend.account_capabilities(account).await.unwrap();
        if capabilities.calendars.is_some() && capabilities.contacts.is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("calendars and contacts never announced");
}

/// The collection `id` (`c…`/`a…`) on the DAV server.
async fn collection_href(store: &Store, account: &AccountContext, id: &str) -> Href {
    let row: i64 = id.get(1..).unwrap().parse().unwrap();
    let found = store
        .dav_collections(account.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id.get() == row)
        .unwrap();
    Href::from_url(&url::Url::parse(found.props.href.as_str()).unwrap()).unwrap()
}

/// What another client sees of the calendar.
async fn remote_calendar(other: &DavClient, href: &Href) -> nebula_dav::Collection {
    let home = &other.discover().await.unwrap().calendar_homes[0];
    other
        .list_collections(home)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.href == *href)
        .expect("the calendar on the server")
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one session, step by step")]
async fn defaults_and_queries() {
    let Some(under) = Under::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = super::contacts::backend_with(
        &store,
        &under.base_url(),
        under.mail(),
        Duration::from_hours(1),
    );
    let alice = super::start(&backend, &store, under.mail(), User::Alice).await;
    let a = alice.jmap_id.as_str().to_owned();
    wait_for_groupware(&backend, &alice).await;
    let other = DavClient::new(
        DavServer::new(&under.base_url())
            .unwrap()
            .with_private_network(true),
        Some(Credentials::new(User::Alice.login(), under.mail().password(User::Alice)).unwrap()),
    )
    .unwrap();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("nebula-test-defaults-{nanos}");

    // A calendar with a zone and default alerts.
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "create": {"c": {
            "name": name, "timeZone": "Europe/Madrid",
            "defaultAlertsWithTime": {"ten": {"@type": "Alert", "trigger": {"@type": "OffsetTrigger", "offset": "-PT10M"}}},
            "defaultAlertsWithoutTime": {"eve": {"trigger": {"offset": "-PT9H"}, "action": "display"}}
        }}}),
    )
    .await;
    let calendar = created(&set, "c");
    let get = one(
        &backend,
        &alice,
        "Calendar/get",
        json!({"accountId": a, "ids": [calendar], "properties": ["timeZone", "defaultAlertsWithTime", "defaultAlertsWithoutTime"]}),
    )
    .await;
    let got = &get["list"][0];
    assert_eq!(got["timeZone"], "Europe/Madrid", "{get}");
    assert_eq!(
        got["defaultAlertsWithTime"]["ten"]["trigger"]["offset"], "-PT10M",
        "{get}"
    );
    assert_eq!(
        got["defaultAlertsWithoutTime"]["eve"]["trigger"]["offset"], "-PT9H",
        "{get}"
    );
    let href = collection_href(&store, &alice, &calendar).await;
    let remote = remote_calendar(&other, &href).await;
    // Stalwart keeps only the zone property set last (the id).
    assert!(
        remote.timezone_id.as_deref() == Some("Europe/Madrid")
            || remote
                .timezone
                .as_deref()
                .is_some_and(|t| t.contains("TZID:Europe/Madrid")),
        "{remote:?}"
    );
    assert!(
        remote
            .default_alarms_datetime
            .as_deref()
            .is_some_and(|t| t.contains("TRIGGER:-PT10M")),
        "{remote:?}"
    );

    // Patch one alert, change the zone; then remove the zone.
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "update": {calendar.clone(): {
            "defaultAlertsWithTime/end": {"trigger": {"offset": "PT0S", "relativeTo": "end"}},
            "timeZone": "America/New_York"
        }}}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let get = one(
        &backend,
        &alice,
        "Calendar/get",
        json!({"accountId": a, "ids": [calendar], "properties": ["timeZone", "defaultAlertsWithTime"]}),
    )
    .await;
    let got = &get["list"][0];
    assert_eq!(got["timeZone"], "America/New_York", "{get}");
    let alerts = got["defaultAlertsWithTime"].as_object().unwrap();
    assert_eq!(alerts.len(), 2, "{get}");
    assert_eq!(alerts["end"]["trigger"]["relativeTo"], "end");
    let bad = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "update": {calendar.clone(): {"timeZone": "Mars/Olympus"}}}),
    )
    .await;
    assert_eq!(
        bad["notUpdated"][&calendar]["properties"],
        json!(["timeZone"]),
        "{bad}"
    );

    // New calendars with the zones browsers give (the web sends the
    // browser's zone when it creates one): old names
    // (Asia/Calcutta), new ones (Europe/Kyiv), three parts and UTC, which
    // comes back as Etc/UTC (as Stalwart's JMAP gives it).
    for (i, (zone, expected)) in [
        ("Asia/Calcutta", "Asia/Calcutta"),
        ("Europe/Kyiv", "Europe/Kyiv"),
        (
            "America/Argentina/Buenos_Aires",
            "America/Argentina/Buenos_Aires",
        ),
        ("UTC", "Etc/UTC"),
    ]
    .into_iter()
    .enumerate()
    {
        let set = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "create": {"z": {"name": format!("{name}-zone{i}"), "timeZone": zone}}}),
        )
        .await;
        let zoned = created(&set, "z");
        let get = one(
            &backend,
            &alice,
            "Calendar/get",
            json!({"accountId": a, "ids": [zoned], "properties": ["timeZone"]}),
        )
        .await;
        assert_eq!(get["list"][0]["timeZone"], expected, "{under:?} {get}");
        let href = collection_href(&store, &alice, &zoned).await;
        let remote = remote_calendar(&other, &href).await;
        assert!(
            [zone, expected].into_iter().any(|id| {
                remote.timezone_id.as_deref() == Some(id)
                    || remote
                        .timezone
                        .as_deref()
                        .is_some_and(|t| t.contains(&format!("TZID:{id}")))
            }),
            "{under:?} {zone}: {remote:?}"
        );
        let gone = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "destroy": [zoned]}),
        )
        .await;
        assert!(gone["notDestroyed"].is_null(), "{gone}");
    }

    // Events: the query state first, for queryChanges.
    let first = one(
        &backend,
        &alice,
        "CalendarEvent/query",
        json!({"accountId": a, "filter": {"inCalendar": calendar}}),
    )
    .await;
    assert_eq!(first["canCalculateChanges"], true, "{first}");
    let since = first["queryState"].as_str().unwrap().to_owned();
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "create": {
            "timed": {"calendarIds": {calendar.clone(): true}, "title": "Reunión de presupuesto",
                      "start": "2026-11-02T10:00:00", "timeZone": "Europe/Madrid", "duration": "PT1H",
                      "locations": {"l": {"@type": "Location", "name": "Sala Ébano"}}, "useDefaultAlerts": true,
                      "alerts": {"x": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT1M"}}}},
            "day": {"calendarIds": {calendar.clone(): true}, "title": "Cumpleaños de José",
                    "start": "2026-11-03T00:00:00", "showWithoutTime": true, "duration": "P1D",
                    "useDefaultAlerts": true}
        }}),
    )
    .await;
    let timed = created(&set, "timed");
    let day = created(&set, "day");
    let get = one(
        &backend,
        &alice,
        "CalendarEvent/get",
        json!({"accountId": a, "ids": [timed, day], "properties": ["alerts", "title"]}),
    )
    .await;
    let list = get["list"].as_array().unwrap();
    let alerts_of = |id: &str| {
        list.iter().find(|e| e["id"] == id).unwrap()["alerts"]
            .as_object()
            .cloned()
            .unwrap_or_default()
    };
    let timed_alerts = alerts_of(&timed);
    assert_eq!(timed_alerts.len(), 2, "{get}");
    assert!(
        timed_alerts.contains_key("ten") && timed_alerts.contains_key("end"),
        "{get}"
    );
    let day_alerts = alerts_of(&day);
    assert_eq!(day_alerts["eve"]["trigger"]["offset"], "-PT9H", "{get}");

    // Text filters, without case or accents.
    for (filter, expected) in [
        (
            json!({"inCalendar": calendar, "title": "reunion"}),
            vec![timed.clone()],
        ),
        (
            json!({"inCalendar": calendar, "text": "EBANO"}),
            vec![timed.clone()],
        ),
        (
            json!({"inCalendar": calendar, "title": "jose"}),
            vec![day.clone()],
        ),
        (
            json!({"inCalendar": calendar, "description": "reunion"}),
            vec![],
        ),
        (
            json!({"operator": "AND", "conditions": [{"inCalendar": calendar}, {"operator": "NOT", "conditions": [{"title": "presupuesto"}]}]}),
            vec![day.clone()],
        ),
    ] {
        let found = one(
            &backend,
            &alice,
            "CalendarEvent/query",
            json!({"accountId": a, "filter": filter}),
        )
        .await;
        assert_eq!(ids(&found), expected, "{filter}: {found}");
    }
    // Expanded, with a text filter.
    let found = one(
        &backend,
        &alice,
        "CalendarEvent/query",
        json!({"accountId": a, "expandRecurrences": true, "timeZone": "Europe/Madrid",
               "filter": {"inCalendar": calendar, "after": "2026-11-01T00:00:00", "before": "2026-11-08T00:00:00", "title": "cumple"}}),
    )
    .await;
    assert_eq!(found["canCalculateChanges"], false, "{found}");
    assert_eq!(ids(&found).len(), 1, "{found}");

    // queryChanges: both new events added, in order.
    let changes = one(
        &backend,
        &alice,
        "CalendarEvent/queryChanges",
        json!({"accountId": a, "filter": {"inCalendar": calendar}, "sinceQueryState": since, "calculateTotal": true}),
    )
    .await;
    assert_eq!(changes["total"], 2, "{changes}");
    let added: Vec<&str> = changes["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    let now = one(
        &backend,
        &alice,
        "CalendarEvent/query",
        json!({"accountId": a, "filter": {"inCalendar": calendar}}),
    )
    .await;
    assert_eq!(added, ids(&now), "{changes}");
    assert_eq!(added.len(), 2, "{changes}");
    assert_eq!(changes["removed"], json!([]), "{changes}");
    let state = changes["newQueryState"].as_str().unwrap().to_owned();
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "destroy": [day]}),
    )
    .await;
    assert!(set["notDestroyed"].is_null(), "{set}");
    let changes = one(
        &backend,
        &alice,
        "CalendarEvent/queryChanges",
        json!({"accountId": a, "filter": {"inCalendar": calendar}, "sinceQueryState": state}),
    )
    .await;
    assert_eq!(changes["removed"], json!([day]), "{changes}");
    assert_eq!(changes["added"], json!([]), "{changes}");
    assert_eq!(
        error(
            &backend,
            &alice,
            "CalendarEvent/queryChanges",
            json!({"accountId": a, "expandRecurrences": true, "sinceQueryState": state,
                   "filter": {"after": "2026-11-01T00:00:00", "before": "2026-11-08T00:00:00"}}),
        )
        .await,
        "cannotCalculateChanges"
    );

    // The timed event's VALARMs are on the server.
    let event_row: i64 = timed.strip_prefix('v').unwrap().parse().unwrap();
    let stored = store
        .dav_resource(
            alice.id,
            nebula_store::ResourceId::from_raw(event_row).unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    let event_href = Href::from_url(&url::Url::parse(stored.href.as_str()).unwrap()).unwrap();
    let text = other
        .get(&event_href, ResourceKind::Calendar)
        .await
        .unwrap()
        .data;
    assert!(
        text.contains("-PT10M") && !text.contains("-PT1M\r"),
        "{text}"
    );

    // Contacts: text filters and queryChanges.
    let book_name = format!("{name}-book");
    let set = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "create": {"b": {"name": book_name}}}),
    )
    .await;
    let book = created(&set, "b");
    let first = one(
        &backend,
        &alice,
        "ContactCard/query",
        json!({"accountId": a, "filter": {"inAddressBook": book}}),
    )
    .await;
    assert_eq!(first["canCalculateChanges"], true, "{first}");
    let since = first["queryState"].as_str().unwrap().to_owned();
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "create": {
            "jose": {"@type": "Card", "version": "1.0", "addressBookIds": {book.clone(): true},
                     "name": {"components": [{"kind": "given", "value": "José"}, {"kind": "surname", "value": "Müller"}]},
                     "emails": {"e": {"address": "jose@example.com"}},
                     "phones": {"p": {"number": "+34 600 11 22 33"}}},
            "ana": {"@type": "Card", "version": "1.0", "addressBookIds": {book.clone(): true},
                    "name": {"full": "Ana Pérez"}, "organizations": {"o": {"name": "Ácme"}}}
        }}),
    )
    .await;
    let jose = created(&set, "jose");
    let ana = created(&set, "ana");
    for (filter, expected) in [
        (
            json!({"inAddressBook": book, "text": "muller"}),
            vec![jose.clone()],
        ),
        (
            json!({"inAddressBook": book, "name/given": "JOSE"}),
            vec![jose.clone()],
        ),
        (
            json!({"inAddressBook": book, "phone": "600112233"}),
            vec![jose.clone()],
        ),
        (
            json!({"inAddressBook": book, "organization": "acme"}),
            vec![ana.clone()],
        ),
        (
            json!({"inAddressBook": book, "email": "example.com"}),
            vec![jose.clone()],
        ),
        (json!({"inAddressBook": book, "kind": "group"}), vec![]),
    ] {
        let found = one(
            &backend,
            &alice,
            "ContactCard/query",
            json!({"accountId": a, "filter": filter}),
        )
        .await;
        assert_eq!(ids(&found), expected, "{filter}: {found}");
    }
    let changes = one(
        &backend,
        &alice,
        "ContactCard/queryChanges",
        json!({"accountId": a, "filter": {"inAddressBook": book, "text": "perez"}, "sinceQueryState": since}),
    )
    .await;
    assert_eq!(
        changes["added"],
        json!([{"id": ana, "index": 0}]),
        "{changes}"
    );
    assert_eq!(
        error(
            &backend,
            &alice,
            "ContactCard/queryChanges",
            json!({"accountId": a, "sinceQueryState": "nope"}),
        )
        .await,
        "cannotCalculateChanges"
    );

    // Clean up.
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Calendar/set", {"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}, "0"],
            ["AddressBook/set", {"accountId": a, "destroy": [book], "onDestroyRemoveContents": true}, "1"]
        ]),
    )
    .await;
    assert!(responses[0][1]["notDestroyed"].is_null(), "{responses:?}");
    assert!(responses[1][1]["notDestroyed"].is_null(), "{responses:?}");
}
