// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sharing calendars and address books through the bridge
//!  against a real DAV server: Stalwart's DAV (WebDAV
//! ACL, with Stalwart's IMAP), Radicale (no sharing; Dovecot's IMAP),
//! Nextcloud (its own `O:share`, calendars and address books; Dovecot's
//! IMAP) or Baïkal (Calendar Server sharing of calendars; Dovecot's IMAP).
//! Skipped unless `NEBULA_TEST_DAV=1` (Stalwart, Radicale or Nextcloud,
//! `NEBULA_TEST_SERVER`) or `NEBULA_TEST_BAIKAL=1` (Baïkal, `docker
//! compose … --profile bridge up -d baikal`):
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=stalwart cargo test -p nebula-jmap --test server sharing
//! NEBULA_TEST_BAIKAL=1 cargo test -p nebula-jmap --test server sharing
//! ```
//!
//! Alice creates a calendar and an address book through the bridge and
//! shares them with bob as the web does (`shareWith/<id>`), read-only and
//! then read and write; bob sees them in alice's account in the session,
//! can or can't write accordingly; unsharing removes them. Everything is
//! deleted at the end.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_dav::proto::{CalendarAddress, Href, ShareAccess, Sharee};
use nebula_dav::{Credentials, DavClient, DavServer};
use nebula_http::{AccountContext, ApiCall, Backend};
use nebula_jmap::JmapBackend;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_store::Store;
use nebula_tests::{DavKind, DavTestServer, ServerKind, TestServer, User};
use serde_json::{Value, json};

/// The server under test.
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
            Self::Baikal => format!("http://{}:5233/dav.php/", nebula_tests::HOST),
            Self::Nextcloud => DavTestServer {
                kind: DavKind::Nextcloud,
            }
            .base_url(),
        }
    }

    fn mail(self) -> TestServer {
        TestServer::of(match self {
            Self::Stalwart => ServerKind::Stalwart,
            Self::Radicale | Self::Baikal | Self::Nextcloud => ServerKind::Dovecot,
        })
    }

    /// Calendars can be shared.
    const fn calendars(self) -> bool {
        matches!(self, Self::Stalwart | Self::Baikal | Self::Nextcloud)
    }

    /// Address books can be shared (sabre/dav has no sharing for them).
    const fn books(self) -> bool {
        matches!(self, Self::Stalwart | Self::Nextcloud)
    }
}

async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let body = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:principals",
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
            Capability::Principals,
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

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}-{nanos}")
}

/// Waits for calendars and contacts (DAV discovery).
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

/// The calendar or book called `name` in `account_id`, if it is there.
async fn named(
    backend: &JmapBackend,
    account: &AccountContext,
    method: &str,
    account_id: &str,
    name: &str,
) -> Option<Value> {
    // An account that is not (or no longer) there is an error: `None`.
    let got = call(
        backend,
        account,
        json!([[method, {"accountId": account_id}, "0"]]),
    )
    .await;
    got[0][1]["list"]
        .as_array()?
        .iter()
        .find(|c| {
            // Nextcloud names a sharee's copy "<name> (<owner>)".
            c["name"] == name || c["name"] == format!("{name} ({})", User::Alice.login()).as_str()
        })
        .cloned()
}

/// Waits until `check` holds for the shared account of alice in bob's
/// session (or for no shared account, with `None`).
async fn wait_shared(
    backend: &JmapBackend,
    bob: &AccountContext,
    what: &str,
    check: impl Fn(Option<&nebula_http::SharedAccount>) -> bool,
) -> Option<nebula_http::SharedAccount> {
    for _ in 0..90 {
        let accounts = backend.shared_accounts(bob).await.unwrap();
        // Named with her address, or (Baïkal, which hides her principal
        // from bob) the last part of her principal's location.
        let alices = accounts.iter().find(|a| {
            a.name.eq_ignore_ascii_case(&User::Alice.address()) || a.name == User::Alice.login()
        });
        if check(alices) {
            return alices.cloned();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("bob never saw {what}");
}

const CALENDAR_READ: &str = r#"{"mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": false, "mayWriteOwn": false, "mayUpdatePrivate": false, "mayRSVP": false, "mayShare": false, "mayDelete": false}"#;
const CALENDAR_WRITE: &str = r#"{"mayReadFreeBusy": true, "mayReadItems": true, "mayWriteAll": true, "mayWriteOwn": true, "mayUpdatePrivate": true, "mayRSVP": true, "mayShare": false, "mayDelete": false}"#;
/// The web's levels (contacts/sharing.ts `bookRightsFor`).
const BOOK_READ: &str =
    r#"{"mayRead": true, "mayWrite": false, "mayDelete": false, "mayShare": false}"#;
const BOOK_WRITE: &str =
    r#"{"mayRead": true, "mayWrite": true, "mayDelete": false, "mayShare": false}"#;

/// How alice shares with bob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Read,
    Write,
    Stop,
}

/// Another DAV client, as alice (for Baïkal, where the bridge can't name
/// bob).
fn other_client(under: Under) -> DavClient {
    let config = DavServer::new(&under.base_url())
        .unwrap()
        .with_private_network(true);
    let credentials =
        Credentials::new(User::Alice.login(), under.mail().password(User::Alice)).unwrap();
    DavClient::new(config, Some(credentials)).unwrap()
}

/// Where alice's collection `name` is on the server.
async fn stored_href(store: &Store, alice: &AccountContext, name: &str) -> Href {
    let collection = store
        .dav_collections(alice.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.props.name.as_deref() == Some(name))
        .unwrap();
    Href::from_url(&url::Url::parse(collection.props.href.as_str()).unwrap()).unwrap()
}

fn rights(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn event(calendar: &str, title: &str) -> Value {
    json!({
        "@type": "Event", "calendarIds": {calendar: true}, "title": title,
        "start": "2026-11-20T10:00:00", "timeZone": "Europe/Madrid", "duration": "PT1H"
    })
}

fn card(book: &str, name: &str) -> Value {
    json!({
        "@type": "Card", "version": "1.0", "addressBookIds": {book: true}, "kind": "individual",
        "name": {"@type": "Name", "full": name}
    })
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one sharing session, step by step")]
async fn alice_shares_with_bob() {
    let Some(under) = Under::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = super::contacts::backend_with(
        &store,
        &under.base_url(),
        under.mail(),
        Duration::from_secs(2),
    );
    let alice = super::start(&backend, &store, under.mail(), User::Alice).await;
    let bob = super::start(&backend, &store, under.mail(), User::Bob).await;
    wait_for_groupware(&backend, &alice).await;
    wait_for_groupware(&backend, &bob).await;
    let a = alice.jmap_id.as_str().to_owned();

    // Alice's new calendar and address book.
    let calendar_name = unique("Compartido");
    let book_name = unique("Libreta compartida");
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Calendar/set", {"accountId": a, "create": {"c": {"name": calendar_name, "color": "#3366cc"}}}, "0"],
            ["AddressBook/set", {"accountId": a, "create": {"b": {"name": book_name}}}, "1"]
        ]),
    )
    .await;
    let calendar = responses[0][1]["created"]["c"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{responses:?}"))
        .to_owned();
    let book = responses[1][1]["created"]["b"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{responses:?}"))
        .to_owned();
    let own_calendar = named(&backend, &alice, "Calendar/get", &a, &calendar_name)
        .await
        .unwrap();
    let own_book = named(&backend, &alice, "AddressBook/get", &a, &book_name)
        .await
        .unwrap();
    assert_eq!(
        own_calendar["myRights"]["mayShare"],
        under.calendars(),
        "{own_calendar}"
    );
    assert_eq!(
        own_book["myRights"]["mayShare"],
        under.books(),
        "{own_book}"
    );
    if under.calendars() {
        assert_eq!(own_calendar["shareWith"], json!({}), "{own_calendar}");
    } else {
        assert_eq!(own_calendar["shareWith"], Value::Null, "{own_calendar}");
    }

    // Bob's principal id, as the web finds people (Principal/query).
    let found = one(
        &backend,
        &alice,
        "Principal/query",
        json!({"accountId": a, "filter": {"email": User::Bob.address()}, "limit": 1}),
    )
    .await;
    let bob_id = found["ids"][0].as_str().map(str::to_owned);

    let cleanup = async || {
        let responses = call(
            &backend,
            &alice,
            json!([
                ["Calendar/set", {"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}, "0"],
                ["AddressBook/set", {"accountId": a, "destroy": [book], "onDestroyRemoveContents": true}, "1"]
            ]),
        )
        .await;
        assert_eq!(
            responses[0][1]["destroyed"],
            json!([calendar]),
            "{responses:?}"
        );
        assert_eq!(responses[1][1]["destroyed"], json!([book]), "{responses:?}");
    };

    if !under.calendars() {
        // Radicale: nobody else is shown, and nothing can be shared.
        assert_eq!(bob_id, None);
        let set = one(
            &backend,
            &alice,
            "Calendar/set",
            json!({"accountId": a, "update": {calendar.clone(): {"shareWith/r12345": rights(CALENDAR_READ)}}}),
        )
        .await;
        assert_eq!(
            set["notUpdated"][&calendar]["type"], "invalidProperties",
            "{set}"
        );
        assert!(backend.shared_accounts(&bob).await.unwrap().is_empty());
        cleanup().await;
        return;
    }
    // Baïkal does not let users read each other's principals (403): bob
    // is found by his typed address (an id of the bridge, shared with
    // `mailto:`).
    let bob_id = Some(bob_id.expect("the server shows bob, or his address"));
    let other = (under == Under::Baikal).then(|| other_client(under));
    let calendar_href = stored_href(&store, &alice, &calendar_name).await;
    let share = async |level: Level| {
        let Some(bob_id) = &bob_id else {
            let other = other.as_ref().unwrap();
            let bob = Sharee::Address(CalendarAddress::parse(&User::Bob.address()).unwrap());
            let (set, remove) = match level {
                Level::Read => (vec![(bob, ShareAccess::Read)], vec![]),
                Level::Write => (vec![(bob, ShareAccess::ReadWrite)], vec![]),
                Level::Stop => (vec![], vec![bob]),
            };
            other.share(&calendar_href, &set, &remove).await.unwrap();
            return;
        };
        let (calendar_rights, book_rights) = match level {
            Level::Read => (rights(CALENDAR_READ), rights(BOOK_READ)),
            Level::Write => (rights(CALENDAR_WRITE), rights(BOOK_WRITE)),
            Level::Stop => (Value::Null, Value::Null),
        };
        // Stopping: shareWith set whole (calendar) and per principal
        // (book), both as the web may do.
        let calendar_patch = if level == Level::Stop {
            json!({"shareWith": {}})
        } else {
            json!({format!("shareWith/{bob_id}"): calendar_rights})
        };
        let mut updates = vec![json!([
            "Calendar/set",
            {"accountId": a, "update": {calendar.clone(): calendar_patch}},
            "0"
        ])];
        if under.books() {
            updates.push(json!([
                "AddressBook/set",
                {"accountId": a, "update": {book.clone(): {format!("shareWith/{bob_id}"): book_rights}}},
                "1"
            ]));
        }
        let responses = call(&backend, &alice, Value::Array(updates)).await;
        for response in &responses {
            assert!(
                response[1]["updated"]
                    .as_object()
                    .is_some_and(|u| !u.is_empty()),
                "{responses:?}"
            );
        }
        let own_calendar = named(&backend, &alice, "Calendar/get", &a, &calendar_name)
            .await
            .unwrap();
        let shared_rights = &own_calendar["shareWith"][bob_id];
        if level == Level::Stop {
            assert_eq!(own_calendar["shareWith"], json!({}), "{own_calendar}");
        } else {
            assert_eq!(shared_rights["mayReadItems"], true, "{own_calendar}");
            assert_eq!(
                shared_rights["mayWriteAll"],
                level == Level::Write,
                "{own_calendar}"
            );
        }
        if under.books() {
            let own_book = named(&backend, &alice, "AddressBook/get", &a, &book_name)
                .await
                .unwrap();
            if level == Level::Stop {
                assert_eq!(own_book["shareWith"], json!({}), "{own_book}");
            } else if under == Under::Nextcloud && level == Level::Write {
                // Nextcloud has read and read-write only: "view and edit"
                // comes back as read-write, deleting included.
                let mut expected = book_rights.clone();
                expected["mayDelete"] = json!(true);
                assert_eq!(own_book["shareWith"][bob_id], expected, "{own_book}");
            } else {
                assert_eq!(own_book["shareWith"][bob_id], book_rights, "{own_book}");
            }
        }
    };

    // Read-only.
    share(Level::Read).await;

    // Bob sees alice's account with them.
    let shared = wait_shared(&backend, &bob, "alice's calendar", |account| {
        account.is_some_and(|a| a.capabilities.calendars.is_some())
    })
    .await
    .unwrap();
    let s = shared.id.as_str().to_owned();
    assert!(s.starts_with(&format!("{}-r", bob.jmap_id.as_str())), "{s}");
    assert!(
        !shared
            .capabilities
            .calendars
            .as_ref()
            .unwrap()
            .may_create_calendar
    );
    let mut theirs = None;
    for _ in 0..30 {
        theirs = named(&backend, &bob, "Calendar/get", &s, &calendar_name).await;
        if theirs.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let theirs = theirs.expect("bob sees the calendar");
    let shared_calendar = theirs["id"].as_str().unwrap().to_owned();
    assert_eq!(theirs["myRights"]["mayReadItems"], true, "{theirs}");
    assert_eq!(theirs["myRights"]["mayWriteAll"], false, "{theirs}");
    assert_eq!(theirs["myRights"]["mayShare"], false, "{theirs}");
    assert_eq!(theirs["isDefault"], false, "{theirs}");
    // Not in bob's own account.
    assert!(
        named(
            &backend,
            &bob,
            "Calendar/get",
            bob.jmap_id.as_str(),
            &calendar_name
        )
        .await
        .is_none()
    );
    // Read-only: bob can't add an event, nor create calendars there.
    let responses = call(
        &backend,
        &bob,
        json!([
            ["CalendarEvent/set", {"accountId": s, "create": {"e": event(&shared_calendar, "De Bob")}}, "0"],
            ["Calendar/set", {"accountId": s, "create": {"c": {"name": "No"}}}, "1"]
        ]),
    )
    .await;
    assert!(
        responses[0][1]["notCreated"]["e"].is_object(),
        "{responses:?}"
    );
    assert_eq!(
        responses[1][1]["notCreated"]["c"]["type"], "forbidden",
        "{responses:?}"
    );
    // A shared account is only for calendars and contacts: another
    // method there is `accountNotSupportedByMethod` (RFC 8620 §3.6.2: the
    // account exists), not `accountNotFound`.
    let responses = call(
        &backend,
        &bob,
        json!([
            ["Mailbox/get", {"accountId": s}, "0"],
            ["Principal/get", {"accountId": s, "ids": null}, "1"],
            ["Principal/get", {"accountId": format!("{s}9"), "ids": null}, "2"]
        ]),
    )
    .await;
    assert_eq!(responses[0][0], "error", "{responses:?}");
    assert_eq!(
        responses[1][1]["type"], "accountNotSupportedByMethod",
        "{responses:?}"
    );
    assert_eq!(responses[2][1]["type"], "accountNotFound", "{responses:?}");
    let shared_book = if under.books() {
        let mut theirs = None;
        for _ in 0..30 {
            theirs = named(&backend, &bob, "AddressBook/get", &s, &book_name).await;
            if theirs.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let theirs = theirs.expect("bob sees the address book");
        assert_eq!(theirs["myRights"]["mayWrite"], false, "{theirs}");
        let id = theirs["id"].as_str().unwrap().to_owned();
        let set = one(
            &backend,
            &bob,
            "ContactCard/set",
            json!({"accountId": s, "create": {"k": card(&id, "No")}}),
        )
        .await;
        assert!(set["notCreated"]["k"].is_object(), "{set}");
        Some(id)
    } else {
        None
    };

    // Read and write.
    share(Level::Write).await;
    let mut writable = false;
    for _ in 0..60 {
        if let Some(theirs) = named(&backend, &bob, "Calendar/get", &s, &calendar_name).await
            && theirs["myRights"]["mayWriteAll"] == true
        {
            writable = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(writable, "bob never got write access");
    let set = one(
        &backend,
        &bob,
        "CalendarEvent/set",
        json!({"accountId": s, "create": {"e": event(&shared_calendar, "De Bob")}}),
    )
    .await;
    assert!(set["created"]["e"]["id"].is_string(), "{set}");
    if let Some(shared_book) = &shared_book {
        let mut done = false;
        for _ in 0..60 {
            let set = one(
                &backend,
                &bob,
                "ContactCard/set",
                json!({"accountId": s, "create": {"k": card(shared_book, "Contacto de Bob")}}),
            )
            .await;
            if set["created"]["k"]["id"].is_string() {
                done = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(done, "bob never could add a contact");
    }
    // Alice sees bob's event in her calendar.
    let mut seen = false;
    for _ in 0..60 {
        let found = one(
            &backend,
            &alice,
            "CalendarEvent/query",
            json!({"accountId": a, "filter": {"inCalendar": calendar}}),
        )
        .await;
        if found["ids"].as_array().is_some_and(|ids| !ids.is_empty()) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(seen, "alice never saw bob's event");

    // Unshare.
    share(Level::Stop).await;
    // Bob no longer sees it (alice's account goes away with the last
    // thing she shares with him).
    let mut gone = false;
    for _ in 0..60 {
        if named(&backend, &bob, "Calendar/get", &s, &calendar_name)
            .await
            .is_none()
        {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(gone, "the calendar is still shared with bob");

    cleanup().await;
}

/// [`call`] for big requests (a `shareWith` of thousands is megabytes).
async fn call_big(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let body = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:principals",
            "urn:ietf:params:jmap:calendars",
            "urn:ietf:params:jmap:contacts"
        ],
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
            Capability::Principals,
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

/// The principal ids of these addresses: the whole listing
/// (`Principal/query` without a filter, by pages, and `Principal/get` of
/// their `email`), one listing for all; asking by `email` searches the
/// server once per address (some 0.4 s each on Nextcloud). On a server
/// that hides its principals (Baïkal), by `email` instead: those are
/// addresses only the bridge knows.
async fn principal_ids(
    backend: &JmapBackend,
    alice: &AccountContext,
    addresses: &[String],
) -> Vec<String> {
    let a = alice.jmap_id.as_str();
    let mut listed: Vec<String> = Vec::new();
    loop {
        let page = one(
            backend,
            alice,
            "Principal/query",
            json!({"accountId": a, "position": listed.len(), "limit": 500}),
        )
        .await;
        let ids = page["ids"].as_array().unwrap_or_else(|| panic!("{page}"));
        if ids.is_empty() {
            break;
        }
        listed.extend(ids.iter().map(|id| id.as_str().unwrap().to_owned()));
    }
    let mut by_address = std::collections::HashMap::new();
    for chunk in listed.chunks(500) {
        let got = one(
            backend,
            alice,
            "Principal/get",
            json!({"accountId": a, "ids": chunk, "properties": ["email"]}),
        )
        .await;
        for principal in got["list"].as_array().unwrap() {
            if let Some(email) = principal["email"].as_str() {
                by_address.insert(
                    email.to_ascii_lowercase(),
                    principal["id"].as_str().unwrap().to_owned(),
                );
            }
        }
    }
    let mut ids = Vec::with_capacity(addresses.len());
    let mut missing = Vec::new();
    for address in addresses {
        match by_address.get(&address.to_ascii_lowercase()) {
            Some(id) => ids.push(id.clone()),
            None => missing.push(address.clone()),
        }
    }
    for request in missing.chunks(16 * 32) {
        let calls: Vec<Value> = request
            .chunks(16)
            .enumerate()
            .map(|(index, group)| {
                let conditions: Vec<Value> = group.iter().map(|e| json!({"email": e})).collect();
                json!(["Principal/query", {"accountId": a, "limit": 16,
                    "filter": {"operator": "OR", "conditions": conditions}}, index.to_string()])
            })
            .collect();
        for response in call_big(backend, alice, Value::Array(calls)).await {
            assert_eq!(response[0], "Principal/query", "{response}");
            ids.extend(
                response[1]["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| id.as_str().unwrap().to_owned()),
            );
        }
    }
    ids
}

/// Sets `shareWith` of one calendar or book whole, again while the bridge
/// says the change is unfinished (one sent in many `POST`s stops at
/// `share_deadline`); then checks what it reads back.
/// Returns the tries and the time.
#[allow(
    clippy::print_stderr,
    reason = "the times and counts of a run are the point"
)]
async fn set_share_with(
    backend: &JmapBackend,
    alice: &AccountContext,
    method: &str,
    id: &str,
    share_with: &Value,
) -> (usize, Duration) {
    let a = alice.jmap_id.as_str();
    let started = std::time::Instant::now();
    for tries in 1..=10 {
        let responses = call_big(
            backend,
            alice,
            json!([[method, {"accountId": a, "update": {id: {"shareWith": share_with}}}, "0"]]),
        )
        .await;
        let set = &responses[0][1];
        if set["updated"].get(id).is_some() {
            return (tries, started.elapsed());
        }
        eprintln!(
            "{method} try {tries} after {:?}: {}",
            started.elapsed(),
            set["notUpdated"][id]
        );
        // Only an unfinished change is tried again; anything else fails.
        let unfinished = set["notUpdated"][id]["description"]
            .as_str()
            .is_some_and(|d| d.contains("partly changed"));
        assert!(unfinished, "{responses:?}");
    }
    panic!("{method}: the sharing change never finished");
}

/// The `shareWith` of `id` as the bridge reads it back from the server.
async fn read_share_with(
    backend: &JmapBackend,
    alice: &AccountContext,
    method: &str,
    id: &str,
) -> serde_json::Map<String, Value> {
    let a = alice.jmap_id.as_str();
    let got = call_big(
        backend,
        alice,
        json!([[method, {"accountId": a, "ids": [id], "properties": ["shareWith"]}, "0"]]),
    )
    .await;
    got[0][1]["list"][0]["shareWith"]
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("{got:?}"))
}

/// Sharing a calendar (and an address book, where the server shares
/// them) with many accounts at once, then with nobody
/// (sharing limits): more than one `POST` of [`MAX_SHARE_REQUEST`] on
/// Calendar Server sharing,
/// one `ACL` of all of them on Stalwart (whose own limit, `maxShares` in
/// `x:Sharing`, is 10 by default: raise it for the test and put it back,
/// or the bridge rightly answers `forbidden`), and at most
/// [`MAX_OC_SHARE_REQUEST`] (10) a `POST` on Nextcloud, whose listing shows
/// only `sharing.maxAutocompleteResults` users (25 by default): raise it
/// too (`occ config:system:set sharing.maxAutocompleteResults --value=5000
/// --type=integer`, then `occ config:system:delete …`), or each address is
/// searched one by one (about 0.6 s each) and the bridge keeps only the
/// last 1000 found. The bridge reads back exactly the
/// sharees it sent (bob among them, who then sees the calendar), and none
/// once it is unshared.
///
/// Only with `NEBULA_TEST_MANY_SHAREES=<n>` and the accounts `share-1` …
/// `share-<n>` made by `bun docker/many-sharees.ts <server> <n>` (remove
/// them afterwards with `--remove`):
///
/// ```sh
/// bun docker/many-sharees.ts baikal 300
/// NEBULA_TEST_BAIKAL=1 NEBULA_TEST_MANY_SHAREES=300 \
///   cargo test -p nebula-jmap --test server share_with_many_accounts
/// ```
///
/// [`MAX_SHARE_REQUEST`]: nebula_dav::proto::MAX_SHARE_REQUEST
/// [`MAX_OC_SHARE_REQUEST`]: nebula_dav::proto::MAX_OC_SHARE_REQUEST
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario, step by step")]
#[allow(
    clippy::print_stderr,
    reason = "the times and counts of a run are the point"
)]
async fn share_with_many_accounts() {
    let Some(under) = Under::selected() else {
        return;
    };
    let Some(count) = std::env::var("NEBULA_TEST_MANY_SHAREES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return;
    };
    if !under.calendars() {
        eprintln!("{under:?} shares nothing: skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = super::contacts::backend_with(
        &store,
        &under.base_url(),
        under.mail(),
        Duration::from_secs(2),
    );
    let alice = super::start(&backend, &store, under.mail(), User::Alice).await;
    let bob = super::start(&backend, &store, under.mail(), User::Bob).await;
    wait_for_groupware(&backend, &alice).await;
    wait_for_groupware(&backend, &bob).await;
    let a = alice.jmap_id.as_str().to_owned();

    let calendar_name = unique("Muchos");
    let book_name = unique("Libreta de muchos");
    let mut creates = vec![
        json!(["Calendar/set", {"accountId": a, "create": {"c": {"name": calendar_name}}}, "0"]),
    ];
    if under.books() {
        creates.push(
            json!(["AddressBook/set", {"accountId": a, "create": {"b": {"name": book_name}}}, "1"]),
        );
    }
    let responses = call(&backend, &alice, Value::Array(creates)).await;
    let calendar = responses[0][1]["created"]["c"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{responses:?}"))
        .to_owned();
    let book = under.books().then(|| {
        responses[1][1]["created"]["b"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{responses:?}"))
            .to_owned()
    });

    let mut addresses: Vec<String> = (1..=count)
        .map(|n| format!("share-{n}@{}", nebula_tests::DOMAIN))
        .collect();
    addresses.push(User::Bob.address());
    let started = std::time::Instant::now();
    let ids = principal_ids(&backend, &alice, &addresses).await;
    eprintln!(
        "{under:?}: {} principal ids in {:?}",
        ids.len(),
        started.elapsed()
    );
    assert_eq!(ids.len(), addresses.len(), "every account is found");

    let mut collections = vec![(
        "Calendar/get",
        "Calendar/set",
        calendar.clone(),
        CALENDAR_READ,
    )];
    if let Some(book) = &book {
        collections.push((
            "AddressBook/get",
            "AddressBook/set",
            book.clone(),
            BOOK_READ,
        ));
    }
    for (get, set, id, level) in &collections {
        let share_with: serde_json::Map<String, Value> =
            ids.iter().map(|id| (id.clone(), rights(level))).collect();
        let (tries, took) =
            set_share_with(&backend, &alice, set, id, &Value::Object(share_with)).await;
        let read = read_share_with(&backend, &alice, get, id).await;
        eprintln!(
            "{under:?} {set}: shared with {} in {took:?} ({tries} JMAP calls), {} read back",
            ids.len(),
            read.len()
        );
        assert_eq!(read.len(), ids.len(), "{get}: every sharee is there");
        for id in &ids {
            assert_eq!(
                read[id]["mayReadItems"]
                    .as_bool()
                    .or(read[id]["mayRead"].as_bool()),
                Some(true),
                "{id}"
            );
        }
    }
    // Bob, one of them, sees it.
    let shared = wait_shared(&backend, &bob, "alice's calendar", |account| {
        account.is_some_and(|a| a.capabilities.calendars.is_some())
    })
    .await
    .unwrap();
    let mut seen = false;
    for _ in 0..30 {
        if named(
            &backend,
            &bob,
            "Calendar/get",
            shared.id.as_str(),
            &calendar_name,
        )
        .await
        .is_some()
        {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(seen, "bob sees the calendar shared with everyone");

    // With nobody: every one of them removed.
    for (get, set, id, _) in &collections {
        let (tries, took) = set_share_with(&backend, &alice, set, id, &json!({})).await;
        let read = read_share_with(&backend, &alice, get, id).await;
        eprintln!(
            "{under:?} {set}: unshared in {took:?} ({tries} JMAP calls), {} left",
            read.len()
        );
        assert!(read.is_empty(), "{get}: {} sharees left", read.len());
    }
    let mut gone = false;
    for _ in 0..60 {
        if named(
            &backend,
            &bob,
            "Calendar/get",
            shared.id.as_str(),
            &calendar_name,
        )
        .await
        .is_none()
        {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(gone, "the calendar is still shared with bob");

    let mut destroys = vec![
        json!(["Calendar/set", {"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}, "0"]),
    ];
    if let Some(book) = &book {
        destroys.push(json!(["AddressBook/set", {"accountId": a, "destroy": [book], "onDestroyRemoveContents": true}, "1"]));
    }
    let responses = call(&backend, &alice, Value::Array(destroys)).await;
    for response in &responses {
        assert!(
            response[1]["destroyed"]
                .as_array()
                .is_some_and(|d| d.len() == 1),
            "{responses:?}"
        );
    }
}

/// The name of the group the group test shares with: bob is in it. On
/// Nextcloud, docker/nextcloud/post-install.sh (on an older install, `occ
/// group:add --display-name=Ventas ventas && occ group:adduser ventas
/// bob`); on Stalwart, docker/setup.ts (`bun run dev:setup`), whose
/// group's description is its `displayname`.
const GROUP: &str = "Ventas";
/// The Stalwart group's login (its principal is `/dav/pal/ventas@…/`).
const STALWART_GROUP: &str = "ventas";

/// Sharing with a group: one sharee instead of every
/// member. On Nextcloud, `principal:principals/groups/<id>` with
/// `O:share`: the group has no address and is not listed, so it is found
/// as the web finds it, `Principal/query` with `text` (the bridge searches
/// `displayname` on the server). On Stalwart, an ACE for the group's
/// principal (WebDAV ACL), which Stalwart applies to its members; the
/// group is listed with the others, with `calendar-user-type` `GROUP` and
/// an address. Either way it comes back as a group, with its rights, in
/// `shareWith`; bob, a member, sees the calendar and the address book and
/// can write when the group can; unsharing takes them from him. With
/// `NEBULA_TEST_DAV=1` and `NEBULA_TEST_SERVER` `nextcloud` or
/// `stalwart`.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one sharing session, step by step")]
async fn alice_shares_with_a_group() {
    let Some(under @ (Under::Nextcloud | Under::Stalwart)) = Under::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = super::contacts::backend_with(
        &store,
        &under.base_url(),
        under.mail(),
        Duration::from_secs(2),
    );
    let alice = super::start(&backend, &store, under.mail(), User::Alice).await;
    let bob = super::start(&backend, &store, under.mail(), User::Bob).await;
    wait_for_groupware(&backend, &alice).await;
    wait_for_groupware(&backend, &bob).await;
    let a = alice.jmap_id.as_str().to_owned();

    // Found by name only, and a group.
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Principal/query", {"accountId": a, "filter": {"text": "venta"}, "limit": 20}, "0"],
            ["Principal/get", {"accountId": a, "#ids": {"resultOf": "0", "name": "Principal/query", "path": "/ids"},
                "properties": ["name", "email", "type"]}, "1"]
        ]),
    )
    .await;
    let group = responses[1][1]["list"]
        .as_array()
        .unwrap_or_else(|| panic!("{responses:?}"))
        .iter()
        .find(|p| p["name"] == GROUP)
        .unwrap_or_else(|| {
            panic!(
                "{under:?} has no group {GROUP} with bob in it (Nextcloud: docker/nextcloud/post-install.sh; Stalwart: bun run dev:setup): {responses:?}"
            )
        })
        .clone();
    assert_eq!(group["type"], "group", "{group}");
    let group_id = group["id"].as_str().unwrap().to_owned();
    let listed = one(
        &backend,
        &alice,
        "Principal/query",
        json!({"accountId": a, "limit": 500}),
    )
    .await;
    let in_listing = listed["ids"].as_array().unwrap().contains(&json!(group_id));
    if under == Under::Nextcloud {
        assert_eq!(group["email"], Value::Null, "{group}");
        // Not in the listing (a search for `@`): only the search finds it.
        assert!(!in_listing, "{listed}");
    } else {
        // Stalwart lists its groups with the others (`/dav/pal/`, Depth
        // 1), with their address.
        assert_eq!(
            group["email"],
            format!("{STALWART_GROUP}@{}", nebula_tests::DOMAIN),
            "{group}"
        );
        assert!(in_listing, "{listed}");
    }

    let calendar_name = unique("Del grupo");
    let book_name = unique("Libreta del grupo");
    let responses = call(
        &backend,
        &alice,
        json!([
            ["Calendar/set", {"accountId": a, "create": {"c": {"name": calendar_name}}}, "0"],
            ["AddressBook/set", {"accountId": a, "create": {"b": {"name": book_name}}}, "1"]
        ]),
    )
    .await;
    let calendar = responses[0][1]["created"]["c"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{responses:?}"))
        .to_owned();
    let book = responses[1][1]["created"]["b"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{responses:?}"))
        .to_owned();

    let share = async |level: Level| {
        let (calendar_rights, book_rights) = match level {
            Level::Read => (rights(CALENDAR_READ), rights(BOOK_READ)),
            Level::Write => (rights(CALENDAR_WRITE), rights(BOOK_WRITE)),
            Level::Stop => (Value::Null, Value::Null),
        };
        let key = format!("shareWith/{group_id}");
        let responses = call(
            &backend,
            &alice,
            json!([
                ["Calendar/set", {"accountId": a, "update": {calendar.clone(): {key.clone(): calendar_rights}}}, "0"],
                ["AddressBook/set", {"accountId": a, "update": {book.clone(): {key: book_rights}}}, "1"]
            ]),
        )
        .await;
        for response in &responses {
            assert!(
                response[1]["updated"]
                    .as_object()
                    .is_some_and(|u| !u.is_empty()),
                "{responses:?}"
            );
        }
        // Read back from the server: the group, with the rights given.
        let calendar_shares = read_share_with(&backend, &alice, "Calendar/get", &calendar).await;
        let book_shares = read_share_with(&backend, &alice, "AddressBook/get", &book).await;
        if level == Level::Stop {
            assert!(calendar_shares.is_empty(), "{calendar_shares:?}");
            assert!(book_shares.is_empty(), "{book_shares:?}");
            return;
        }
        assert_eq!(calendar_shares.len(), 1, "{calendar_shares:?}");
        let got = &calendar_shares[&group_id];
        assert_eq!(got["mayReadItems"], true, "{got}");
        assert_eq!(got["mayWriteAll"], level == Level::Write, "{got}");
        assert_eq!(book_shares.len(), 1, "{book_shares:?}");
        let got = &book_shares[&group_id];
        assert_eq!(got["mayRead"], true, "{got}");
        assert_eq!(got["mayWrite"], level == Level::Write, "{got}");
    };

    share(Level::Read).await;
    // A fresh look at the sharee (what the web does for one not in the
    // directory): still the group.
    let described = one(
        &backend,
        &alice,
        "Principal/get",
        json!({"accountId": a, "ids": [group_id], "properties": ["name", "type"]}),
    )
    .await;
    assert_eq!(described["list"][0]["name"], GROUP, "{described}");
    assert_eq!(described["list"][0]["type"], "group", "{described}");

    // Bob, a member, sees both, read-only.
    let shared = wait_shared(&backend, &bob, "the group's calendar", |account| {
        account.is_some_and(|a| {
            a.capabilities.calendars.is_some() && a.capabilities.contacts.is_some()
        })
    })
    .await
    .unwrap();
    let s = shared.id.as_str().to_owned();
    let mut theirs = None;
    for _ in 0..30 {
        theirs = named(&backend, &bob, "Calendar/get", &s, &calendar_name).await;
        if theirs.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let theirs = theirs.expect("bob sees the calendar shared with his group");
    let shared_calendar = theirs["id"].as_str().unwrap().to_owned();
    assert_eq!(theirs["myRights"]["mayReadItems"], true, "{theirs}");
    assert_eq!(theirs["myRights"]["mayWriteAll"], false, "{theirs}");
    let mut book_seen = None;
    for _ in 0..30 {
        book_seen = named(&backend, &bob, "AddressBook/get", &s, &book_name).await;
        if book_seen.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let book_seen = book_seen.expect("bob sees the address book shared with his group");
    assert_eq!(book_seen["myRights"]["mayWrite"], false, "{book_seen}");
    let shared_book = book_seen["id"].as_str().unwrap().to_owned();

    // The group can write: so can bob.
    share(Level::Write).await;
    let mut wrote = false;
    for _ in 0..60 {
        let set = one(
            &backend,
            &bob,
            "CalendarEvent/set",
            json!({"accountId": s, "create": {"e": event(&shared_calendar, "Del grupo")}}),
        )
        .await;
        if set["created"]["e"]["id"].is_string() {
            wrote = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(wrote, "bob never could write in the group's calendar");
    let mut added = false;
    for _ in 0..60 {
        let set = one(
            &backend,
            &bob,
            "ContactCard/set",
            json!({"accountId": s, "create": {"k": card(&shared_book, "Cliente del grupo")}}),
        )
        .await;
        if set["created"]["k"]["id"].is_string() {
            added = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(added, "bob never could write in the group's address book");

    // Unshared: gone for bob.
    share(Level::Stop).await;
    let mut gone = false;
    for _ in 0..60 {
        if named(&backend, &bob, "Calendar/get", &s, &calendar_name)
            .await
            .is_none()
            && named(&backend, &bob, "AddressBook/get", &s, &book_name)
                .await
                .is_none()
        {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(gone, "still shared with bob after unsharing with his group");

    let responses = call(
        &backend,
        &alice,
        json!([
            ["Calendar/set", {"accountId": a, "destroy": [calendar], "onDestroyRemoveEvents": true}, "0"],
            ["AddressBook/set", {"accountId": a, "destroy": [book], "onDestroyRemoveContents": true}, "1"]
        ]),
    )
    .await;
    assert_eq!(
        responses[0][1]["destroyed"],
        json!([calendar]),
        "{responses:?}"
    );
    assert_eq!(responses[1][1]["destroyed"], json!([book]), "{responses:?}");
}
