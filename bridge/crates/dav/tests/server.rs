// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Integration tests against the DAV servers of `docker/compose.yaml`
//! (Stalwart, Radicale or Nextcloud, see `nebula-tests`).
//!
//! Skipped unless `NEBULA_TEST_DAV=1`. Test collections get unique names
//! and are deleted at the end.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "integration test helpers; a roundtrip test reads best in one piece"
)]

use std::time::{SystemTime, UNIX_EPOCH};

use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{
    CalendarAddress, CalendarColor, Component, ComponentSet, FreeBusyRequest, Href, Property,
    RequestStatus, ResourceName, SyncToken,
};
use nebula_dav::{
    CollectionKind, Credentials, DavClient, DavError, DavServer, Discovery, PutCondition,
    ResourceKind, TrashContents,
};
use nebula_tests::{DavKind, DavTestServer, User};
use secrecy::SecretString;

fn server() -> Option<DavTestServer> {
    DavTestServer::selected()
}

fn client_with(server: DavTestServer, password: SecretString) -> DavClient {
    let config = DavServer::new(&server.base_url())
        .unwrap()
        .with_private_network(true);
    let credentials = Credentials::new(server.login(User::Alice), password).unwrap();
    DavClient::new(config, Some(credentials)).unwrap()
}

fn client(server: DavTestServer) -> DavClient {
    client_with(server, server.password(User::Alice))
}

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{prefix}-{nanos}")
}

fn event(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula Mail//bridge tests//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261027T120000Z\r\n\
         DTSTART:20261028T100000Z\r\nDTEND:20261028T110000Z\r\nSUMMARY:{summary}\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn card(uid: &str, name: &str) -> String {
    format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nN:{name};;;;\r\n\
         EMAIL:{uid}@example.com\r\nEND:VCARD\r\n"
    )
}

/// Prefix of the collections the tests make.
const SCRATCH: &str = "nebula-test-";

async fn discover(client: &DavClient) -> Discovery {
    let found = client.discover().await.expect("discovery");
    sweep(client, &found).await;
    found
}

/// Deletes scratch collections of earlier runs that failed before their
/// cleanup (older than ten minutes, so a test running now keeps its own).
async fn sweep(client: &DavClient, found: &Discovery) {
    if found.capabilities.nc_calendar_trashbin {
        empty_trash_bin(client, &found.calendar_homes).await;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let old = |name: &str| {
        let Some(rest) = name.strip_prefix(SCRATCH) else {
            return false;
        };
        let nanos = rest.rsplit('-').next().and_then(|n| n.parse::<u128>().ok());
        nanos.is_some_and(|nanos| now.saturating_sub(nanos) > 600 * 1_000_000_000)
    };
    for home in found.calendar_homes.iter().chain(&found.addressbook_homes) {
        for collection in client.list_collections(home).await.unwrap_or_default() {
            if collection.href.last_segment().is_some_and(old) {
                let _ = client.delete(&collection.href, None).await;
            }
        }
    }
}

/// Nextcloud keeps deleted calendars in its trash bin, and they count
/// toward its limit of calendars per user: the scratch collections of
/// earlier runs are deleted for good (a throwaway test server).
async fn empty_trash_bin(client: &DavClient, homes: &[Href]) {
    for home in homes {
        let Ok(bin) = client.trash_bin(home, TrashContents::HrefsOnly).await else {
            continue;
        };
        for href in bin
            .calendars
            .iter()
            .map(|c| &c.href)
            .chain(bin.events.iter().map(|e| &e.href))
        {
            let _ = client.delete_forever(href).await;
        }
    }
}

/// Alice's own collection for a test, created under her first home.
async fn scratch(client: &DavClient, home: &Href, kind: CollectionKind, label: &str) -> Href {
    let name = ResourceName::try_from(unique(&format!("{SCRATCH}{label}")).as_str()).unwrap();
    let href = home.member(&name, true).unwrap();
    let display = PropValue::DisplayName(format!("Nebula <{label}>&co ñ"));
    match kind {
        CollectionKind::Calendar => {
            let components = PropValue::SupportedComponents(ComponentSet::new(&[Component::Event]));
            client
                .mkcalendar(&href, &[display, components])
                .await
                .expect("MKCALENDAR");
        }
        CollectionKind::Addressbook => client
            .mkcol(&href, kind, &[display])
            .await
            .expect("extended MKCOL"),
    }
    href
}

#[tokio::test]
async fn discovery_finds_alices_homes() {
    let Some(server) = server() else { return };
    let client = client(server);
    let found = discover(&client).await;
    let mine = |href: &Href| href.segments().iter().any(|s| s.starts_with("alice"));
    assert!(
        mine(&found.principal),
        "principal {:?}",
        found.principal.path()
    );
    assert!(!found.calendar_homes.is_empty() && found.calendar_homes.iter().all(mine));
    assert!(!found.addressbook_homes.is_empty() && found.addressbook_homes.iter().all(mine));
    assert!(found.capabilities.calendar_access);
    assert!(found.capabilities.addressbook);
    // Radicale does no scheduling, so the bridge would send invitations.
    if server.kind == DavKind::Radicale {
        assert!(!found.capabilities.calendar_auto_schedule);
    }
    if server.kind == DavKind::Nextcloud {
        assert!(found.capabilities.calendar_auto_schedule);
        // Its own sharing; it announces Calendar Server sharing too, but
        // answers a `CS:share` with 501.
        assert!(found.capabilities.oc_resource_sharing);
        assert!(found.capabilities.calendarserver_sharing);
    }
    if matches!(server.kind, DavKind::Stalwart | DavKind::Nextcloud) {
        assert!(found.capabilities.calendar_auto_schedule);
        assert!(
            found
                .calendar_user_addresses
                .iter()
                .any(|a| a.eq_ignore_ascii_case("mailto:alice@nebulamail.test")),
            "{:?}",
            found.calendar_user_addresses
        );
    }

    // Her collections, and a new one of each kind among them.
    let calendar_home = &found.calendar_homes[0];
    let addressbook_home = &found.addressbook_homes[0];
    let calendar = scratch(&client, calendar_home, CollectionKind::Calendar, "cal").await;
    let book = scratch(
        &client,
        addressbook_home,
        CollectionKind::Addressbook,
        "book",
    )
    .await;
    let calendars = client.list_collections(calendar_home).await.unwrap();
    let listed = calendars
        .iter()
        .find(|c| c.href == calendar)
        .expect("new calendar listed");
    assert_eq!(listed.kind, CollectionKind::Calendar);
    assert_eq!(listed.display_name.as_deref(), Some("Nebula <cal>&co ñ"));
    assert!(listed.supports_sync());
    if let Some(components) = &listed.components {
        assert!(components.contains(Component::Event));
    }
    if let Some(privileges) = listed.privileges {
        assert!(privileges.can_read() && privileges.can_write_content());
    }
    let books = client.list_collections(addressbook_home).await.unwrap();
    let listed = books
        .iter()
        .find(|c| c.href == book)
        .expect("new book listed");
    assert_eq!(listed.kind, CollectionKind::Addressbook);

    // PROPPATCH the name and colour.
    let failed = client
        .proppatch(
            &calendar,
            &[
                PropValue::DisplayName("Renombrado".into()),
                PropValue::CalendarColor(CalendarColor::parse("#FF8800").unwrap()),
            ],
            &[],
        )
        .await
        .unwrap();
    assert!(failed.is_empty(), "{failed:?}");
    let renamed = client.list_collections(calendar_home).await.unwrap();
    let renamed = renamed.iter().find(|c| c.href == calendar).unwrap();
    assert_eq!(renamed.display_name.as_deref(), Some("Renombrado"));
    assert!(renamed.color.is_some());

    client.delete(&calendar, None).await.unwrap();
    client.delete(&book, None).await.unwrap();
    let after = client.list_collections(calendar_home).await.unwrap();
    assert!(after.iter().all(|c| c.href != calendar));
}

#[tokio::test]
async fn calendar_put_get_sync_delete() {
    let Some(server) = server() else { return };
    let client = client(server);
    let found = discover(&client).await;
    let home = &found.calendar_homes[0];
    let calendar = scratch(&client, home, CollectionKind::Calendar, "sync").await;

    let start = client.sync(&calendar, None).await.unwrap();
    assert!(start.full && start.changed.is_empty() && start.removed.is_empty());

    let uid = unique("nebula-event");
    let href = calendar
        .member(
            &ResourceName::try_from(format!("{uid}.ics").as_str()).unwrap(),
            false,
        )
        .unwrap();
    let created = client
        .put(
            &href,
            ResourceKind::Calendar,
            &event(&uid, "Primera"),
            &PutCondition::Create,
        )
        .await
        .unwrap();
    // Creating it again must not overwrite it.
    let again = client
        .put(
            &href,
            ResourceKind::Calendar,
            &event(&uid, "Otra"),
            &PutCondition::Create,
        )
        .await;
    assert!(
        matches!(again, Err(DavError::PreconditionFailed)),
        "{again:?}"
    );

    let read = client.get(&href, ResourceKind::Calendar).await.unwrap();
    assert!(read.data.contains(&format!("UID:{uid}")));
    assert!(read.data.contains("SUMMARY:Primera"));
    let etag = read
        .etag
        .clone()
        .or(created)
        .expect("an etag from PUT or GET");

    let changes = client.sync(&calendar, Some(&start.token)).await.unwrap();
    assert!(!changes.full);
    assert_eq!(changes.changed.len(), 1, "{changes:?}");
    assert_eq!(changes.changed[0].href, href);
    let fetched = client
        .multiget(&calendar, ResourceKind::Calendar, &changes.changed_hrefs())
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].href, href);
    assert!(
        fetched[0]
            .data
            .as_deref()
            .unwrap()
            .contains(&format!("UID:{uid}\r\n"))
    );
    assert!(fetched[0].etag.is_some());

    // A stale etag is refused; the right one works.
    let stale = nebula_dav::proto::ETag::parse("\"not-the-etag\"").unwrap();
    let refused = client
        .put(
            &href,
            ResourceKind::Calendar,
            &event(&uid, "Tarde"),
            &PutCondition::Update(stale),
        )
        .await;
    assert!(
        matches!(refused, Err(DavError::PreconditionFailed)),
        "{refused:?}"
    );
    let updated = client
        .put(
            &href,
            ResourceKind::Calendar,
            &event(&uid, "Segunda"),
            &PutCondition::Update(etag),
        )
        .await
        .unwrap();
    let etag = match updated {
        Some(etag) => etag,
        None => client
            .get(&href, ResourceKind::Calendar)
            .await
            .unwrap()
            .etag
            .unwrap(),
    };

    // Without sync-collection: ctag/token and the etag listing agree.
    let markers = client.collection_state(&calendar).await.unwrap();
    assert!(markers.ctag.is_some() || markers.sync_token.is_some());
    let members = client.list_members(&calendar).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].href, href);

    // MOVE to another calendar and back.
    let other = scratch(&client, home, CollectionKind::Calendar, "move").await;
    let moved = other
        .member(&ResourceName::try_from("moved.ics").unwrap(), false)
        .unwrap();
    client
        .move_to(&href, &moved, Some(&etag), false)
        .await
        .unwrap();
    assert!(matches!(
        client.get(&href, ResourceKind::Calendar).await,
        Err(DavError::NotFound)
    ));
    let moved_etag = client
        .get(&moved, ResourceKind::Calendar)
        .await
        .unwrap()
        .etag;
    client
        .move_to(&moved, &href, moved_etag.as_ref(), false)
        .await
        .unwrap();
    let etag = client
        .get(&href, ResourceKind::Calendar)
        .await
        .unwrap()
        .etag;

    let before_delete = client.sync(&calendar, None).await.unwrap();
    client.delete(&href, etag.as_ref()).await.unwrap();
    let gone = client
        .sync(&calendar, Some(&before_delete.token))
        .await
        .unwrap();
    assert_eq!(gone.removed, std::slice::from_ref(&href), "{gone:?}");
    assert!(matches!(
        client.get(&href, ResourceKind::Calendar).await,
        Err(DavError::NotFound)
    ));

    // A token the server never gave: Radicale says `valid-sync-token` and
    // the client starts over (`full`). Stalwart 0.16 answers with every
    // member and no error, so nothing tells it apart from a normal sync.
    let bogus = SyncToken::parse("http://nebula.invalid/sync/0").unwrap();
    let restarted = client.sync(&calendar, Some(&bogus)).await.unwrap();
    if server.kind == DavKind::Radicale {
        assert!(restarted.full, "{restarted:?}");
    }

    client.delete(&other, None).await.unwrap();
    client.delete(&calendar, None).await.unwrap();
}

#[tokio::test]
async fn contacts_put_multiget_delete() {
    let Some(server) = server() else { return };
    let client = client(server);
    let found = discover(&client).await;
    let book = scratch(
        &client,
        &found.addressbook_homes[0],
        CollectionKind::Addressbook,
        "cards",
    )
    .await;

    let start = client.sync(&book, None).await.unwrap();
    let mut hrefs = Vec::new();
    for i in 0..3 {
        let uid = unique(&format!("nebula-card-{i}"));
        let href = book
            .member(
                &ResourceName::try_from(format!("{uid}.vcf").as_str()).unwrap(),
                false,
            )
            .unwrap();
        client
            .put(
                &href,
                ResourceKind::Contact,
                &card(&uid, &format!("Persona {i}")),
                &PutCondition::Create,
            )
            .await
            .unwrap();
        hrefs.push(href);
    }
    let changes = client.sync(&book, Some(&start.token)).await.unwrap();
    assert_eq!(changes.changed.len(), 3, "{changes:?}");
    let fetched = client
        .multiget(&book, ResourceKind::Contact, &hrefs)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 3);
    for item in &fetched {
        assert!(item.data.as_deref().unwrap().starts_with("BEGIN:VCARD\r\n"));
    }
    let card = client.get(&hrefs[0], ResourceKind::Contact).await.unwrap();
    assert!(card.data.contains("FN:Persona 0"));

    // An href outside the book is refused before any request.
    let outside = found.addressbook_homes[0]
        .member(&ResourceName::try_from("x.vcf").unwrap(), false)
        .unwrap();
    assert!(matches!(
        client
            .multiget(&book, ResourceKind::Contact, &[outside])
            .await,
        Err(DavError::OutsideCollection)
    ));

    for href in &hrefs {
        client.delete(href, None).await.unwrap();
    }
    assert!(client.list_members(&book).await.unwrap().is_empty());
    client.delete(&book, None).await.unwrap();
}

#[tokio::test]
async fn wrong_password_is_unauthorized() {
    let Some(server) = server() else { return };
    let client = client_with(server, SecretString::from("definitely-not-the-password"));
    let result = client.discover().await;
    assert!(matches!(result, Err(DavError::Unauthorized)), "{result:?}");
}

/// Free-busy and principals (libre/ocupado). Stalwart (0.16.27 and 1.0.1)
/// has an outbox that answers a VFREEBUSY request (alice's own busy times;
/// bob, who shares nothing with alice, with 3.8 since 1.0 (0.16.27: an
/// empty reply); someone from outside with 3.7, a `.invalid` domain not at
/// all) and lists its
/// principals under `/dav/pal/` with their `mailto:` addresses; clients may
/// search `displayname` and, since 1.0, `calendar-user-address-set`.
/// Radicale 3.8 has no outbox (no scheduling), shows only alice's own
/// principal under `/`, with a path as its "address", and answers the
/// searchable properties with an empty multistatus.
#[tokio::test]
async fn free_busy_and_principals() {
    let Some(server) = server() else { return };
    let client = client(server);
    let found = discover(&client).await;
    assert!(!found.principal_collections.is_empty());
    let collection = &found.principal_collections[0];
    // Nextcloud does not list its users (405, "Listing members of this
    // collection is disabled"): only searches find them.
    let listed = match client.list_principals(collection).await {
        Err(DavError::MethodNotAllowed) if server.kind == DavKind::Nextcloud => Vec::new(),
        other => other.unwrap(),
    };
    let alice = User::Alice.address();
    let bob = User::Bob.address();
    let has = |address: &str| {
        listed.iter().any(|p| {
            p.addresses
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&format!("mailto:{address}")))
        })
    };
    match server.kind {
        DavKind::Radicale => {
            assert_eq!(found.schedule_outbox, None);
            assert!(!found.capabilities.calendar_auto_schedule);
            assert_eq!(listed.len(), 1, "{listed:?}");
            assert!(listed[0].href.same_location(&found.principal));
            assert!(!has(&alice), "Radicale has no mailto: addresses");
            let searchable = client.principal_search_properties(collection).await;
            assert!(searchable.is_err(), "{searchable:?}");
        }
        DavKind::Nextcloud => {
            assert!(listed.is_empty(), "{listed:?}");
            // The users are searchable by name (a substring of the name
            // or the address) and by address (sabre's `email-address` and
            // `calendar-user-address-set`, the address without `mailto:`,
            // whole and without case).
            assert_eq!(
                client
                    .principal_search_properties(collection)
                    .await
                    .unwrap(),
                [
                    Property::DisplayName,
                    Property::EmailAddress,
                    Property::CalendarUserAddressSet
                ]
            );
            let by_address = client
                .search_principals(collection, Property::EmailAddress, &bob.to_uppercase())
                .await
                .unwrap();
            assert_eq!(by_address.len(), 1, "{by_address:?}");
            let by_address = client
                .search_principals(collection, Property::CalendarUserAddressSet, &bob)
                .await
                .unwrap();
            assert_eq!(by_address.len(), 1, "{by_address:?}");
            let by_name = client
                .search_principals(collection, Property::DisplayName, "@")
                .await
                .unwrap();
            assert!(by_name.len() >= 2, "{by_name:?}");
            assert!(
                by_address[0]
                    .addresses
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&format!("mailto:{bob}"))),
                "{by_address:?}"
            );
            let partial = client
                .search_principals(collection, Property::EmailAddress, "nebulamail.test")
                .await
                .unwrap();
            assert!(partial.is_empty(), "{partial:?}");
            let outbox = found.schedule_outbox.clone().expect("outbox");
            let now = i64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            )
            .unwrap();
            let address = |a: &str| CalendarAddress::parse(a).unwrap();
            let request = FreeBusyRequest {
                uid: unique("nebula-fb"),
                stamp: now,
                start: now,
                end: now + 14 * 86_400,
                organizer: address(&alice),
                attendees: vec![address(&alice), address(&bob)],
            };
            let answers = client.free_busy(&outbox, &request).await.unwrap();
            assert_eq!(answers.len(), 2, "{answers:?}");
            assert!(answers[0].has_data(), "{answers:?}");
            assert!(answers[1].has_data(), "{answers:?}");
        }
        DavKind::Stalwart => {
            assert!(found.capabilities.calendar_auto_schedule);
            assert!(has(&alice) && has(&bob), "{listed:?}");
            // Stalwart 1.0 searches addresses too (0.16.27 only names),
            // with `Sharing.allowDirectoryQueries` on (`dev:setup`).
            assert_eq!(
                client
                    .principal_search_properties(collection)
                    .await
                    .unwrap(),
                [Property::DisplayName, Property::CalendarUserAddressSet]
            );
            let by_address = client
                .search_principals(collection, Property::CalendarUserAddressSet, &bob)
                .await
                .unwrap();
            assert_eq!(by_address.len(), 1, "{by_address:?}");
            assert!(
                by_address[0]
                    .addresses
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&format!("mailto:{bob}"))),
                "{by_address:?}"
            );

            let outbox = found.schedule_outbox.clone().expect("outbox");
            let now = i64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            )
            .unwrap();
            let address = |a: &str| CalendarAddress::parse(a).unwrap();
            let request = FreeBusyRequest {
                uid: unique("nebula-fb"),
                stamp: now,
                start: now,
                end: now + 14 * 86_400,
                organizer: address(&alice),
                attendees: vec![
                    address(&alice),
                    address(&bob),
                    address("nobody@example.com"),
                    address("nobody@nebula-test.invalid"),
                ],
            };
            let answers = client.free_busy(&outbox, &request).await.unwrap();
            assert_eq!(answers.len(), 4);
            assert!(answers[0].has_data(), "{answers:?}");
            assert!(
                answers[0]
                    .calendar_data
                    .as_deref()
                    .unwrap()
                    .contains("BEGIN:VFREEBUSY\r\n")
            );
            // Bob shares nothing with alice: 1.0 answers 3.8 ("no
            // authority"), 0.16.27 gave an empty success.
            assert!(!answers[1].has_data(), "{answers:?}");
            assert_eq!(
                answers[1].status.as_ref().map(RequestStatus::code),
                Some("3.8"),
                "{answers:?}"
            );
            assert!(!answers[2].has_data(), "{answers:?}");
            // Someone from outside: 3.7; a domain that cannot exist is
            // left out of the answer.
            assert_eq!(
                answers[2].status.as_ref().map(RequestStatus::class),
                Some(3),
                "{answers:?}"
            );
            assert_eq!(answers[3].status, None, "{answers:?}");
        }
    }
}

/// Properties the server does not have come back as failed, not as an
/// error.
#[tokio::test]
async fn unknown_properties_are_reported() {
    let Some(server) = server() else { return };
    let client = client(server);
    let found = discover(&client).await;
    let calendar = scratch(
        &client,
        &found.calendar_homes[0],
        CollectionKind::Calendar,
        "props",
    )
    .await;
    let failed = client
        .proppatch(&calendar, &[], &[Property::CalendarDescription])
        .await
        .unwrap();
    // Removing a property that is not there is fine for every server.
    assert!(
        failed
            .iter()
            .all(|(p, _)| *p == Property::CalendarDescription)
    );
    client.delete(&calendar, None).await.unwrap();
}
