// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP for Contacts through the bridge against a real CardDAV server
//!Stalwart's DAV (with Stalwart's IMAP) or
//! Radicale (with Dovecot's), see `nebula-tests`. Skipped unless
//! `NEBULA_TEST_DAV=1`:
//!
//! ```sh
//! NEBULA_TEST_DAV=1 NEBULA_TEST_SERVER=radicale cargo test -p nebula-jmap --test server contacts
//! ```
//!
//! The requests are the web's (apps/web/src/contacts/api.ts, card.ts and
//! mail/contacts.ts); what they did is checked on the DAV server with the
//! DAV client as another client. Everything is in two new address books
//! with unique names, deleted at the end.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_dav::proto::{Href, ResourceName};
use nebula_dav::{
    CollectionKind, Credentials, DavClient, DavError, DavServer, PutCondition, ResourceKind,
};
use nebula_groupware::{Groupware, GroupwareConfig};
use nebula_http::{AccountContext, ApiCall, Backend, UploadBody};
use nebula_jmap::{JmapBackend, JmapConfig, SmtpSettings};
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_mail::{CertificateCheck, ImapServer, Mail, MailConfig, TlsMode};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::{ResourceId, Store};
use nebula_tests::{DavTestServer, User};
use net_guard::{NetworkAccess, Policy, Port, PortPolicy, SystemResolver, Target};
use serde_json::{Value, json};

/// The backend with the mail server of `dav` and the DAV server itself.
/// Polling is slow on purpose: the bridge sees other clients' changes only
/// when its own writes make it look (so the 412 path is certain).
pub(super) fn backend(store: &Store, dav: DavTestServer) -> JmapBackend {
    backend_with(store, &dav.base_url(), dav.mail(), Duration::from_hours(1))
}

/// [`backend`] for a DAV server at `base_url` with the mail server
/// `server`, polling every `poll`.
pub(super) fn backend_with(
    store: &Store,
    base_url: &str,
    server: nebula_tests::TestServer,
    poll: Duration,
) -> JmapBackend {
    let mut groupware =
        GroupwareConfig::new(DavServer::new(base_url).unwrap().with_private_network(true));
    groupware.poll_with_push = poll;
    groupware.poll_without_push = poll;
    backend_with_groupware(store, server, groupware)
}

/// The backend with the mail server `server` and this groupware layer.
pub(super) fn backend_with_groupware(
    store: &Store,
    server: nebula_tests::TestServer,
    groupware: GroupwareConfig,
) -> JmapBackend {
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
    let groupware = Groupware::new(store.clone(), groupware, Arc::clone(&resolver));
    JmapBackend::with_groupware(store.clone(), mail, Some(groupware), resolver, config)
}

/// The "other client": the DAV client on its own, as alice.
pub(super) fn other_client(dav: DavTestServer) -> DavClient {
    let config = DavServer::new(&dav.base_url())
        .unwrap()
        .with_private_network(true);
    let credentials = Credentials::new(dav.login(User::Alice), dav.password(User::Alice)).unwrap();
    DavClient::new(config, Some(credentials)).unwrap()
}

/// One request with the capabilities the web puts in `using` for
/// contacts; the method responses as JSON.
async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let body = json!({
        "using": [
            "urn:ietf:params:jmap:core",
            "urn:ietf:params:jmap:mail",
            "urn:ietf:params:jmap:contacts",
            "urn:ietf:params:jmap:contacts:parse"
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
            Capability::Mail,
            Capability::Contacts,
            Capability::ContactsParse,
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

/// The arguments of the only response, checking it is not an error.
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
    format!("nebula-test-{prefix}-{nanos}")
}

/// Where a card is on the DAV server, from the store.
async fn card_href(store: &Store, account: &AccountContext, id: &str) -> Href {
    let row = ResourceId::from_raw(id.strip_prefix('k').unwrap().parse().unwrap()).unwrap();
    let card = store.dav_resource(account.id, row).await.unwrap().unwrap();
    Href::from_url(&url::Url::parse(card.href.as_str()).unwrap()).unwrap()
}

/// Where a book is on the DAV server, from the store.
async fn book_href(store: &Store, account: &AccountContext, id: &str) -> Href {
    let row: i64 = id.strip_prefix('a').unwrap().parse().unwrap();
    let book = store
        .dav_collections(account.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id.get() == row)
        .unwrap();
    Href::from_url(&url::Url::parse(book.props.href.as_str()).unwrap()).unwrap()
}

/// The text of a card on the DAV server (`None`: not there).
async fn dav_text(client: &DavClient, href: &Href) -> Option<String> {
    match client.get(href, ResourceKind::Contact).await {
        Ok(resource) => Some(resource.data),
        Err(DavError::NotFound) => None,
        Err(error) => panic!("GET failed: {error}"),
    }
}

/// `newCard` (contacts/card.ts) for a person.
fn new_person(book: &str, uid: &str) -> Value {
    json!({
        "@type": "Card", "addressBookIds": {book: true}, "kind": "individual",
        "uid": uid, "version": "1.0",
        "name": {"@type": "Name", "full": "Carla Gómez",
                 "components": [{"kind": "given", "value": "Carla"}, {"kind": "surname", "value": "Gómez"}]},
        "emails": {"e1": {"@type": "EmailAddress", "address": "carla@example.com", "contexts": {"work": true}}},
        "phones": {"p1": {"number": "+34 600 000 000", "features": {"mobile": true}}},
        "notes": {"n1": {"note": "Línea 1\nLínea 2"}},
        "media": {"m1": {"kind": "photo", "uri": "data:image/png;base64,iVBORw0KGgo="}}
    })
}

/// `formProperties` (contacts/card.ts) after editing a person.
fn person_form(full: &str, email: &str) -> Value {
    json!({
        "addresses": null, "anniversaries": null,
        "emails": {"e1": {"@type": "EmailAddress", "address": email}},
        "kind": "individual", "media": null, "members": null,
        "name": {"@type": "Name", "full": full},
        "notes": null, "organizations": {"o1": {"name": "ACME"}},
        "phones": null, "titles": {"t1": {"kind": "title", "name": "CTO"}}
    })
}

/// A card as Apple's Contacts writes it (vCard 3.0).
fn apple_card(uid: &str) -> String {
    format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nPRODID:-//Apple Inc.//iPhone OS 17.0//EN\r\n\
         N:Pérez;Ana;;;\r\nFN:Ana Pérez\r\nORG:ACME;\r\n\
         item1.EMAIL;type=INTERNET;type=pref:ana@example.com\r\nitem1.X-ABLabel:_$!<Other>!$_\r\n\
         TEL;type=CELL;type=VOICE;type=pref:+34 600 000 001\r\nX-ABUID:{uid}:ABPerson\r\n\
         X-CUSTOM-THING:keep me\r\nUID:{uid}\r\nEND:VCARD\r\n"
    )
}

/// `ContactCard/get` says `id` is a group with exactly `members`.
async fn assert_group(backend: &JmapBackend, account: &AccountContext, id: &str, members: &[&str]) {
    let got = one(
        backend,
        account,
        "ContactCard/get",
        json!({"accountId": account.jmap_id.as_str(), "ids": [id], "properties": ["kind", "members"]}),
    )
    .await;
    let card = &got["list"][0];
    assert_eq!(card["kind"], "group", "{got}");
    let mut found: Vec<&str> = card["members"]
        .as_object()
        .unwrap_or_else(|| panic!("no members: {got}"))
        .keys()
        .map(String::as_str)
        .collect();
    found.sort_unstable();
    let mut wanted = members.to_vec();
    wanted.sort_unstable();
    assert_eq!(found, wanted, "{got}");
}

/// The created id of `key` in a `/set` response.
fn created(response: &Value, key: &str) -> String {
    response["created"][key]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{key} not created: {response}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one session of the web, step by step"
)]
async fn contacts_like_the_web() {
    let Some(dav) = DavTestServer::selected() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = backend(&store, dav);
    let alice = super::start(&backend, &store, dav.mail(), User::Alice).await;
    let a = alice.jmap_id.as_str();
    let other = other_client(dav);

    // The capability comes once DAV discovery found an address book home.
    let mut announced = false;
    for _ in 0..200 {
        let capabilities = backend.account_capabilities(&alice).await.unwrap();
        if capabilities.contacts.is_some() {
            assert!(capabilities.contacts_parse);
            announced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(announced, "contacts never announced");
    let first_state = one(
        &backend,
        &alice,
        "ContactCard/get",
        json!({"accountId": a, "ids": []}),
    )
    .await["state"]
        .clone();

    // A book of our own (the web lists them with these properties).
    let name = unique("book");
    let set = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "create": {"b": {"name": name, "description": "Pruebas"}}}),
    )
    .await;
    let book = created(&set, "b");
    let books = one(
        &backend,
        &alice,
        "AddressBook/get",
        json!({"accountId": a, "properties": ["name", "isDefault", "myRights", "sortOrder"]}),
    )
    .await;
    let listed = books["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == book.as_str())
        .unwrap()
        .clone();
    assert_eq!(listed["name"], name.as_str());
    assert_eq!(listed["myRights"]["mayWrite"], true);
    let home = &other.discover().await.unwrap().addressbook_homes[0];
    let on_server = other.list_collections(home).await.unwrap();
    let remote = on_server
        .iter()
        .find(|c| c.display_name.as_deref() == Some(name.as_str()))
        .expect("the new book on the DAV server");
    assert_eq!(remote.kind, CollectionKind::Addressbook);
    assert_eq!(remote.description.as_deref(), Some("Pruebas"));

    // createContact.
    let carla_uid = format!("urn:uuid:{}", unique("carla"));
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "create": {"card": new_person(&book, &carla_uid)}}),
    )
    .await;
    let carla = created(&set, "card");
    let carla_href = card_href(&store, &alice, &carla).await;
    let text = dav_text(&other, &carla_href).await.unwrap();
    for wanted in [
        "carla@example.com",
        "Carla Gómez",
        carla_uid.as_str(),
        "+34 600 000 000",
        "PHOTO",
    ] {
        assert!(text.contains(wanted), "{wanted} missing:\n{text}");
    }
    assert!(!text.contains("JSPROP"), "{text}");

    // listContacts (contacts/api.ts) and the recipient suggestions
    // (mail/contacts.ts).
    let responses = call(
        &backend,
        &alice,
        json!([
            ["ContactCard/query", {"accountId": a, "calculateTotal": true, "limit": 500, "position": 0}, "0"],
            ["ContactCard/get", {"#ids": {"name": "ContactCard/query", "path": "/ids", "resultOf": "0"},
                "accountId": a,
                "properties": ["uid", "kind", "name", "emails", "phones", "organizations", "addressBookIds", "members"]}, "1"],
            ["ContactCard/query", {"accountId": a, "limit": 500}, "2"],
            ["ContactCard/get", {"#ids": {"name": "ContactCard/query", "path": "/ids", "resultOf": "2"},
                "accountId": a, "properties": ["name", "emails"]}, "3"]
        ]),
    )
    .await;
    assert!(responses[0][1]["total"].as_u64().unwrap() >= 1);
    let card = responses[1][1]["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == carla.as_str())
        .unwrap()
        .clone();
    assert_eq!(card["uid"], carla_uid.as_str());
    // A 3.0 card has no KIND (RFC 2426; Radicale only takes 3.0), and an
    // absent `kind` is "individual" (RFC 9553 §2.1.4).
    assert!(
        card["kind"] == "individual" || card["kind"].is_null(),
        "{card}"
    );
    assert_eq!(card["addressBookIds"], json!({book.clone(): true}));
    assert!(card.get("media").is_none(), "not asked for");
    let emails: Vec<&Value> = card["emails"].as_object().unwrap().values().collect();
    assert_eq!(emails[0]["address"], "carla@example.com");
    let suggestion = responses[3][1]["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == carla.as_str())
        .unwrap()
        .clone();
    assert_eq!(suggestion["name"]["full"], "Carla Gómez");
    assert!(suggestion.get("uid").is_none());

    // getContact: everything, the photo too.
    let full = one(
        &backend,
        &alice,
        "ContactCard/get",
        json!({"accountId": a, "ids": [carla]}),
    )
    .await;
    let full = &full["list"][0];
    assert_eq!(full["@type"], "Card");
    let photo = full["media"].as_object().unwrap().values().next().unwrap();
    assert_eq!(photo["uri"], "data:image/png;base64,iVBORw0KGgo=");

    // Another client changes the card meanwhile: the web's edit gets a
    // 412, reads it again and applies its change on top.
    let current = other.get(&carla_href, ResourceKind::Contact).await.unwrap();
    let theirs = current
        .data
        .replace("END:VCARD", "NICKNAME:Carlita\r\nEND:VCARD");
    other
        .put(
            &carla_href,
            ResourceKind::Contact,
            &theirs,
            &PutCondition::Update(current.etag.unwrap()),
        )
        .await
        .unwrap();
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "update": {carla.clone(): person_form("Carla G. Gómez", "carla@new.example")}}),
    )
    .await;
    assert_eq!(set["updated"], json!({carla.clone(): null}), "{set}");
    let text = dav_text(&other, &carla_href).await.unwrap();
    for wanted in [
        "Carlita",
        "Carla G. Gómez",
        "carla@new.example",
        "ACME",
        "CTO",
    ] {
        assert!(text.contains(wanted), "{wanted} missing:\n{text}");
    }
    assert!(!text.contains("carla@example.com"), "{text}");
    assert!(!text.contains("PHOTO"), "media: null removes it:\n{text}");

    // A vCard 3.0 that Apple wrote; the bridge sees it at its next look
    // (a second book makes it list again).
    let apple_uid = unique("apple");
    let apple_href = book_href(&store, &alice, &book)
        .await
        .member(
            &ResourceName::try_from(format!("{apple_uid}.vcf").as_str()).unwrap(),
            false,
        )
        .unwrap();
    other
        .put(
            &apple_href,
            ResourceKind::Contact,
            &apple_card(&apple_uid),
            &PutCondition::Create,
        )
        .await
        .unwrap();
    let second_name = unique("second");
    let set = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "create": {"b2": {"name": second_name}}}),
    )
    .await;
    let second = created(&set, "b2");
    let found = one(
        &backend,
        &alice,
        "ContactCard/query",
        json!({"accountId": a, "filter": {"uid": apple_uid}}),
    )
    .await;
    let apple = found["ids"][0]
        .as_str()
        .expect("the Apple card seen")
        .to_owned();
    // The web edits it: what it does not show stays, and so does the
    // version the server keeps (Radicale: Apple's 3.0; Stalwart stores
    // every card as 4.0).
    let before = dav_text(&other, &apple_href).await.unwrap();
    let version = before
        .lines()
        .find(|line| line.starts_with("VERSION:"))
        .unwrap()
        .to_owned();
    eprintln!("{}: Apple's card is kept as {version}", dav.kind);
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "update": {apple.clone(): {
            "name": {"@type": "Name", "full": "Ana P. Pérez",
                     "components": [{"kind": "given", "value": "Ana"}, {"kind": "surname", "value": "Pérez"}]},
            "titles": {"t1": {"kind": "title", "name": "Directora"}},
            "notes": null, "addresses": null, "anniversaries": null, "members": null
        }}}),
    )
    .await;
    assert_eq!(set["updated"], json!({apple.clone(): null}), "{set}");
    let text = dav_text(&other, &apple_href).await.unwrap();
    assert!(
        text.starts_with(&format!("BEGIN:VCARD\r\n{version}\r\n")),
        "{text}"
    );
    if dav.kind == nebula_tests::DavKind::Radicale {
        assert_eq!(version, "VERSION:3.0");
    }
    // Nextcloud keeps each card as it was written and converts it to the
    // version asked for; the bridge asks for 4.0 (else a 4.0 card from the
    // web reads back with `X-ANNIVERSARY`, a year-less birthday as 1604…),
    // and sabre's 3.0 → 4.0 conversion puts its own PRODID.
    let prodid = if dav.kind == nebula_tests::DavKind::Nextcloud {
        "PRODID:-//Sabre//Sabre VObject"
    } else {
        "PRODID:-//Apple Inc.//iPhone OS 17.0//EN"
    };
    for kept in [
        format!("X-ABUID:{apple_uid}:ABPerson"),
        "X-CUSTOM-THING:keep me".to_owned(),
        "X-ABLABEL:_$!<Other>!$_".to_owned(),
        "ana@example.com".to_owned(),
        "+34 600 000 001".to_owned(),
        prodid.to_owned(),
        format!("UID:{apple_uid}"),
        "Ana P. Pérez".to_owned(),
        "Directora".to_owned(),
    ] {
        assert!(
            text.to_lowercase().contains(&kept.to_lowercase()),
            "{kept} missing:\n{text}"
        );
    }

    // A group (newCard with kind group) and setGroupMembers.
    let group_uid = format!("urn:uuid:{}", unique("group"));
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "create": {"g": {
            "@type": "Card", "addressBookIds": {book.clone(): true}, "kind": "group",
            "uid": group_uid, "version": "1.0",
            "name": {"@type": "Name", "full": "Equipo"},
            "members": {carla_uid.clone(): true}
        }}}),
    )
    .await;
    let group = created(&set, "g");
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "update": {group.clone(): {
            "members": {carla_uid.clone(): true, apple_uid.clone(): true}
        }}}),
    )
    .await;
    assert_eq!(set["updated"], json!({group.clone(): null}), "{set}");
    let group_text = dav_text(&other, &card_href(&store, &alice, &group).await)
        .await
        .unwrap();
    assert!(
        group_text.to_uppercase().contains("KIND:GROUP"),
        "{group_text}"
    );
    assert!(
        group_text.contains(&format!("MEMBER:{carla_uid}")),
        "{group_text}"
    );
    assert!(
        group_text.contains(&format!("MEMBER:{apple_uid}")),
        "{group_text}"
    );
    let got = one(
        &backend,
        &alice,
        "ContactCard/get",
        json!({"accountId": a, "ids": [group], "properties": ["kind", "members"]}),
    )
    .await;
    assert_eq!(got["list"][0]["kind"], "group");
    assert_eq!(got["list"][0]["members"].as_object().unwrap().len(), 2);
    // Read again from the server, by GET and by a full sync round (the
    // stored etag made stale so the multiget fetches it): Nextcloud keeps
    // the group as 3.0 (`X-ADDRESSBOOKSERVER-*`) and gives 4.0 only when
    // asked; either way it stays a group with its members.
    let running = backend.groupware().unwrap().account(alice.id).unwrap();
    let group_row =
        ResourceId::from_raw(group.strip_prefix('k').unwrap().parse().unwrap()).unwrap();
    let stored = store
        .dav_resource(alice.id, group_row)
        .await
        .unwrap()
        .unwrap();
    running.refetch(&stored).await.unwrap().unwrap();
    assert_group(&backend, &alice, &group, &[&carla_uid, &apple_uid]).await;
    store
        .set_resource_etags(
            alice.id,
            stored.collection,
            vec![(stored.href.clone(), Some("\"stale\"".to_owned()))],
        )
        .await
        .unwrap();
    running.sync_round(true).await.unwrap();
    let stored = store
        .dav_resource(alice.id, group_row)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        stored.etag.as_deref(),
        Some("\"stale\""),
        "the round fetched it"
    );
    assert_group(&backend, &alice, &group, &[&carla_uid, &apple_uid]).await;
    if dav.kind == nebula_tests::DavKind::Nextcloud {
        let text = running
            .resource_text(group_row)
            .await
            .unwrap()
            .unwrap()
            .text;
        assert!(text.contains("VERSION:4.0"), "asked for 4.0:\n{text}");
    }

    // A group as Apple writes it (3.0, `X-ADDRESSBOOKSERVER-*`), from
    // another client: read as a group, and its members changed in the
    // same form (no `KIND`/`MEMBER` in a 3.0 card, nothing stale).
    let apple_group_uid = unique("apple-group");
    let (old_member, kept_member) = (
        format!("urn:uuid:{}", unique("old")),
        format!("urn:uuid:{}", unique("kept")),
    );
    let apple_group_href = book_href(&store, &alice, &book)
        .await
        .member(
            &ResourceName::try_from(format!("{apple_group_uid}.vcf").as_str()).unwrap(),
            false,
        )
        .unwrap();
    other
        .put(
            &apple_group_href,
            ResourceKind::Contact,
            &format!(
                "BEGIN:VCARD\r\nVERSION:3.0\r\nPRODID:-//Apple Inc.//iOS 17.0//EN\r\n\
                 N:Amigos\r\nFN:Amigos\r\nX-ADDRESSBOOKSERVER-KIND:group\r\n\
                 X-ADDRESSBOOKSERVER-MEMBER:{old_member}\r\n\
                 X-ADDRESSBOOKSERVER-MEMBER:{kept_member}\r\n\
                 UID:{apple_group_uid}\r\nEND:VCARD\r\n"
            ),
            &PutCondition::Create,
        )
        .await
        .unwrap();
    running.sync_round(true).await.unwrap();
    let found = one(
        &backend,
        &alice,
        "ContactCard/query",
        json!({"accountId": a, "filter": {"uid": apple_group_uid}}),
    )
    .await;
    let apple_group = found["ids"][0]
        .as_str()
        .expect("the Apple group seen")
        .to_owned();
    assert_group(&backend, &alice, &apple_group, &[&old_member, &kept_member]).await;
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "update": {apple_group.clone(): {
            "members": {kept_member.clone(): true, carla_uid.clone(): true}
        }}}),
    )
    .await;
    assert_eq!(set["updated"], json!({apple_group.clone(): null}), "{set}");
    let text = dav_text(&other, &apple_group_href).await.unwrap();
    assert!(!text.contains(&old_member), "stale member:\n{text}");
    let names = |name: &str| -> Vec<String> {
        text.split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .filter(|(head, _)| {
                head.split(';')
                    .next()
                    .is_some_and(|n| n.rsplit('.').next().unwrap_or(n).eq_ignore_ascii_case(name))
            })
            .map(|(_, value)| value.to_owned())
            .collect()
    };
    let (kind, members) = if text.contains("VERSION:3.0") {
        assert!(
            names("KIND").is_empty() && names("MEMBER").is_empty(),
            "{text}"
        );
        (
            names("X-ADDRESSBOOKSERVER-KIND"),
            names("X-ADDRESSBOOKSERVER-MEMBER"),
        )
    } else {
        assert!(!text.to_uppercase().contains("ADDRESSBOOKSERVER"), "{text}");
        (names("KIND"), names("MEMBER"))
    };
    assert_eq!(kind.len(), 1, "{text}");
    assert!(kind[0].eq_ignore_ascii_case("group"), "{text}");
    let mut members = members;
    members.sort();
    let mut expected = vec![kept_member.clone(), carla_uid.clone()];
    expected.sort();
    assert_eq!(members, expected, "{text}");
    running.sync_round(true).await.unwrap();
    assert_group(&backend, &alice, &apple_group, &[&kept_member, &carla_uid]).await;
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "destroy": [apple_group]}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([apple_group.clone()]), "{set}");

    // Moved to the second book: same id, another href.
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "update": {apple.clone(): {"addressBookIds": {second.clone(): true}}}}),
    )
    .await;
    assert_eq!(set["updated"], json!({apple.clone(): null}), "{set}");
    assert!(dav_text(&other, &apple_href).await.is_none());
    let moved_href = card_href(&store, &alice, &apple).await;
    assert!(moved_href.is_member_of(&book_href(&store, &alice, &second).await));
    let moved = dav_text(&other, &moved_href).await.unwrap();
    assert!(moved.contains("X-CUSTOM-THING:keep me"), "{moved}");
    let got = one(
        &backend,
        &alice,
        "ContactCard/get",
        json!({"accountId": a, "ids": [apple], "properties": ["addressBookIds"]}),
    )
    .await;
    assert_eq!(
        got["list"][0]["addressBookIds"],
        json!({second.clone(): true})
    );
    // Two books at once is not a CardDAV card.
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "update": {apple.clone(): {"addressBookIds": {second.clone(): true, book.clone(): true}}}}),
    )
    .await;
    assert_eq!(
        set["notUpdated"][apple.as_str()]["type"],
        "invalidProperties"
    );

    // Books: rename, describe, unsubscribe.
    let set = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "update": {second.clone(): {
            "name": format!("{second_name} renamed"), "description": "Otra", "isSubscribed": false
        }}}),
    )
    .await;
    assert_eq!(set["updated"], json!({second.clone(): null}), "{set}");
    let got = one(
        &backend,
        &alice,
        "AddressBook/get",
        json!({"accountId": a, "ids": [second]}),
    )
    .await;
    assert_eq!(got["list"][0]["name"], format!("{second_name} renamed"));
    assert_eq!(got["list"][0]["description"], "Otra");
    assert_eq!(got["list"][0]["isSubscribed"], false);
    // Shareable on Stalwart (WebDAV ACL) and Nextcloud (`O:share`), not
    // on Radicale.
    let shareable = matches!(
        dav.kind,
        nebula_tests::DavKind::Stalwart | nebula_tests::DavKind::Nextcloud
    );
    assert_eq!(
        got["list"][0]["shareWith"],
        if shareable { json!({}) } else { Value::Null }
    );
    assert_eq!(got["list"][0]["myRights"]["mayShare"], shareable);
    let sort = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "update": {second.clone(): {"sortOrder": 5}}}),
    )
    .await;
    // Kept as Apple's `calendar-order` (a dead property for both servers).
    assert_eq!(sort["updated"], json!({second.clone(): null}), "{sort}");
    let got = one(
        &backend,
        &alice,
        "AddressBook/get",
        json!({"accountId": a, "ids": [second], "properties": ["sortOrder"]}),
    )
    .await;
    assert_eq!(got["list"][0]["sortOrder"], 5);
    let shared = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "update": {second.clone(): {"shareWith/x": {"mayRead": true}}}}),
    )
    .await;
    assert_eq!(
        shared["notUpdated"][second.as_str()]["type"],
        "invalidProperties"
    );

    // previewVcf: an uploaded .vcf, parsed without saving.
    let uploaded = backend
        .upload(
            &alice,
            "text/vcard".parse().unwrap(),
            UploadBody::from_chunks(vec![bytes::Bytes::from(apple_card("parse-me"))], 1_000_000),
        )
        .await
        .unwrap();
    let parsed = one(
        &backend,
        &alice,
        "ContactCard/parse",
        json!({"accountId": a, "blobIds": [uploaded.blob_id.as_str(), "bu999999"]}),
    )
    .await;
    let card = &parsed["parsed"][uploaded.blob_id.as_str()];
    assert_eq!(card["name"]["full"], "Ana Pérez");
    assert_eq!(card["uid"], "parse-me");
    assert!(card.get("id").is_none());
    assert_eq!(parsed["notFound"], json!(["bu999999"]));

    // deleteContacts.
    let before_delete = one(
        &backend,
        &alice,
        "ContactCard/get",
        json!({"accountId": a, "ids": []}),
    )
    .await["state"]
        .clone();
    let set = one(
        &backend,
        &alice,
        "ContactCard/set",
        json!({"accountId": a, "destroy": [carla]}),
    )
    .await;
    assert_eq!(set["destroyed"], json!([carla.clone()]), "{set}");
    assert!(dav_text(&other, &carla_href).await.is_none());

    // The changes since the start.
    let changes = one(
        &backend,
        &alice,
        "ContactCard/changes",
        json!({"accountId": a, "sinceState": first_state}),
    )
    .await;
    let ids = |key: &str| -> Vec<String> {
        changes[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    assert!(ids("created").contains(&group), "{changes}");
    assert!(ids("created").contains(&apple), "{changes}");
    // Created and destroyed in this window: in neither list (RFC 8620).
    assert!(!ids("created").contains(&carla), "{changes}");
    assert!(ids("destroyed").is_empty(), "{changes}");
    let changes = one(
        &backend,
        &alice,
        "ContactCard/changes",
        json!({"accountId": a, "sinceState": before_delete}),
    )
    .await;
    assert_eq!(changes["destroyed"], json!([carla.clone()]), "{changes}");
    assert_eq!(changes["created"], json!([]), "{changes}");

    // A book with cards goes only with onDestroyRemoveContents.
    let set = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "destroy": [book]}),
    )
    .await;
    assert_eq!(
        set["notDestroyed"][book.as_str()]["type"],
        "addressBookHasContents"
    );
    let set = one(
        &backend,
        &alice,
        "AddressBook/set",
        json!({"accountId": a, "destroy": [book, second], "onDestroyRemoveContents": true}),
    )
    .await;
    assert_eq!(set["destroyed"].as_array().unwrap().len(), 2, "{set}");
    let left = other.list_collections(home).await.unwrap();
    assert!(
        left.iter()
            .all(|c| c.display_name.as_deref() != Some(name.as_str())
                && !c
                    .display_name
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with(&second_name)),
        "test books left on the server"
    );
    let books = one(
        &backend,
        &alice,
        "AddressBook/get",
        json!({"accountId": a, "ids": [book, second]}),
    )
    .await;
    assert_eq!(books["notFound"].as_array().unwrap().len(), 2);
    backend.account_stopped(alice.id).await;
}
