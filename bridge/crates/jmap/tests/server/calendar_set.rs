// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP calendar writes through the bridge against a real CalDAV server
//!Stalwart's DAV (with Stalwart's IMAP and
//! SMTP) or Radicale (with Dovecot's IMAP and Postfix's SMTP, so the
//! invitations the bridge sends land in Dovecot). Skipped unless
//! `NEBULA_TEST_DAV=1`:
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=radicale cargo test -p nebula-jmap --test server calendar_set -- --test-threads=1
//! ```
//!
//! The requests are the web's (apps/web/src/calendar/api.ts, editing.ts,
//! invitations.ts, alerts.ts); what they did is checked on the DAV server
//! with the DAV client as another client. Everything happens in new
//! calendars with unique names, deleted at the end.
//!
//! Invitations: with Radicale (no scheduling) the bridge mails them
//! through Postfix and bob gets them in Dovecot; with Stalwart (it
//! announces `calendar-auto-schedule`) Stalwart sends them. Stalwart
//! limits alice → bob to about 25 messages an hour: this sends two.
//! Nextcloud schedules too, but delivers to its own users' calendars
//! without mail (and the test container never sends any): bob's calendar
//! is checked instead.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_dav::proto::{ETag, Href};
use nebula_dav::{Credentials, DavClient, DavError, DavServer, PutCondition, ResourceKind};
use nebula_groupware::{Groupware, GroupwareConfig};
use nebula_http::{AccountContext, ApiCall, Backend};
use nebula_jmap::{JmapBackend, JmapConfig, SmtpSettings};
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_mail::{CertificateCheck, ImapServer, Mail, MailConfig, TlsMode};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::{ResourceId, Store};
use nebula_tests::{DavKind, DavTestServer, ServerKind, TestServer, User};
use net_guard::{NetworkAccess, Policy, Port, PortPolicy, SystemResolver, Target};
use serde_json::{Value, json};

/// The mail server of the test: Stalwart for its DAV; for Radicale and
/// Nextcloud, Dovecot's IMAP and Postfix's submission (which delivers to
/// Dovecot).
pub(super) fn mail_server(dav: DavTestServer) -> TestServer {
    match dav.kind {
        DavKind::Radicale | DavKind::Nextcloud => TestServer::of(ServerKind::Postfix),
        DavKind::Stalwart => dav.mail(),
    }
}

/// The backend. Polling is slow on purpose: the bridge sees other
/// clients' changes only when its own writes make it look.
pub(super) fn backend(store: &Store, dav: DavTestServer) -> JmapBackend {
    let server = mail_server(dav);
    let imap_port = Port::try_from(server.imap.port).unwrap();
    let smtp_port = Port::try_from(server.smtp.port).unwrap();
    let host: net_guard::Host = nebula_tests::HOST.parse().unwrap();
    let imap = ImapServer::new(host.clone(), imap_port, TlsMode::Implicit)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_private_network(true)
        .with_ports(PortPolicy::only(vec![imap_port]));
    let resolver: Arc<dyn net_guard::Resolver> = Arc::new(SystemResolver::new().unwrap());
    let mail = Mail::new(store.clone(), MailConfig::new(imap), Arc::clone(&resolver)).unwrap();
    let smtp = SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
        .with_certificates(SmtpCertificates::InsecureLoopbackOnly);
    let settings = SmtpSettings::new(
        SmtpClient::new(smtp).unwrap(),
        Target::new(host, smtp_port),
        Policy::new(
            PortPolicy::only(vec![smtp_port]),
            NetworkAccess::PrivateAllowed,
        ),
    );
    let mut config = JmapConfig::new(settings);
    config.email_domain = Some(nebula_tests::DOMAIN.to_owned());
    let mut groupware = GroupwareConfig::new(
        DavServer::new(&dav.base_url())
            .unwrap()
            .with_private_network(true),
    );
    groupware.poll_with_push = Duration::from_hours(1);
    groupware.poll_without_push = Duration::from_hours(1);
    let groupware = Groupware::new(store.clone(), groupware, Arc::clone(&resolver));
    JmapBackend::with_groupware(store.clone(), mail, Some(groupware), resolver, config)
}

/// The "other client": the DAV client on its own.
pub(super) fn other_client(dav: DavTestServer, user: User) -> DavClient {
    let config = DavServer::new(&dav.base_url())
        .unwrap()
        .with_private_network(true);
    let credentials = Credentials::new(dav.login(user), dav.password(user)).unwrap();
    DavClient::new(config, Some(credentials)).unwrap()
}

/// One request with what the web puts in `using` for calendars.
async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    call_using(backend, account, calls, false).await
}

/// The same, with the bridge's capability too (its own methods).
async fn call_using(
    backend: &JmapBackend,
    account: &AccountContext,
    calls: Value,
    bridge: bool,
) -> Vec<Value> {
    let mut using = vec![
        "urn:ietf:params:jmap:core",
        "urn:ietf:params:jmap:mail",
        "urn:ietf:params:jmap:calendars",
        "urn:ietf:params:jmap:calendars:parse",
    ];
    if bridge {
        using.push("https://helixnebula.tech/jmap/bridge");
    }
    let body = json!({
        "using": using,
        "methodCalls": calls,
    });
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 10_000_000,
            max_calls_in_request: 32,
        },
        &[
            Capability::Core,
            Capability::Mail,
            Capability::Calendars,
            Capability::CalendarsParse,
            Capability::Bridge,
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
    let value = serde_json::to_value(response).unwrap();
    value["methodResponses"].as_array().unwrap().clone()
}

async fn one(backend: &JmapBackend, account: &AccountContext, method: &str, args: Value) -> Value {
    let responses = call(backend, account, json!([[method, args, "0"]])).await;
    assert_eq!(responses[0][0], method, "{responses:?}");
    responses[0][1].clone()
}

pub(super) fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("nebula-test-{prefix}-{nanos}")
}

pub(super) fn created(response: &Value, key: &str) -> String {
    response["created"][key]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{key} not created: {response}"))
        .to_owned()
}

/// Waits for the calendars capability (DAV discovery).
pub(super) async fn wait_for_calendars(backend: &JmapBackend, account: &AccountContext) {
    for _ in 0..200 {
        let capabilities = backend.account_capabilities(account).await.unwrap();
        if let Some(calendars) = &capabilities.calendars {
            assert!(calendars.may_create_calendar);
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("calendars never announced");
}

/// Where an event is on the DAV server, from the store.
pub(super) async fn event_href(store: &Store, account: &AccountContext, id: &str) -> Href {
    let base = id.split('_').next().unwrap();
    let row = ResourceId::from_raw(base.strip_prefix('v').unwrap().parse().unwrap()).unwrap();
    let event = store.dav_resource(account.id, row).await.unwrap().unwrap();
    Href::from_url(&url::Url::parse(event.href.as_str()).unwrap()).unwrap()
}

pub(super) async fn calendar_href(store: &Store, account: &AccountContext, id: &str) -> Href {
    let row: i64 = id.strip_prefix('c').unwrap().parse().unwrap();
    let calendar = store
        .dav_collections(account.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id.get() == row)
        .unwrap();
    Href::from_url(&url::Url::parse(calendar.props.href.as_str()).unwrap()).unwrap()
}

pub(super) async fn dav_text(client: &DavClient, href: &Href) -> Option<String> {
    match client.get(href, ResourceKind::Calendar).await {
        Ok(resource) => Some(resource.data),
        Err(DavError::NotFound) => None,
        Err(error) => panic!("GET failed: {error}"),
    }
}

/// iCalendar unfolded (long lines are folded at 75 octets).
pub(super) fn unfolded(text: &str) -> String {
    text.replace("\r\n ", "").replace("\r\n\t", "")
}

/// `eventProperties` + `withoutNulls` (calendar/editing.ts) of a new event.
fn new_event(calendar: &str, title: &str, start: &str, guests: &[(&str, &str)], me: &str) -> Value {
    let mut event = json!({
        "alerts": {"a1": {"@type": "Alert", "action": "display",
            "trigger": {"@type": "OffsetTrigger", "offset": "-PT15M", "relativeTo": "start"}}},
        "duration": "PT1H",
        "showWithoutTime": false,
        "start": start,
        "title": title,
        "calendarIds": {calendar: true},
        "timeZone": "Europe/Madrid",
        "description": "Orden del día",
        "locations": {"l1": {"@type": "Location", "name": "Sala Andrómeda"}}
    });
    if !guests.is_empty() {
        let mut participants = serde_json::Map::new();
        participants.insert(
            "oself".to_owned(),
            json!({"@type": "Participant", "calendarAddress": format!("mailto:{me}"),
                   "name": "Alice", "participationStatus": "accepted",
                   "roles": {"attendee": true, "owner": true}}),
        );
        for (key, address) in guests {
            participants.insert(
                (*key).to_owned(),
                json!({"@type": "Participant", "calendarAddress": format!("mailto:{address}"),
                       "expectReply": true, "kind": "individual",
                       "participationStatus": "needs-action", "roles": {"attendee": true}}),
            );
        }
        event["participants"] = Value::Object(participants);
        event["organizerCalendarAddress"] = Value::from(format!("mailto:{me}"));
    }
    event
}

/// `listOccurrences` (calendar/api.ts): the expanded occurrences of a
/// calendar between two dates, with what the web asks for.
async fn occurrences(
    backend: &JmapBackend,
    account: &AccountContext,
    calendar: &str,
    after: &str,
    before: &str,
) -> Vec<Value> {
    let a = account.jmap_id.as_str();
    let responses = call(
        backend,
        account,
        json!([
            ["CalendarEvent/query", {"accountId": a, "expandRecurrences": true,
                "filter": {"after": after, "before": before, "inCalendar": calendar},
                "sort": [{"property": "start"}], "timeZone": "Europe/Madrid"}, "0"],
            ["CalendarEvent/get", {"#ids": {"name": "CalendarEvent/query", "path": "/ids", "resultOf": "0"},
                "accountId": a, "timeZone": "Europe/Madrid",
                "properties": ["title", "start", "duration", "baseEventId", "recurrenceId",
                               "showWithoutTime", "participants", "utcStart"]}, "1"]
        ]),
    )
    .await;
    responses[1][1]["list"].as_array().unwrap().clone()
}

async fn set_event(backend: &JmapBackend, account: &AccountContext, args: Value) -> Value {
    let mut args = args;
    args["accountId"] = Value::from(account.jmap_id.as_str());
    one(backend, account, "CalendarEvent/set", args).await
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one session of the web, step by step"
)]
async fn calendars_like_the_web() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let alice = super::start(&backend, &store, mail_server(dav), User::Alice).await;
    let a = alice.jmap_id.as_str();
    let other = other_client(dav, User::Alice);
    wait_for_calendars(&backend, &alice).await;

    // createCalendar (two: the second to move events to).
    let name = unique("cal");
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "create": {
            "calendar": {"color": "#3366cc", "isSubscribed": true, "isVisible": true, "name": name},
            "second": {"name": format!("{name}-2"), "isSubscribed": true, "isVisible": true}
        }}),
    )
    .await;
    let calendar = created(&set, "calendar");
    let second = created(&set, "second");
    let home = &other.discover().await.unwrap().calendar_homes[0];
    let remote = other.list_collections(home).await.unwrap();
    let found = remote
        .iter()
        .find(|c| c.display_name.as_deref() == Some(name.as_str()))
        .expect("the new calendar on the DAV server");
    assert_eq!(
        found.color.map(|c| c.to_string()).as_deref(),
        Some("#3366CC")
    );

    // updateCalendar: rename and recolor (PROPPATCH), hide (store only).
    let renamed = format!("{name}-renamed");
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "update": {
            calendar.clone(): {"name": renamed, "color": "#cc3366"},
            second.clone(): {"isVisible": false}
        }}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let remote = other.list_collections(home).await.unwrap();
    let found = remote
        .iter()
        .find(|c| c.display_name.as_deref() == Some(renamed.as_str()))
        .expect("renamed on the DAV server");
    assert_eq!(
        found.color.map(|c| c.to_string()).as_deref(),
        Some("#CC3366")
    );
    let listed = one(
        &backend,
        &alice,
        "Calendar/get",
        json!({"accountId": a, "ids": [calendar.clone(), second.clone()]}),
    )
    .await;
    assert_eq!(listed["list"][0]["name"], renamed.as_str());
    assert_eq!(listed["list"][0]["color"], "#CC3366");
    assert_eq!(listed["list"][1]["isVisible"], false);

    // createEvent, then read it back as the web does.
    let set = set_event(
        &backend,
        &alice,
        json!({"create": {"event": new_event(&calendar, "Planificación", "2027-03-08T10:00:00", &[], &alice.email)},
               "sendSchedulingMessages": false}),
    )
    .await;
    let single = created(&set, "event");
    let uid = set["created"]["event"]["uid"].as_str().unwrap().to_owned();
    let single_href = event_href(&store, &alice, &single).await;
    let text = unfolded(&dav_text(&other, &single_href).await.unwrap());
    for wanted in [
        "SUMMARY:Planificación",
        "DTSTART;TZID=Europe/Madrid:20270308T100000",
        "LOCATION",
        "Sala Andrómeda",
        "BEGIN:VALARM",
        "BEGIN:VTIMEZONE",
        "DTSTAMP:",
        &format!("UID:{uid}"),
    ] {
        assert!(text.contains(wanted), "{wanted} missing:\n{text}");
    }
    let list = occurrences(
        &backend,
        &alice,
        &calendar,
        "2027-03-08T00:00:00",
        "2027-03-09T00:00:00",
    )
    .await;
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["title"], "Planificación");

    // A full update from the editor (null removes), then dismissAlert.
    let set = set_event(
        &backend,
        &alice,
        json!({"update": {single.clone(): {
            "alerts": {"a1": {"@type": "Alert", "action": "display",
                "trigger": {"@type": "OffsetTrigger", "offset": "-PT10M", "relativeTo": "start"}}},
            "duration": "PT90M", "locations": null, "participants": null,
            "showWithoutTime": false, "start": "2027-03-08T11:00:00", "title": "Planificación Q2",
            "calendarIds": {calendar.clone(): true}, "recurrenceRule": null,
            "timeZone": "Europe/Madrid", "organizerCalendarAddress": null,
            "description": null, "descriptionContentType": null
        }}, "sendSchedulingMessages": false}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let set = set_event(
        &backend,
        &alice,
        json!({"update": {single.clone(): {"alerts/a1/acknowledged": "2027-03-08T09:50:00Z"}},
               "sendSchedulingMessages": false}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let text = unfolded(&dav_text(&other, &single_href).await.unwrap());
    assert!(text.contains("SUMMARY:Planificación Q2"), "{text}");
    assert!(
        text.contains("20270308T110000") && (text.contains("PT1H30M") || text.contains("PT90M")),
        "{text}"
    );
    assert!(
        !text.contains("Sala Andrómeda") && !text.contains("Orden del día"),
        "{text}"
    );
    assert!(text.contains("ACKNOWLEDGED:20270308T095000Z"), "{text}");

    // Two writers at once: another client changes the event while the
    // bridge's copy is old; the bridge gets a 412, reads it again and
    // applies its change on top. Then two of the bridge's own at once.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let current = other
        .get(&single_href, ResourceKind::Calendar)
        .await
        .unwrap();
    let theirs = current
        .data
        .replace("END:VEVENT", "CATEGORIES:theirs\r\nEND:VEVENT");
    other
        .put(
            &single_href,
            ResourceKind::Calendar,
            &theirs,
            &PutCondition::Update(current.etag.unwrap()),
        )
        .await
        .unwrap();
    let set = set_event(
        &backend,
        &alice,
        json!({"update": {single.clone(): {"title": "Ours"}}, "sendSchedulingMessages": false}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let text = unfolded(&dav_text(&other, &single_href).await.unwrap());
    assert!(
        text.contains("SUMMARY:Ours") && text.contains("theirs"),
        "{text}"
    );
    let (first, second_write) = tokio::join!(
        set_event(
            &backend,
            &alice,
            json!({"update": {single.clone(): {"description": "uno"}}, "sendSchedulingMessages": false}),
        ),
        set_event(
            &backend,
            &alice,
            json!({"update": {single.clone(): {"color": "red"}}, "sendSchedulingMessages": false}),
        ),
    );
    assert!(first["notUpdated"].is_null(), "{first}");
    assert!(second_write["notUpdated"].is_null(), "{second_write}");
    let text = unfolded(&dav_text(&other, &single_href).await.unwrap());
    assert!(
        text.contains("DESCRIPTION:uno") && text.contains("COLOR:red"),
        "{text}"
    );

    // A weekly series; "this event" changes one occurrence (an override),
    // deleting one excludes it (EXDATE).
    let mut weekly = new_event(
        &calendar,
        "Semanal",
        "2027-03-01T09:00:00",
        &[],
        &alice.email,
    );
    weekly["recurrenceRule"] =
        json!({"@type": "RecurrenceRule", "frequency": "weekly", "count": 4});
    let set = set_event(
        &backend,
        &alice,
        json!({"create": {"w": weekly}, "sendSchedulingMessages": false}),
    )
    .await;
    let series = created(&set, "w");
    let list = occurrences(
        &backend,
        &alice,
        &calendar,
        "2027-03-01T00:00:00",
        "2027-03-29T00:00:00",
    )
    .await;
    let weekly_ids: Vec<String> = list
        .iter()
        .filter(|o| o["baseEventId"] == series.as_str())
        .map(|o| o["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(weekly_ids.len(), 4, "{list:?}");
    let set = set_event(
        &backend,
        &alice,
        json!({"update": {weekly_ids[1].clone(): {
            "alerts": null, "duration": "PT1H", "locations": null, "participants": null,
            "showWithoutTime": false, "start": "2027-03-08T12:00:00", "title": "Semanal (movida)",
            "organizerCalendarAddress": null, "description": null, "descriptionContentType": null
        }}, "destroy": [weekly_ids[2].clone()], "sendSchedulingMessages": false}),
    )
    .await;
    assert!(
        set["notUpdated"].is_null() && set["notDestroyed"].is_null(),
        "{set}"
    );
    let series_href = event_href(&store, &alice, &series).await;
    let text = unfolded(&dav_text(&other, &series_href).await.unwrap());
    assert!(
        text.contains("RECURRENCE-ID;TZID=Europe/Madrid:20270308T090000"),
        "{text}"
    );
    assert!(
        text.contains("EXDATE;TZID=Europe/Madrid:20270315T090000"),
        "{text}"
    );
    assert!(text.contains("RRULE:FREQ=WEEKLY;COUNT=4"), "{text}");
    let list = occurrences(
        &backend,
        &alice,
        &calendar,
        "2027-03-01T00:00:00",
        "2027-03-29T00:00:00",
    )
    .await;
    let titles: Vec<(&str, &str)> = list
        .iter()
        .filter(|o| o["baseEventId"] == series.as_str())
        .map(|o| (o["start"].as_str().unwrap(), o["title"].as_str().unwrap()))
        .collect();
    assert_eq!(
        titles,
        [
            ("2027-03-01T09:00:00", "Semanal"),
            ("2027-03-08T12:00:00", "Semanal (movida)"),
            ("2027-03-22T09:00:00", "Semanal")
        ]
    );

    // One day of an all-day series (Stalwart 0.16.27 turns the series into
    // one event; through the bridge the series stays).
    let set = set_event(
        &backend,
        &alice,
        json!({"create": {"d": {"calendarIds": {calendar.clone(): true}, "title": "Congreso",
            "start": "2027-03-10T00:00:00", "duration": "P1D", "showWithoutTime": true,
            "recurrenceRule": {"@type": "RecurrenceRule", "frequency": "daily", "count": 3}}},
            "sendSchedulingMessages": false}),
    )
    .await;
    let days = created(&set, "d");
    let list = occurrences(
        &backend,
        &alice,
        &calendar,
        "2027-03-10T00:00:00",
        "2027-03-13T00:00:00",
    )
    .await;
    let day_ids: Vec<String> = list
        .iter()
        .filter(|o| o["baseEventId"] == days.as_str())
        .map(|o| o["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(day_ids.len(), 3, "{list:?}");
    let set = set_event(
        &backend,
        &alice,
        json!({"update": {day_ids[1].clone(): {"title": "Congreso (día 2)"}}, "sendSchedulingMessages": false}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    let days_href = event_href(&store, &alice, &days).await;
    let text = unfolded(&dav_text(&other, &days_href).await.unwrap());
    assert!(text.contains("RECURRENCE-ID;VALUE=DATE:20270311"), "{text}");
    assert!(text.contains("RRULE:FREQ=DAILY;COUNT=3"), "{text}");
    let list = occurrences(
        &backend,
        &alice,
        &calendar,
        "2027-03-10T00:00:00",
        "2027-03-13T00:00:00",
    )
    .await;
    let titles: Vec<&str> = list
        .iter()
        .filter(|o| o["baseEventId"] == days.as_str())
        .map(|o| o["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Congreso", "Congreso (día 2)", "Congreso"]);

    // Moving the single event to the second calendar (MOVE, same id).
    let set = set_event(
        &backend,
        &alice,
        json!({"update": {single.clone(): {"calendarIds": {second.clone(): true}}}, "sendSchedulingMessages": false}),
    )
    .await;
    assert!(set["notUpdated"].is_null(), "{set}");
    assert!(dav_text(&other, &single_href).await.is_none());
    let moved_href = event_href(&store, &alice, &single).await;
    assert!(moved_href.is_member_of(&calendar_href(&store, &alice, &second).await));
    assert!(
        dav_text(&other, &moved_href)
            .await
            .unwrap()
            .contains("SUMMARY:Ours")
    );

    // deleteEvent, and a calendar with events.
    let set = set_event(
        &backend,
        &alice,
        json!({"destroy": [single.clone()], "sendSchedulingMessages": false}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([single]), "{set}");
    assert!(dav_text(&other, &moved_href).await.is_none());
    let refused = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [calendar.clone()]}),
    )
    .await;
    assert_eq!(
        refused["notDestroyed"][&calendar]["type"], "calendarHasEvent",
        "{refused}"
    );
    let gone = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [calendar.clone(), second.clone()], "onDestroyRemoveEvents": true}),
    )
    .await;
    assert_eq!(gone["destroyed"].as_array().unwrap().len(), 2, "{gone}");
    let remote = other.list_collections(home).await.unwrap();
    assert!(!remote.iter().any(|c| {
        c.display_name
            .as_deref()
            .is_some_and(|n| n.starts_with(&name))
    }));
}

/// The iMIP emails bob got for this title: (`method` from the
/// `text/calendar` part, its event as `CalendarEvent/parse` reads it).
/// They are deleted after.
async fn invitations(
    backend: &JmapBackend,
    bob: &AccountContext,
    title: &str,
    count: usize,
    wait: Duration,
) -> Vec<(String, Value)> {
    itip_parts(backend, bob, title, count, wait, true)
        .await
        .into_iter()
        .map(|(method, event, _)| (method, event))
        .collect()
}

/// The same with each part's blob id; `destroy`: delete the emails after.
async fn itip_parts(
    backend: &JmapBackend,
    bob: &AccountContext,
    title: &str,
    count: usize,
    wait: Duration,
    destroy: bool,
) -> Vec<(String, Value, String)> {
    let b = bob.jmap_id.as_str();
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        // Only what arrived: Sent holds the bridge's own copies.
        let ids = in_mailbox(backend, bob, "inbox", title).await;
        if ids.len() >= count || tokio::time::Instant::now() > deadline {
            let mut out = Vec::new();
            if ids.is_empty() {
                return out;
            }
            let got = one(
                backend,
                bob,
                "Email/get",
                json!({"accountId": b, "ids": ids, "properties": ["subject", "attachments", "bodyStructure"],
                       "bodyProperties": ["type", "blobId", "partId", "subParts"]}),
            )
            .await;
            for email in got["list"].as_array().unwrap() {
                let mut stack = vec![email["bodyStructure"].clone()];
                while let Some(part) = stack.pop() {
                    if let Some(children) = part["subParts"].as_array() {
                        stack.extend(children.iter().cloned());
                    }
                    if part["type"] == "text/calendar" {
                        let part_blob = part["blobId"].as_str().unwrap();
                        let parsed = one(
                            backend,
                            bob,
                            "CalendarEvent/parse",
                            json!({"accountId": b, "blobIds": [part_blob]}),
                        )
                        .await;
                        let event = parsed["parsed"][part_blob][0].clone();
                        let method = event["method"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_default();
                        out.push((method, event, part_blob.to_owned()));
                    }
                }
            }
            if destroy {
                one(
                    backend,
                    bob,
                    "Email/set",
                    json!({"accountId": b, "destroy": ids}),
                )
                .await;
            }
            return out;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// One of bob's events: where, its etag and its unfolded text.
type BobsEvent = (Href, Option<ETag>, String);

/// Bob's events (his own calendars, as another client of his) whose text
/// contains `needle`.
async fn bobs_events(dav: DavTestServer, needle: &str) -> Vec<BobsEvent> {
    let bobs = other_client(dav, User::Bob);
    let found = bobs.discover().await.unwrap();
    let mut out = Vec::new();
    for home in &found.calendar_homes {
        for calendar in bobs.list_collections(home).await.unwrap() {
            for member in bobs.list_members(&calendar.href).await.unwrap_or_default() {
                if let Ok(resource) = bobs.get(&member.href, ResourceKind::Calendar).await
                    && resource.data.contains(needle)
                {
                    out.push((member.href, resource.etag, unfolded(&resource.data)));
                }
            }
        }
    }
    out
}

/// Nextcloud: the server puts the REQUEST in bob's calendar (no mail),
/// and a CANCEL when alice deletes it with `sendSchedulingMessages`;
/// nothing for the quiet one. Bob's copies are deleted at the end.
async fn nextcloud_invitation(
    backend: &JmapBackend,
    alice: &AccountContext,
    dav: DavTestServer,
    invite: &str,
    uid: &str,
    quiet_title: &str,
) {
    let wait = async |done: &dyn Fn(&[BobsEvent]) -> bool| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let found = bobs_events(dav, uid).await;
            if done(&found) || tokio::time::Instant::now() > deadline {
                return found;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };
    let delivered = wait(&|found| !found.is_empty()).await;
    assert_eq!(delivered.len(), 1, "{delivered:?}");
    assert!(
        delivered[0].2.contains("PARTSTAT=NEEDS-ACTION"),
        "{}",
        delivered[0].2
    );
    let set = set_event(
        backend,
        alice,
        json!({"destroy": [invite], "sendSchedulingMessages": true}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([invite]), "{set}");
    let cancelled = |found: &[BobsEvent]| {
        found
            .iter()
            .all(|(_, _, text)| text.contains("STATUS:CANCELLED"))
    };
    let after = wait(&cancelled).await;
    assert!(cancelled(&after), "{after:?}");
    assert!(bobs_events(dav, quiet_title).await.is_empty());
    let bobs = other_client(dav, User::Bob);
    for (href, etag, _) in after {
        bobs.delete(&href, etag.as_ref()).await.unwrap();
    }
}

/// alice invites bob (`sendSchedulingMessages: true`, as the editor's
/// "send invitations"), then deletes the event: bob gets a REQUEST and a
/// CANCEL. With Radicale the bridge sends them (through Postfix, to
/// Dovecot); with Stalwart, Stalwart does.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one invitation, step by step")]
async fn invitation_round_trip() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let mail = mail_server(dav);
    let alice = super::start(&backend, &store, mail, User::Alice).await;
    let bob = super::start(&backend, &store, mail, User::Bob).await;
    let a = alice.jmap_id.as_str();
    let other = other_client(dav, User::Alice);
    wait_for_calendars(&backend, &alice).await;
    wait_for_calendars(&backend, &bob).await;
    super::mailbox(&backend, &bob, "inbox").await;

    let name = unique("invites");
    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "create": {"c": {"name": name, "isSubscribed": true, "isVisible": true}}}),
    )
    .await;
    let calendar = created(&set, "c");
    let bob_address = bob.email.clone();

    // Without sendSchedulingMessages: nothing is sent, and the attendees
    // say so (SCHEDULE-AGENT=CLIENT) to a scheduling server.
    let quiet_title = unique("quiet");
    let set = set_event(
        &backend,
        &alice,
        json!({"create": {"e": new_event(&calendar, &quiet_title, "2027-04-05T10:00:00",
                &[("pbob", &bob_address)], &alice.email)},
               "sendSchedulingMessages": false}),
    )
    .await;
    let quiet = created(&set, "e");
    let quiet_href = event_href(&store, &alice, &quiet).await;
    let text = unfolded(&dav_text(&other, &quiet_href).await.unwrap());
    let bob_line = text
        .split("\r\n")
        .find(|l| l.starts_with("ATTENDEE") && l.to_lowercase().contains(&bob_address))
        .unwrap_or_else(|| panic!("no bob in\n{text}"))
        .to_owned();
    assert!(bob_line.contains("SCHEDULE-AGENT=CLIENT"), "{bob_line}");

    // With it: a REQUEST to bob.
    let title = unique("invite");
    let set = set_event(
        &backend,
        &alice,
        json!({"create": {"e": new_event(&calendar, &title, "2027-04-06T10:00:00",
                &[("pbob", &bob_address)], &alice.email)},
               "sendSchedulingMessages": true}),
    )
    .await;
    let invite = created(&set, "e");
    let uid = {
        let got = one(
            &backend,
            &alice,
            "CalendarEvent/get",
            json!({"accountId": a, "ids": [invite.clone()], "properties": ["uid"]}),
        )
        .await;
        got["list"][0]["uid"].as_str().unwrap().to_owned()
    };
    let invite_href = event_href(&store, &alice, &invite).await;
    let text = unfolded(&dav_text(&other, &invite_href).await.unwrap());
    let bob_line = text
        .split("\r\n")
        .find(|l| l.starts_with("ATTENDEE") && l.to_lowercase().contains(&bob_address))
        .unwrap()
        .to_owned();
    match dav.kind {
        // The bridge sent it: a scheduling server reached later must not.
        DavKind::Radicale => assert!(bob_line.contains("SCHEDULE-AGENT=CLIENT"), "{bob_line}"),
        // The server sends it.
        DavKind::Stalwart | DavKind::Nextcloud => {
            assert!(!bob_line.contains("SCHEDULE-AGENT"), "{bob_line}");
        }
    }
    if dav.kind == DavKind::Nextcloud {
        nextcloud_invitation(&backend, &alice, dav, &invite, &uid, &quiet_title).await;
        one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}),
        )
        .await;
        return;
    }
    let got = invitations(&backend, &bob, &title, 1, Duration::from_secs(90)).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (method, event) = &got[0];
    assert_eq!(event["uid"], uid.as_str(), "{event}");
    assert_eq!(method, "request", "{event}");
    if dav.kind == DavKind::Radicale {
        // The bridge leaves the organizer's alarms out (Stalwart sends
        // them in the first REQUEST).
        assert!(event.get("alerts").is_none(), "{event}");
    }

    // Deleting it with sendSchedulingMessages: a CANCEL.
    let set = set_event(
        &backend,
        &alice,
        json!({"destroy": [invite.clone()], "sendSchedulingMessages": true}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([invite]), "{set}");
    let got = invitations(&backend, &bob, &title, 1, Duration::from_secs(90)).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (method, event) = &got[0];
    assert_eq!(event["uid"], uid.as_str(), "{event}");
    assert_eq!(method, "cancel", "{event}");
    assert_eq!(event["status"], "cancelled", "{event}");
    // Nothing ever came for the quiet one.
    assert!(
        invitations(&backend, &bob, &quiet_title, 1, Duration::from_secs(5))
            .await
            .is_empty()
    );

    one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}),
    )
    .await;
    // Stalwart puts invitations from its users into bob's calendar on its
    // own (also from DAV writes): remove what is there with
    // this UID, as another client of bob's.
    if dav.kind == DavKind::Stalwart {
        let bobs = other_client(dav, User::Bob);
        let found = bobs.discover().await.unwrap();
        let mut removed = 0;
        for home in &found.calendar_homes {
            for calendar in bobs.list_collections(home).await.unwrap() {
                for member in bobs.list_members(&calendar.href).await.unwrap_or_default() {
                    let Ok(resource) = bobs.get(&member.href, ResourceKind::Calendar).await else {
                        continue;
                    };
                    if resource.data.contains(&uid) {
                        bobs.delete(&member.href, resource.etag.as_ref())
                            .await
                            .unwrap();
                        removed += 1;
                    }
                }
            }
        }
        // Stalwart put the invitation in bob's calendar by itself.
        assert!(removed >= 1, "the invitation was not in bob's calendar");
    }
}

/// Deletes the emails with `title` in the subject.
/// Ids of the emails with `subject` in the mailbox with `role`.
async fn in_mailbox(
    backend: &JmapBackend,
    account: &AccountContext,
    role: &str,
    subject: &str,
) -> Vec<String> {
    let mailbox = super::mailbox(backend, account, role).await;
    let found = one(
        backend,
        account,
        "Email/query",
        json!({"accountId": account.jmap_id.as_str(),
               "filter": {"inMailbox": mailbox, "subject": subject}}),
    )
    .await;
    found["ids"]
        .as_array()
        .map(|ids| ids.iter().map(|v| v.as_str().unwrap().to_owned()).collect())
        .unwrap_or_default()
}

/// The subjects and `To` addresses of the copies in Sent with `title`
/// (the bridge keeps one per iMIP message).
async fn sent_copies(
    backend: &JmapBackend,
    account: &AccountContext,
    title: &str,
) -> Vec<(String, Vec<String>, bool)> {
    let ids = in_mailbox(backend, account, "sent", title).await;
    if ids.is_empty() {
        return Vec::new();
    }
    let got = one(
        backend,
        account,
        "Email/get",
        json!({"accountId": account.jmap_id.as_str(), "ids": ids,
               "properties": ["subject", "to", "keywords", "receivedAt"]}),
    )
    .await;
    let mut list: Vec<_> = got["list"]
        .as_array()
        .unwrap()
        .iter()
        .map(|email| {
            let to = email["to"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["email"].as_str().unwrap().to_owned())
                .collect();
            (
                email["subject"].as_str().unwrap().to_owned(),
                to,
                email["keywords"]["$seen"] == true,
            )
        })
        .collect();
    list.sort();
    list
}

async fn delete_mail(backend: &JmapBackend, account: &AccountContext, title: &str) {
    let ids = super::find(backend, account, title, Duration::from_secs(1)).await;
    if !ids.is_empty() {
        one(
            backend,
            account,
            "Email/set",
            json!({"accountId": account.jmap_id.as_str(), "destroy": ids}),
        )
        .await;
    }
}

/// The bridge sends what the web used to:
/// alice changes one occurrence of a weekly series with bob → bob gets
/// the REQUEST of that occurrence; bob proposes another time from that
/// email (`CalendarEvent/counter` with its blob) → alice gets a COUNTER;
/// alice keeps the time (`CalendarEvent/declineCounter`) → bob gets a
/// DECLINECOUNTER. Radicale only (the bridge sends there, through
/// Postfix to Dovecot); Stalwart limits its users' mail per hour.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one exchange, step by step")]
async fn occurrence_request_and_proposals() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    if dav.kind != DavKind::Radicale {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let mail = mail_server(dav);
    let alice = super::start(&backend, &store, mail, User::Alice).await;
    let bob = super::start(&backend, &store, mail, User::Bob).await;
    let (a, b) = (alice.jmap_id.as_str(), bob.jmap_id.as_str());
    wait_for_calendars(&backend, &alice).await;
    wait_for_calendars(&backend, &bob).await;
    super::mailbox(&backend, &alice, "inbox").await;
    super::mailbox(&backend, &bob, "inbox").await;

    let set = one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "create": {"c": {"name": unique("proposals"), "isSubscribed": true, "isVisible": true}}}),
    )
    .await;
    let calendar = created(&set, "c");
    let title = unique("weekly");
    let mut series = new_event(
        &calendar,
        &title,
        "2027-05-03T10:00:00",
        &[("pbob", &bob.email)],
        &alice.email,
    );
    series["recurrenceRule"] =
        json!({"@type": "RecurrenceRule", "frequency": "weekly", "count": 4});
    let set = set_event(
        &backend,
        &alice,
        json!({"create": {"e": series}, "sendSchedulingMessages": true}),
    )
    .await;
    let event = created(&set, "e");
    let got = invitations(&backend, &bob, &title, 1, Duration::from_secs(90)).await;
    assert_eq!(got.len(), 1, "{got:?}");
    // A copy in alice's Sent (Dovecot: `sent_copy = "append"`), read.
    assert_eq!(
        sent_copies(&backend, &alice, &title).await,
        [(
            format!("Invitation: {title}"),
            vec![bob.email.clone()],
            true
        )]
    );

    // One occurrence moved, "send the changes": the bridge mails it.
    let list = occurrences(
        &backend,
        &alice,
        &calendar,
        "2027-05-09T00:00:00",
        "2027-05-12T00:00:00",
    )
    .await;
    let second = list
        .iter()
        .find(|o| o["baseEventId"] == event.as_str())
        .unwrap_or_else(|| panic!("{list:?}"));
    let occurrence = second["id"].as_str().unwrap().to_owned();
    let set = set_event(
        &backend,
        &alice,
        json!({"update": {occurrence.clone(): {"start": "2027-05-10T12:00:00", "duration": "PT1H"}},
               "sendSchedulingMessages": true}),
    )
    .await;
    assert!(set["updated"].get(&occurrence).is_some(), "{set}");
    let got = itip_parts(&backend, &bob, &title, 1, Duration::from_secs(90), false).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (method, request, request_blob) = &got[0];
    assert_eq!(method, "request", "{request}");
    assert_eq!(request["recurrenceId"], "2027-05-10T10:00:00", "{request}");
    assert!(request.get("recurrenceRule").is_none(), "{request}");

    // Bob proposes another time from that email.
    let responses = call_using(
        &backend,
        &bob,
        json!([["CalendarEvent/counter", {"accountId": b, "blobId": request_blob,
            "utcStart": "2027-05-10T14:00:00Z", "utcEnd": "2027-05-10T15:00:00Z",
            "comment": "After lunch?"}, "0"]]),
        true,
    )
    .await;
    assert_eq!(responses[0][0], "CalendarEvent/counter", "{responses:?}");
    // The proposal is in bob's Sent, as the web kept it.
    assert_eq!(
        sent_copies(&backend, &bob, &title).await,
        [(
            format!("New time proposed: {title}"),
            vec![alice.email.clone()],
            true
        )]
    );
    delete_mail(&backend, &bob, &title).await;
    let got = itip_parts(&backend, &alice, &title, 1, Duration::from_secs(90), false).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (method, proposal, proposal_blob) = &got[0];
    assert_eq!(method, "counter", "{proposal}");
    assert_eq!(
        proposal["recurrenceId"], "2027-05-10T10:00:00",
        "{proposal}"
    );
    assert!(
        proposal.to_string().contains("After lunch?"),
        "the note: {proposal}"
    );

    // Alice keeps the time.
    let responses = call_using(
        &backend,
        &alice,
        json!([["CalendarEvent/declineCounter", {"accountId": a, "eventId": event,
            "blobId": proposal_blob}, "0"]]),
        true,
    )
    .await;
    assert_eq!(
        responses[0][0], "CalendarEvent/declineCounter",
        "{responses:?}"
    );
    // alice's Sent: the invitation, the moved occurrence and the refusal,
    // one copy each.
    let copies = sent_copies(&backend, &alice, &title).await;
    let subjects: Vec<&str> = copies.iter().map(|(s, _, _)| s.as_str()).collect();
    assert_eq!(
        subjects,
        [
            format!("Invitation: {title}"),
            format!("Proposal declined: {title}"),
            format!("Updated invitation: {title}"),
        ],
        "{copies:?}"
    );
    assert!(
        copies
            .iter()
            .all(|(_, to, seen)| *seen && to == std::slice::from_ref(&bob.email)),
        "{copies:?}"
    );
    delete_mail(&backend, &alice, &title).await;
    let got = invitations(&backend, &bob, &title, 1, Duration::from_secs(90)).await;
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "declinecounter", "{:?}", got[0].1);

    // Without the bridge capability in `using`: unknownMethod.
    let responses = call(
        &backend,
        &alice,
        json!([["CalendarEvent/declineCounter", {"accountId": a, "eventId": event,
            "blobId": proposal_blob}, "0"]]),
    )
    .await;
    assert_eq!(responses[0][0], "error", "{responses:?}");
    one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}),
    )
    .await;
}

/// alice assigns a task to bob: a task
/// with bob as a participant and `sendSchedulingMessages: true`, as the
/// web's task panel does. bob gets a REQUEST with the VTODO; when alice
/// takes the task back (deletes it), a CANCEL. With Radicale the bridge
/// sends them; with Stalwart's DAV, Stalwart does (its iTIP code takes
/// VTODO as it takes VEVENT).
#[tokio::test(flavor = "multi_thread")]
async fn task_assignment_round_trip() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    if dav.kind == DavKind::Nextcloud {
        // Nextcloud keeps the invitation inside its own users (no mail).
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let mail = mail_server(dav);
    let alice = super::start(&backend, &store, mail, User::Alice).await;
    let bob = super::start(&backend, &store, mail, User::Bob).await;
    let a = alice.jmap_id.as_str();
    let other = other_client(dav, User::Alice);
    wait_for_calendars(&backend, &alice).await;
    wait_for_calendars(&backend, &bob).await;
    super::mailbox(&backend, &bob, "inbox").await;
    let calendar = {
        let set = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "create": {"c": {"name": unique("assign"), "isSubscribed": true, "isVisible": true}}}),
        )
        .await;
        created(&set, "c")
    };
    let bob_address = bob.email.clone();
    let title = unique("task");
    let set = set_event(
        &backend,
        &alice,
        json!({"create": {"t": {
            "@type": "Task", "calendarIds": {calendar.clone(): true}, "title": title,
            "due": "2027-04-06T00:00:00", "showWithoutTime": true, "progress": "needs-action",
            "organizerCalendarAddress": format!("mailto:{}", alice.email),
            "participants": {
                "o": {"@type": "Participant", "calendarAddress": format!("mailto:{}", alice.email),
                      "participationStatus": "accepted", "roles": {"attendee": true, "owner": true}},
                "pbob": {"@type": "Participant", "calendarAddress": format!("mailto:{bob_address}"),
                         "expectReply": true, "participationStatus": "needs-action",
                         "roles": {"attendee": true}}
            }
        }}, "sendSchedulingMessages": true}),
    )
    .await;
    let task = created(&set, "t");
    let href = event_href(&store, &alice, &task).await;
    let text = unfolded(&dav_text(&other, &href).await.unwrap());
    assert!(text.contains("BEGIN:VTODO"), "{text}");
    let bob_line = text
        .split("\r\n")
        .find(|l| l.starts_with("ATTENDEE") && l.to_lowercase().contains(&bob_address))
        .unwrap_or_else(|| panic!("no bob in\n{text}"))
        .to_owned();
    if dav.kind == DavKind::Radicale {
        assert!(bob_line.contains("SCHEDULE-AGENT=CLIENT"), "{bob_line}");
    }
    let got = invitations(&backend, &bob, &title, 1, Duration::from_secs(90)).await;
    assert_eq!(got.len(), 1, "{got:?}");
    let (method, parsed) = &got[0];
    assert_eq!(method, "request", "{parsed}");
    assert_eq!(parsed["@type"], "Task", "{parsed}");

    let set = set_event(
        &backend,
        &alice,
        json!({"destroy": [task.clone()], "sendSchedulingMessages": true}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([task]), "{set}");
    let got = invitations(&backend, &bob, &title, 1, Duration::from_secs(90)).await;
    let (method, parsed) = &got[0];
    assert_eq!(method, "cancel", "{parsed}");
    one(
        &backend,
        &alice,
        "Calendar/set",
        json!({"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}),
    )
    .await;
}
