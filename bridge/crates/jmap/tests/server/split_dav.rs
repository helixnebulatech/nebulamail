// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendars and contacts on two DAV servers (`[upstream.dav]` `url` and
//! `contacts_url`), the way Fastmail
//! has them (`caldav.fastmail.com` and `carddav.fastmail.com`), with two
//! real servers of `docker/compose.yaml`: Radicale for calendars and
//! Baïkal for contacts, both with Dovecot's users and password. Skipped
//! unless `NEBULA_TEST_SPLIT_DAV=1`:
//!
//! ```sh
//! NEBULA_TEST_SPLIT_DAV=1 cargo test -p nebula-jmap --test server split_dav -- --test-threads=1
//! ```
//!
//! An event and a contact are created through JMAP; each must be on its
//! own server (and not on the other one). Both are deleted at the end.

use std::time::Duration;

use nebula_dav::proto::Href;
use nebula_dav::{Credentials, DavClient, DavError, DavServer, ResourceKind};
use nebula_groupware::GroupwareConfig;
use nebula_http::{AccountContext, ApiCall, Backend};
use nebula_jmap::JmapBackend;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_store::{ResourceId, Store};
use nebula_tests::{HOST, ServerKind, TestServer, User};
use serde_json::{Value, json};

fn calendars_url() -> String {
    format!("http://{HOST}:5232/")
}

fn contacts_url() -> String {
    format!("http://{HOST}:5233/dav.php/")
}

fn server(url: &str) -> DavServer {
    DavServer::new(url).unwrap().with_private_network(true)
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

fn created(response: &Value, key: &str) -> String {
    response["created"][key]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{key} not created: {response}"))
        .to_owned()
}

/// Where a resource (`k…` card, or event id) is on its DAV server.
async fn resource_href(store: &Store, account: &AccountContext, id: &str) -> Href {
    let base = id.split('_').next().unwrap();
    let number: i64 = base
        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .parse()
        .unwrap();
    let row = ResourceId::from_raw(number).unwrap();
    let found = store.dav_resource(account.id, row).await.unwrap().unwrap();
    Href::from_url(&url::Url::parse(found.href.as_str()).unwrap()).unwrap()
}

/// The text at `href` on a server (`None`: not there; Radicale answers
/// 403 for a path outside the user's own).
async fn text_at(client: &DavClient, href: &Href, kind: ResourceKind) -> Option<String> {
    match client.get(href, kind).await {
        Ok(found) => Some(found.data),
        Err(DavError::NotFound | DavError::Forbidden { .. }) => None,
        Err(error) => panic!("GET failed: {error}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one session, step by step")]
async fn calendars_and_contacts_on_two_servers() {
    if !std::env::var("NEBULA_TEST_SPLIT_DAV").is_ok_and(|v| v == "1") {
        return;
    }
    let mail = TestServer::of(ServerKind::Dovecot);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let mut groupware = GroupwareConfig::new(server(&calendars_url()))
        .with_contacts_server(server(&contacts_url()));
    groupware.poll_with_push = Duration::from_hours(1);
    groupware.poll_without_push = Duration::from_hours(1);
    let backend = super::contacts::backend_with_groupware(&store, mail, groupware);
    let alice = super::start(&backend, &store, mail, User::Alice).await;
    let a = alice.jmap_id.as_str().to_owned();
    // Announced after discovery; the collections come with the first
    // round (and the defaults a new user gets after it).
    let mut ready = false;
    for _ in 0..300 {
        let capabilities = backend.account_capabilities(&alice).await.unwrap();
        let kinds: Vec<_> = store
            .dav_collections(alice.id)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.props.kind)
            .collect();
        if capabilities.calendars.is_some()
            && capabilities.contacts.is_some()
            && kinds.contains(&nebula_store::CollectionKind::Calendar)
            && kinds.contains(&nebula_store::CollectionKind::AddressBook)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ready, "calendars and contacts never synced");

    // Each kind of collection only from its own server.
    let radicale = url::Url::parse(&calendars_url()).unwrap();
    let baikal = url::Url::parse(&contacts_url()).unwrap();
    let collections = store.dav_collections(alice.id).await.unwrap();
    for collection in &collections {
        let at = url::Url::parse(collection.props.href.as_str()).unwrap();
        let expected = match collection.props.kind {
            nebula_store::CollectionKind::Calendar => &radicale,
            nebula_store::CollectionKind::AddressBook => &baikal,
        };
        assert_eq!(
            at.origin(),
            expected.origin(),
            "{:?}",
            collection.props.kind
        );
    }

    let calendars = one(
        &backend,
        &alice,
        "Calendar/get",
        json!({"accountId": a, "properties": ["name", "isDefault"]}),
    )
    .await;
    let calendar = calendars["list"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|c| c["id"].as_str())
        .unwrap_or_else(|| panic!("no calendar: {calendars}"))
        .to_owned();
    let books = one(&backend, &alice, "AddressBook/get", json!({"accountId": a})).await;
    let book = books["list"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(|b| b["id"].as_str())
        .unwrap_or_else(|| panic!("no address book: {books}"))
        .to_owned();

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let event_uid = format!("nebula-test-split-event-{nanos}");
    let card_uid = format!("nebula-test-split-card-{nanos}");
    let set = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "create": {"e": {
            "calendarIds": {calendar.clone(): true}, "uid": event_uid,
            "title": "Split DAV test", "start": "2026-11-05T10:00:00",
            "timeZone": "Europe/Madrid", "duration": "PT30M"
        }}}),
    )
    .await;
    let event = created(&set, "e");
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "create": {"k": {
            "@type": "Card", "addressBookIds": {book.clone(): true}, "kind": "individual",
            "uid": card_uid, "version": "1.0",
            "name": {"@type": "Name", "full": "Split Test"},
            "emails": {"e1": {"@type": "EmailAddress", "address": "split@example.com"}}
        }}}),
    )
    .await;
    let card = created(&set, "k");

    // Each one on its server, read there by another client (each with the
    // login of its own server only)…
    let login = || Some(Credentials::new(User::Alice.login(), mail.password(User::Alice)).unwrap());
    let on_radicale = DavClient::new(server(&calendars_url()), login()).unwrap();
    let on_baikal = DavClient::new(server(&contacts_url()), login()).unwrap();
    let event_href = resource_href(&store, &alice, &event).await;
    let card_href = resource_href(&store, &alice, &card).await;
    assert_eq!(event_href.url().origin(), radicale.origin());
    assert_eq!(card_href.url().origin(), baikal.origin());
    let ics = text_at(&on_radicale, &event_href, ResourceKind::Calendar)
        .await
        .expect("the event on Radicale");
    assert!(ics.contains(&event_uid), "{ics}");
    let vcf = text_at(&on_baikal, &card_href, ResourceKind::Contact)
        .await
        .expect("the card on Baïkal");
    assert!(vcf.contains(&card_uid), "{vcf}");

    // …and not on the other one: the same path there has nothing, and no
    // collection of the other server holds it.
    let same_path = |href: &Href, base: &str| {
        Href::from_url(&url::Url::parse(base).unwrap().join(href.path()).unwrap()).unwrap()
    };
    assert_eq!(
        text_at(
            &on_baikal,
            &same_path(&event_href, &contacts_url()),
            ResourceKind::Calendar
        )
        .await,
        None
    );
    assert_eq!(
        text_at(
            &on_radicale,
            &same_path(&card_href, &calendars_url()),
            ResourceKind::Contact
        )
        .await,
        None
    );
    let radicale_found = on_radicale.discover().await.unwrap();
    for home in &radicale_found.addressbook_homes {
        for collection in on_radicale.list_collections(home).await.unwrap() {
            let members = on_radicale.list_members(&collection.href).await.unwrap();
            assert!(
                members
                    .iter()
                    .all(|m| m.href.last_segment() != card_href.last_segment()),
                "the card is not on Radicale"
            );
        }
    }
    let baikal_found = on_baikal.discover().await.unwrap();
    for home in &baikal_found.calendar_homes {
        for collection in on_baikal.list_collections(home).await.unwrap() {
            let members = on_baikal.list_members(&collection.href).await.unwrap();
            assert!(
                members
                    .iter()
                    .all(|m| m.href.last_segment() != event_href.last_segment()),
                "the event is not on Baïkal"
            );
        }
    }

    // Cleanup through JMAP: gone from both servers.
    let gone = one(
        &backend,
        &alice,
        "CalendarEvent/set",
        json!({"accountId": a, "destroy": [event]}),
    )
    .await;
    assert_eq!(gone["destroyed"], json!([event]), "{gone}");
    let gone = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "destroy": [card]}),
    )
    .await;
    assert_eq!(gone["destroyed"], json!([card]), "{gone}");
    assert_eq!(
        text_at(&on_radicale, &event_href, ResourceKind::Calendar).await,
        None
    );
    assert_eq!(
        text_at(&on_baikal, &card_href, ResourceKind::Contact).await,
        None
    );
}
