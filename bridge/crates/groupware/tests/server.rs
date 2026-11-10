// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The read-only sync against the DAV servers of `docker/compose.yaml`
//! (Stalwart or Radicale, see `nebula-tests`): what another client writes
//! with the DAV client is seen by the next round, the state goes up and a
//! push event goes out.
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

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nebula_crypto::{AccountId as KeyId, AccountKey, DataKey, KeyVersion, Keyring, MasterKey};
use nebula_dav::proto::request::PropValue;
use nebula_dav::proto::{Component, ComponentSet, Href, ResourceName};
use nebula_dav::{CollectionKind, Credentials, DavClient, DavServer, PutCondition, ResourceKind};
use nebula_groupware::model::sealed;
use nebula_groupware::{DavStatus, Groupware, GroupwareConfig, GroupwareEvent};
use nebula_store::{
    AccountId, ChangesSince, DataType, NewAccount, Parsed, ResourceId, Seq, Store, StoredResource,
    UnixTime,
};
use nebula_tests::{DavTestServer, User};
use secrecy::ExposeSecret as _;
use tokio::sync::broadcast;

fn unique(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("nebula-test-{prefix}-{nanos}")
}

fn event(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula Mail//bridge tests//EN\r\n\
         BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTAMP:20261027T120000Z\r\n\
         DTSTART:20261028T100000Z\r\nDTEND:20261028T110000Z\r\nSUMMARY:{summary}\r\n\
         END:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn card(uid: &str, family: &str, given: &str) -> String {
    format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{given} {family}\r\nN:{family};{given};;;\r\n\
         EMAIL:{uid}@example.com\r\nEND:VCARD\r\n"
    )
}

/// The "other client": the DAV client on its own.
fn other_client(server: DavTestServer) -> DavClient {
    let config = DavServer::new(&server.base_url())
        .unwrap()
        .with_private_network(true);
    let credentials =
        Credentials::new(server.login(User::Alice), server.password(User::Alice)).unwrap();
    DavClient::new(config, Some(credentials)).unwrap()
}

/// Nextcloud keeps deleted calendars in its trash bin, and they count
/// toward its limit of calendars per user: what earlier runs deleted goes
/// for good (a throwaway test server).
async fn empty_trash_bin(server: DavTestServer) {
    let client = other_client(server);
    let Ok(found) = client.discover().await else {
        return;
    };
    if !found.capabilities.nc_calendar_trashbin {
        return;
    }
    for home in &found.calendar_homes {
        let Ok(bin) = client
            .trash_bin(home, nebula_dav::TrashContents::HrefsOnly)
            .await
        else {
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

struct Bridge {
    store: Store,
    groupware: Groupware,
    id: AccountId,
    key: Arc<AccountKey>,
    events: broadcast::Receiver<GroupwareEvent>,
    _dir: tempfile::TempDir,
}

async fn bridge(server: DavTestServer) -> (Bridge, Arc<nebula_groupware::GroupwareAccount>) {
    empty_trash_bin(server).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let id = store
        .get_or_create_account(
            NewAccount {
                login: "alice",
                email: "alice@nebulamail.test",
                wrapped_dek: b"k",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap()
        .id;
    let dav = DavServer::new(&server.base_url())
        .unwrap()
        .with_private_network(true);
    let mut config = GroupwareConfig::new(dav);
    config.poll_with_push = Duration::from_hours(1);
    config.poll_without_push = Duration::from_hours(1);
    let resolver = Arc::new(net_guard::SystemResolver::new().unwrap());
    let groupware = Groupware::new(store.clone(), config, resolver);
    let events = groupware.subscribe();
    // Two copies of one data key (wrapped once, unwrapped twice, like the
    // bridge does at each login): one for the bridge, one for the test to
    // open the sealed copy.
    let key_id = KeyId::new(b"login:alice").unwrap();
    let master = MasterKey::from_bytes(KeyVersion::new(1).unwrap(), &[7; 32]).unwrap();
    let keyring = Keyring::new(master, []).unwrap();
    let wrapped = keyring
        .wrap(&key_id, &DataKey::generate().unwrap())
        .unwrap();
    let copy = || AccountKey::new(key_id.clone(), keyring.unwrap(&key_id, &wrapped).unwrap());
    let account = groupware
        .open(
            id,
            server.login(User::Alice),
            server.password(User::Alice),
            copy(),
        )
        .unwrap();
    for _ in 0..600 {
        match account.status() {
            DavStatus::Ready => break,
            DavStatus::Unauthorized => panic!("the DAV server refused alice"),
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    assert_eq!(account.status(), DavStatus::Ready, "first round");
    let key = Arc::new(copy());
    (
        Bridge {
            store,
            groupware,
            id,
            key,
            events,
            _dir: dir,
        },
        account,
    )
}

fn drain(events: &mut broadcast::Receiver<GroupwareEvent>) -> Vec<GroupwareEvent> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(event);
    }
    out
}

fn pushed(events: &[GroupwareEvent], account: AccountId, data_type: DataType) -> bool {
    events.iter().any(|event| {
        matches!(event, GroupwareEvent::StateChanged(changed)
            if changed.account == account && changed.data_types.contains(&data_type))
    })
}

/// (created, updated, destroyed) since `from`.
async fn delta(bridge: &Bridge, data_type: DataType, from: Seq) -> (Vec<i64>, Vec<i64>, Vec<i64>) {
    match bridge
        .store
        .changes_since(bridge.id, data_type, from, 10_000)
        .await
        .unwrap()
    {
        ChangesSince::Changes(c) => (c.created, c.updated, c.destroyed),
        ChangesSince::CannotCalculate | ChangesSince::TooMany => panic!("cannot calculate"),
    }
}

/// The stored resource at `href`, if visible.
async fn stored(bridge: &Bridge, collection: &Href, href: &Href) -> Option<StoredResource> {
    let collection = bridge
        .store
        .dav_collections(bridge.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.orphaned_at.is_none() && c.props.href.as_str() == collection.url().as_str())?;
    bridge
        .store
        .dav_resources(bridge.id, collection.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.is_visible() && r.href.as_str() == href.url().as_str())
}

async fn scratch(client: &DavClient, home: &Href, kind: CollectionKind) -> Href {
    let name = ResourceName::try_from(unique("sync").as_str()).unwrap();
    let href = home.member(&name, true).unwrap();
    let display = PropValue::DisplayName("Nebula sync test".to_owned());
    match kind {
        CollectionKind::Calendar => {
            let components = PropValue::SupportedComponents(ComponentSet::new(&[Component::Event]));
            client
                .mkcalendar(&href, &[display, components])
                .await
                .unwrap();
        }
        CollectionKind::Addressbook => client.mkcol(&href, kind, &[display]).await.unwrap(),
    }
    href
}

#[tokio::test]
async fn another_clients_writes_are_seen_and_pushed() {
    let Some(server) = DavTestServer::selected() else {
        return;
    };
    let other = other_client(server);
    let found = other.discover().await.unwrap();
    let (mut bridge, account) = bridge(server).await;
    // It also stored a probe sealed with the account's key: the copy is
    // not readable with another key.
    drop(drain(&mut bridge.events));

    // A new calendar and a new address book.
    let calendar = scratch(&other, &found.calendar_homes[0], CollectionKind::Calendar).await;
    let book = scratch(
        &other,
        &found.addressbook_homes[0],
        CollectionKind::Addressbook,
    )
    .await;
    let calendars = bridge
        .store
        .state(bridge.id, DataType::Calendar)
        .await
        .unwrap();
    let books = bridge
        .store
        .state(bridge.id, DataType::AddressBook)
        .await
        .unwrap();
    account.sync_round(false).await.unwrap();
    assert_eq!(
        delta(&bridge, DataType::Calendar, calendars).await.0.len(),
        1
    );
    assert_eq!(
        delta(&bridge, DataType::AddressBook, books).await.0.len(),
        1
    );
    let events = drain(&mut bridge.events);
    assert!(pushed(&events, bridge.id, DataType::Calendar));
    assert!(pushed(&events, bridge.id, DataType::AddressBook));

    // Another client creates an event and a card.
    let uid = unique("event");
    let event_href = calendar
        .member(
            &ResourceName::try_from(format!("{uid}.ics").as_str()).unwrap(),
            false,
        )
        .unwrap();
    other
        .put(
            &event_href,
            ResourceKind::Calendar,
            &event(&uid, "Primera"),
            &PutCondition::Create,
        )
        .await
        .unwrap();
    let card_uid = unique("card");
    let card_href = book
        .member(
            &ResourceName::try_from(format!("{card_uid}.vcf").as_str()).unwrap(),
            false,
        )
        .unwrap();
    other
        .put(
            &card_href,
            ResourceKind::Contact,
            &card(&card_uid, "Zapata", "Ana"),
            &PutCondition::Create,
        )
        .await
        .unwrap();
    let events_state = bridge
        .store
        .state(bridge.id, DataType::CalendarEvent)
        .await
        .unwrap();
    let cards_state = bridge
        .store
        .state(bridge.id, DataType::ContactCard)
        .await
        .unwrap();
    account.sync_round(false).await.unwrap();
    let seen = stored(&bridge, &calendar, &event_href)
        .await
        .expect("event synced");
    match &seen.parsed {
        Parsed::Valid(index) => {
            assert_eq!(index.uid.as_deref(), Some(uid.as_str()));
            assert_eq!(index.utc_start.map(UnixTime::secs), Some(1_793_181_600));
            assert_eq!(index.utc_end.map(UnixTime::secs), Some(1_793_185_200));
        }
        Parsed::Invalid => panic!("the event parses"),
    }
    assert_eq!(
        delta(&bridge, DataType::CalendarEvent, events_state)
            .await
            .0,
        [seen.id.get()]
    );
    let card_seen = stored(&bridge, &book, &card_href)
        .await
        .expect("card synced");
    match &card_seen.parsed {
        Parsed::Valid(index) => assert_eq!(index.sort_name.as_deref(), Some("zapata ana")),
        Parsed::Invalid => panic!("the card parses"),
    }
    assert_eq!(
        delta(&bridge, DataType::ContactCard, cards_state).await.0,
        [card_seen.id.get()]
    );
    let events = drain(&mut bridge.events);
    assert!(pushed(&events, bridge.id, DataType::CalendarEvent));
    assert!(pushed(&events, bridge.id, DataType::ContactCard));
    // The local copy is sealed with the account's key: not in clear.
    assert!(text_of(&bridge, seen.id).await.contains("SUMMARY:Primera"));
    let sealed = bridge
        .store
        .dav_resource_data(bridge.id, seen.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!String::from_utf8_lossy(&sealed).contains(&uid));

    // Another client changes it: updated.
    let read = other
        .get(&event_href, ResourceKind::Calendar)
        .await
        .unwrap();
    let etag = read.etag.expect("the server gives an etag");
    other
        .put(
            &event_href,
            ResourceKind::Calendar,
            &event(&uid, "Cambiada"),
            &PutCondition::Update(etag),
        )
        .await
        .unwrap();
    let events_state = bridge
        .store
        .state(bridge.id, DataType::CalendarEvent)
        .await
        .unwrap();
    account.sync_round(false).await.unwrap();
    let (created, updated, destroyed) = delta(&bridge, DataType::CalendarEvent, events_state).await;
    assert!(created.is_empty() && destroyed.is_empty());
    assert_eq!(updated, [seen.id.get()]);
    assert!(pushed(
        &drain(&mut bridge.events),
        bridge.id,
        DataType::CalendarEvent
    ));
    assert!(text_of(&bridge, seen.id).await.contains("SUMMARY:Cambiada"));

    // Nothing more changed: nothing recorded.
    let events_state = bridge
        .store
        .state(bridge.id, DataType::CalendarEvent)
        .await
        .unwrap();
    account.sync_round(false).await.unwrap();
    assert_eq!(
        bridge
            .store
            .state(bridge.id, DataType::CalendarEvent)
            .await
            .unwrap(),
        events_state
    );

    // And deletes it: destroyed.
    let cards_state = bridge
        .store
        .state(bridge.id, DataType::ContactCard)
        .await
        .unwrap();
    other.delete(&event_href, None).await.unwrap();
    other.delete(&card_href, None).await.unwrap();
    account.sync_round(false).await.unwrap();
    let (_, _, destroyed) = delta(&bridge, DataType::CalendarEvent, events_state).await;
    assert_eq!(destroyed, [seen.id.get()]);
    let (_, _, destroyed) = delta(&bridge, DataType::ContactCard, cards_state).await;
    assert_eq!(destroyed, [card_seen.id.get()]);

    // A full comparison (the daily one) changes nothing.
    let events_state = bridge
        .store
        .state(bridge.id, DataType::CalendarEvent)
        .await
        .unwrap();
    account.sync_round(true).await.unwrap();
    assert_eq!(
        bridge
            .store
            .state(bridge.id, DataType::CalendarEvent)
            .await
            .unwrap(),
        events_state
    );

    // Cleanup: the collections go, and the bridge sees them go.
    other.delete(&calendar, None).await.unwrap();
    other.delete(&book, None).await.unwrap();
    let calendars = bridge
        .store
        .state(bridge.id, DataType::Calendar)
        .await
        .unwrap();
    let books = bridge
        .store
        .state(bridge.id, DataType::AddressBook)
        .await
        .unwrap();
    account.sync_round(false).await.unwrap();
    assert_eq!(
        delta(&bridge, DataType::Calendar, calendars).await.2.len(),
        1
    );
    assert_eq!(
        delta(&bridge, DataType::AddressBook, books).await.2.len(),
        1
    );
    let live = bridge.store.dav_collections(bridge.id).await.unwrap();
    for gone in [&calendar, &book] {
        assert!(
            live.iter()
                .filter(|c| c.orphaned_at.is_none())
                .all(|c| c.props.href.as_str() != gone.url().as_str())
        );
    }
    bridge.groupware.close(bridge.id).await;
}

/// The text of a stored resource, opened with the test's copy of the key.
async fn text_of(bridge: &Bridge, id: ResourceId) -> String {
    let sealed = bridge
        .store
        .dav_resource_data(bridge.id, id)
        .await
        .unwrap()
        .unwrap();
    let resource = bridge
        .store
        .dav_resource(bridge.id, id)
        .await
        .unwrap()
        .unwrap();
    let text = sealed::open(&bridge.key, resource.collection, &resource.href, &sealed).unwrap();
    String::from_utf8(text.expose_secret().to_vec()).unwrap()
}
