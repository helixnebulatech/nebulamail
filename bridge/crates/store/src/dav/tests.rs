// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use crate::account::tests::alice;
use crate::db::test_support::store;
use crate::{
    AccountId, CalendarExtras, Changes, ChangesSince, CollectionId, DataType, ExternalUrl, Seq,
    Store, StoreError, UnixTime,
};

fn href(path: &str) -> DavHref {
    DavHref::new(&format!("https://dav.example.com{path}")).unwrap()
}

fn calendar(path: &str, name: &str) -> CollectionProps {
    CollectionProps {
        kind: CollectionKind::Calendar,
        href: href(path),
        name: Some(name.to_owned()),
        color: Some("#ff0000".to_owned()),
        description: None,
        sort_order: None,
        time_zone: None,
        default_alarms_datetime: None,
        default_alarms_date: None,
        rights: Some(Rights::READ.union(Rights::WRITE_CONTENT)),
        components: Some(Components::EVENT),
        vcard_versions: None,
        supports_sync: true,
        sharing: CollectionSharing::default(),
        extras: CalendarExtras::default(),
    }
}

fn book(path: &str) -> CollectionProps {
    CollectionProps {
        kind: CollectionKind::AddressBook,
        components: None,
        vcard_versions: Some(VcardVersions::V3.union(VcardVersions::V4)),
        ..calendar(path, "Contacts")
    }
}

fn event(path: &str, uid: &str, etag: &str) -> ResourceWrite {
    ResourceWrite {
        href: href(path),
        etag: Some(etag.to_owned()),
        sealed: format!("sealed {uid} {etag}").into_bytes(),
        size: 100,
        parsed: Parsed::Valid(ResourceIndex {
            uid: Some(uid.to_owned()),
            utc_start: Some(UnixTime::from_secs(1000)),
            utc_end: None,
            recurring: true,
            sort_name: None,
        }),
    }
}

fn changes(result: ChangesSince) -> Changes {
    match result {
        ChangesSince::Changes(changes) => changes,
        ChangesSince::CannotCalculate | ChangesSince::TooMany => panic!("cannot calculate"),
    }
}

async fn since(store: &Store, account: AccountId, data_type: DataType, from: Seq) -> Changes {
    changes(
        store
            .changes_since(account, data_type, from, 100)
            .await
            .unwrap(),
    )
}

const T: UnixTime = UnixTime::from_secs(5000);

#[tokio::test(flavor = "multi_thread")]
async fn collections_are_created_updated_hidden_and_revived() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let work = calendar("/cal/alice/work/", "Work");
    let home = calendar("/cal/alice/home/", "Home");
    let (recorded, live) = store
        .apply_collections(
            account,
            vec![work.clone(), home.clone(), book("/card/alice/default/")],
            T,
        )
        .await
        .unwrap();
    assert_eq!(live.len(), 3);
    let cal_state = recorded.state(DataType::Calendar).unwrap();
    assert!(recorded.state(DataType::AddressBook).is_some());
    let first = since(&store, account, DataType::Calendar, Seq::ZERO).await;
    assert_eq!(first.created.len(), 2);
    // One default per kind: the first calendar by id (no sort order).
    let defaults: Vec<&str> = live
        .iter()
        .filter(|c| c.is_default)
        .map(|c| c.props.href.as_str())
        .collect();
    assert_eq!(
        defaults,
        [
            "https://dav.example.com/cal/alice/work/",
            "https://dav.example.com/card/alice/default/"
        ]
    );

    // Same listing: nothing recorded.
    let (again, _) = store
        .apply_collections(
            account,
            vec![work.clone(), home.clone(), book("/card/alice/default/")],
            T,
        )
        .await
        .unwrap();
    assert!(again.is_empty());
    // Only the sync support changed: not visible, nothing recorded.
    let no_sync = CollectionProps {
        supports_sync: false,
        ..work.clone()
    };
    let (quiet, _) = store
        .apply_collections(
            account,
            vec![no_sync, home.clone(), book("/card/alice/default/")],
            T,
        )
        .await
        .unwrap();
    assert!(quiet.is_empty());

    // Renamed, and "home" left the server.
    let renamed = calendar("/cal/alice/work/", "Office");
    let (recorded, live) = store
        .apply_collections(
            account,
            vec![renamed.clone(), book("/card/alice/default/")],
            T,
        )
        .await
        .unwrap();
    assert_eq!(live.len(), 2);
    let delta = since(&store, account, DataType::Calendar, cal_state).await;
    let ids: Vec<i64> = live.iter().map(|c| c.id.get()).collect();
    assert_eq!(delta.updated, [ids[0]]);
    assert_eq!(delta.destroyed.len(), 1);
    let home_id = delta.destroyed[0];
    assert!(recorded.state(DataType::AddressBook).is_none());

    // It came back: same id, synced from scratch.
    let all = store.dav_collections(account).await.unwrap();
    let hidden = all.iter().find(|c| c.id.get() == home_id).unwrap();
    assert!(hidden.orphaned_at.is_some() && !hidden.is_default);
    store
        .set_collection_synced(account, hidden.id, Some("t1".to_owned()), None, T)
        .await
        .unwrap();
    let before = store.state(account, DataType::Calendar).await.unwrap();
    let (_, live) = store
        .apply_collections(
            account,
            vec![renamed, home, book("/card/alice/default/")],
            T,
        )
        .await
        .unwrap();
    let back = live.iter().find(|c| c.id.get() == home_id).unwrap();
    assert_eq!(back.sync, CollectionSync::default());
    let delta = since(&store, account, DataType::Calendar, before).await;
    assert_eq!(delta.created, [home_id]);
}

#[tokio::test(flavor = "multi_thread")]
async fn resources_record_their_visibility() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let (_, live) = store
        .apply_collections(account, vec![calendar("/cal/a/work/", "Work")], T)
        .await
        .unwrap();
    let work = live[0].id;
    let t = DataType::CalendarEvent;

    let s1 = store
        .apply_resources(
            account,
            work,
            vec![
                event("/cal/a/work/1.ics", "u1", "\"e1\""),
                event("/cal/a/work/2.ics", "u2", "\"e1\""),
            ],
            vec![],
            T,
        )
        .await
        .unwrap()
        .state(t)
        .unwrap();
    let stored = store.dav_resources(account, work).await.unwrap();
    assert_eq!(stored.len(), 2);
    assert!(stored.iter().all(StoredResource::is_visible));
    let (one, two) = (stored[0].id, stored[1].id);
    assert_eq!(
        store
            .dav_resource_data(account, one)
            .await
            .unwrap()
            .unwrap(),
        b"sealed u1 \"e1\""
    );

    // 1 changes, 2 stops parsing: updated and destroyed.
    let broken = ResourceWrite {
        parsed: Parsed::Invalid,
        ..event("/cal/a/work/2.ics", "u2", "\"e2\"")
    };
    store
        .apply_resources(
            account,
            work,
            vec![event("/cal/a/work/1.ics", "u1", "\"e2\""), broken],
            vec![],
            T,
        )
        .await
        .unwrap();
    let delta = since(&store, account, t, s1).await;
    assert_eq!(delta.updated, [one.get()]);
    assert_eq!(delta.destroyed, [two.get()]);
    let stored = store.dav_resources(account, work).await.unwrap();
    assert_eq!(stored[1].parsed, Parsed::Invalid);
    assert_eq!(stored[1].etag.as_deref(), Some("\"e2\""));

    // Removing the invalid one records nothing; removing 1 destroys it.
    let s2 = store.state(account, t).await.unwrap();
    let recorded = store
        .apply_resources(account, work, vec![], vec![href("/cal/a/work/2.ics")], T)
        .await
        .unwrap();
    assert!(recorded.is_empty());
    store
        .apply_resources(account, work, vec![], vec![href("/cal/a/work/1.ics")], T)
        .await
        .unwrap();
    assert_eq!(since(&store, account, t, s2).await.destroyed, [one.get()]);

    // Another collection of another account can't be written.
    let bob = store
        .get_or_create_account(
            crate::NewAccount {
                login: "bob",
                email: "bob@example.com",
                wrapped_dek: b"k",
            },
            T,
        )
        .await
        .unwrap()
        .id;
    assert!(
        store
            .apply_resources(bob, work, vec![], vec![], T)
            .await
            .is_err()
    );
    assert!(store.dav_resource_data(bob, one).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_by_another_client_keeps_the_id() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let (_, live) = store
        .apply_collections(
            account,
            vec![
                calendar("/cal/a/work/", "Work"),
                calendar("/cal/a/home/", "Home"),
                book("/card/a/default/"),
            ],
            T,
        )
        .await
        .unwrap();
    let (work, home, contacts): (CollectionId, CollectionId, CollectionId) =
        (live[0].id, live[1].id, live[2].id);
    store
        .apply_resources(
            account,
            work,
            vec![event("/cal/a/work/x.ics", "same", "\"1\"")],
            vec![],
            T,
        )
        .await
        .unwrap();
    let id = store.dav_resources(account, work).await.unwrap()[0].id;
    let before = store.state(account, DataType::CalendarEvent).await.unwrap();
    store
        .apply_resources(account, work, vec![], vec![href("/cal/a/work/x.ics")], T)
        .await
        .unwrap();
    // A card with the same UID is not the same object.
    let card = ResourceWrite {
        href: href("/card/a/default/same.vcf"),
        ..event("/card/a/default/same.vcf", "same", "\"1\"")
    };
    store
        .apply_resources(account, contacts, vec![card], vec![], T)
        .await
        .unwrap();
    assert_ne!(
        store.dav_resources(account, contacts).await.unwrap()[0].id,
        id
    );
    store
        .apply_resources(
            account,
            home,
            vec![event("/cal/a/home/y.ics", "same", "\"2\"")],
            vec![],
            T,
        )
        .await
        .unwrap();
    let moved = store.dav_resources(account, home).await.unwrap();
    assert_eq!(moved[0].id, id);
    assert!(moved[0].is_visible());
    assert!(store.dav_resources(account, work).await.unwrap().is_empty());
    // Destroyed then created in the range: an update for the client.
    let delta = since(&store, account, DataType::CalendarEvent, before).await;
    assert_eq!(delta.updated, [id.get()]);
    assert!(delta.created.is_empty() && delta.destroyed.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_collections_and_resources_are_purged() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let (_, live) = store
        .apply_collections(
            account,
            vec![
                calendar("/cal/a/work/", "Work"),
                calendar("/cal/a/old/", "Old"),
            ],
            T,
        )
        .await
        .unwrap();
    let (work, old) = (live[0].id, live[1].id);
    store
        .apply_resources(
            account,
            work,
            vec![
                event("/cal/a/work/1.ics", "u1", "\"1\""),
                event("/cal/a/work/2.ics", "u2", "\"1\""),
            ],
            vec![],
            T,
        )
        .await
        .unwrap();
    store
        .apply_resources(
            account,
            old,
            vec![event("/cal/a/old/3.ics", "u3", "\"1\"")],
            vec![],
            T,
        )
        .await
        .unwrap();
    let before = store.state(account, DataType::CalendarEvent).await.unwrap();
    store
        .apply_resources(account, work, vec![], vec![href("/cal/a/work/1.ics")], T)
        .await
        .unwrap();
    // "old" leaves the server: its event is destroyed with it.
    store
        .apply_collections(account, vec![calendar("/cal/a/work/", "Work")], T)
        .await
        .unwrap();
    let delta = since(&store, account, DataType::CalendarEvent, before).await;
    assert_eq!(delta.destroyed.len(), 2);

    store
        .purge_dav_orphans(account, UnixTime::from_secs(T.secs() - 1))
        .await
        .unwrap();
    assert_eq!(store.dav_resources(account, work).await.unwrap().len(), 2);
    store
        .purge_dav_orphans(account, UnixTime::from_secs(T.secs() + 1))
        .await
        .unwrap();
    assert_eq!(store.dav_resources(account, work).await.unwrap().len(), 1);
    assert_eq!(store.dav_collections(account).await.unwrap().len(), 1);
    assert!(store.dav_resources(account, old).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_is_stored_and_replaced() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    assert_eq!(store.dav_account(account).await.unwrap(), None);
    let mut found = DavAccount {
        principal: href("/principals/alice/"),
        calendar_homes: vec![href("/cal/alice/")],
        addressbook_homes: vec![href("/card/alice/"), href("/card/shared/")],
        schedule_outbox: None,
        user_addresses: vec!["mailto:alice@example.com".to_owned()],
        auto_schedule: true,
        discovered_at: T,
    };
    store.put_dav_account(account, &found).await.unwrap();
    assert_eq!(
        store.dav_account(account).await.unwrap().as_ref(),
        Some(&found)
    );
    found.schedule_outbox = Some(href("/cal/alice/outbox/"));
    found.auto_schedule = false;
    store.put_dav_account(account, &found).await.unwrap();
    assert_eq!(store.dav_account(account).await.unwrap(), Some(found));
}

#[test]
fn hrefs_and_flags_are_checked() {
    assert!(DavHref::new("https://a.example/x/").is_ok());
    for bad in [
        "ftp://a.example/",
        "/relative",
        "https://a.example/a b",
        "https://a\n",
    ] {
        assert!(DavHref::new(bad).is_err(), "{bad}");
    }
    assert_eq!(format!("{:?}", href("/secret/")), "DavHref(..)");
    assert!(Rights::from_db(127).is_ok());
    assert!(Rights::from_db(128).is_err());
    assert!(Sharee::new("mailto:bob@example.com").is_ok());
    assert!(Sharee::new("https://a.example/pal/bob/").is_ok());
    for bad in ["bob@example.com", "urn:x:bob", "mailto:a b", "mailto:a\n"] {
        assert!(Sharee::new(bad).is_err(), "{bad}");
    }
    assert_eq!(
        format!("{:?}", Sharee::new("mailto:bob@example.com").unwrap()),
        "Sharee(..)"
    );
    assert!(Components::from_db(-1).is_err());
    let rights = Rights::READ.union(Rights::BIND);
    assert!(rights.contains(Rights::BIND) && !rights.contains(Rights::UNBIND));
}

fn card(path: &str, uid: &str, etag: &str) -> ResourceWrite {
    ResourceWrite {
        parsed: Parsed::Valid(ResourceIndex {
            uid: Some(uid.to_owned()),
            sort_name: Some("pérez ana".to_owned()),
            ..ResourceIndex::default()
        }),
        ..event(path, uid, etag)
    }
}

/// The bridge's own writes: a card read by id, moved to another
/// book (same id, one update), and the bridge-only flags of a book.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario, step by step")]
async fn own_moves_and_book_flags() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let (_, live) = store
        .apply_collections(
            account,
            vec![book("/card/alice/default/"), book("/card/alice/work/")],
            T,
        )
        .await
        .unwrap();
    let (first, second) = (live[0].id, live[1].id);
    assert!(live[0].is_default && !live[1].is_default);
    store
        .apply_resources(
            account,
            first,
            vec![card("/card/alice/default/a.vcf", "u1", "1")],
            Vec::new(),
            T,
        )
        .await
        .unwrap();
    let id = store.dav_resources(account, first).await.unwrap()[0].id;
    let found = store.dav_resource(account, id).await.unwrap().unwrap();
    assert_eq!(found.collection, first);
    assert!(found.is_visible());
    let at = store
        .dav_resource_at(account, first, href("/card/alice/default/a.vcf"))
        .await
        .unwrap();
    assert_eq!(at, Some(found));
    let before = store.state(account, DataType::ContactCard).await.unwrap();

    // Moved: same id, new collection and href, one update.
    let moved = card("/card/alice/work/a.vcf", "u1", "2");
    let recorded = store
        .move_dav_resource(account, id, second, moved, T)
        .await
        .unwrap();
    assert!(recorded.state(DataType::ContactCard).is_some());
    let after = store.dav_resource(account, id).await.unwrap().unwrap();
    assert_eq!(after.collection, second);
    assert_eq!(after.href, href("/card/alice/work/a.vcf"));
    assert_eq!(after.etag.as_deref(), Some("2"));
    let seen = since(&store, account, DataType::ContactCard, before).await;
    assert_eq!(seen.updated, [id.get()]);
    assert!(seen.created.is_empty() && seen.destroyed.is_empty());
    assert!(
        store
            .dav_resources(account, first)
            .await
            .unwrap()
            .is_empty()
    );

    // Not into a calendar, not over a visible card.
    let (_, live) = store
        .apply_collections(
            account,
            vec![
                book("/card/alice/default/"),
                book("/card/alice/work/"),
                calendar("/cal/alice/work/", "Work"),
            ],
            T,
        )
        .await
        .unwrap();
    let calendar_id = live
        .iter()
        .find(|c| c.props.kind == CollectionKind::Calendar)
        .unwrap()
        .id;
    let into_calendar = card("/cal/alice/work/a.vcf", "u1", "3");
    assert!(matches!(
        store
            .move_dav_resource(account, id, calendar_id, into_calendar, T)
            .await,
        Err(StoreError::Invalid(_))
    ));
    store
        .apply_resources(
            account,
            first,
            vec![card("/card/alice/default/b.vcf", "u2", "1")],
            Vec::new(),
            T,
        )
        .await
        .unwrap();
    let taken = card("/card/alice/default/b.vcf", "u1", "4");
    assert!(matches!(
        store.move_dav_resource(account, id, first, taken, T).await,
        Err(StoreError::Invalid(_))
    ));

    // Flags only the bridge keeps.
    let books_before = store.state(account, DataType::AddressBook).await.unwrap();
    let recorded = store
        .set_collection_subscribed(account, second, false)
        .await
        .unwrap();
    assert!(!recorded.is_empty());
    assert!(
        store
            .set_collection_subscribed(account, second, false)
            .await
            .unwrap()
            .is_empty()
    );
    store
        .set_default_collection(account, second, CollectionKind::AddressBook)
        .await
        .unwrap();
    // isVisible (calendars): recorded once, kept.
    assert!(
        !store
            .set_collection_visible(account, calendar_id, false)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .set_collection_visible(account, calendar_id, false)
            .await
            .unwrap()
            .is_empty()
    );
    let collections = store.dav_collections(account).await.unwrap();
    assert!(
        !collections
            .iter()
            .find(|c| c.id == calendar_id)
            .unwrap()
            .is_visible
    );
    let flags = |id: CollectionId| {
        let c = collections.iter().find(|c| c.id == id).unwrap();
        (c.is_default, c.is_subscribed)
    };
    assert_eq!(flags(first), (false, true));
    assert_eq!(flags(second), (true, false));
    // The calendar's default is another kind's: untouched.
    assert!(
        collections
            .iter()
            .find(|c| c.id == calendar_id)
            .unwrap()
            .is_default
    );
    let seen = since(&store, account, DataType::AddressBook, books_before).await;
    let mut updated = seen.updated.clone();
    updated.sort_unstable();
    assert_eq!(updated, [first.get(), second.get()]);
    // The next listing keeps the chosen default.
    let (_, live) = store
        .apply_collections(
            account,
            vec![book("/card/alice/default/"), book("/card/alice/work/")],
            T,
        )
        .await
        .unwrap();
    assert!(live.iter().find(|c| c.id == second).unwrap().is_default);
    // Another account's book and card are not found.
    let bob = store
        .get_or_create_account(
            crate::NewAccount {
                login: "bob",
                email: "bob@example.com",
                wrapped_dek: b"dek-b",
            },
            T,
        )
        .await
        .unwrap()
        .id;
    assert!(matches!(
        store
            .set_default_collection(bob, second, CollectionKind::AddressBook)
            .await,
        Err(StoreError::NotFound(_))
    ));
    assert!(store.dav_resource(bob, id).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn fired_alerts_are_recorded_once_and_purged() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let (_, live) = store
        .apply_collections(account, vec![calendar("/cal/a/work/", "Work")], T)
        .await
        .unwrap();
    let work = live[0].id;
    store
        .apply_resources(
            account,
            work,
            vec![event("/cal/a/work/1.ics", "u1", "\"1\"")],
            vec![],
            T,
        )
        .await
        .unwrap();
    let id = store.dav_resources(account, work).await.unwrap()[0].id;
    let fired = |alert: &str, occurrence: i64| FiredAlert {
        resource: id,
        alert_id: alert.to_owned(),
        occurrence: UnixTime::from_secs(occurrence),
        repetition: 0,
    };
    let at = UnixTime::from_secs;
    let record = async |alert: FiredAlert, when: i64| {
        store.record_fired_alert(account, alert, at(when)).await
    };
    assert!(record(fired("k1", 10), 100).await.unwrap());
    // Twice (a restart, or two tasks): only the first one counts.
    assert!(!record(fired("k1", 10), 200).await.unwrap());
    assert!(record(fired("k1", 20), 300).await.unwrap());
    assert!(record(fired("k2", 10), 300).await.unwrap());
    // Each repeat of an alarm is its own ring, also sent once.
    let again = FiredAlert {
        repetition: 2,
        ..fired("k2", 10)
    };
    assert!(record(again.clone(), 400).await.unwrap());
    assert!(!record(again.clone(), 500).await.unwrap());
    let all = store.fired_alerts(account, at(0)).await.unwrap();
    assert_eq!(
        all,
        [fired("k1", 10), fired("k1", 20), fired("k2", 10), again]
    );
    assert_eq!(store.fired_alerts(account, at(150)).await.unwrap().len(), 3);
    assert!(matches!(
        record(fired("", 1), 1).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        record(fired(&"a".repeat(256), 1), 1).await,
        Err(StoreError::Invalid(_))
    ));

    // Another account can't use alice's event.
    let bob = store
        .get_or_create_account(
            crate::NewAccount {
                login: "bob",
                email: "bob@example.com",
                wrapped_dek: b"k",
            },
            T,
        )
        .await
        .unwrap()
        .id;
    assert!(matches!(
        store.record_fired_alert(bob, fired("k9", 1), at(1)).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(store.fired_alerts(bob, at(0)).await.unwrap().is_empty());

    assert_eq!(store.purge_fired_alerts(account, at(300)).await.unwrap(), 1);
    assert_eq!(store.fired_alerts(account, at(0)).await.unwrap().len(), 3);
    // They go with their event.
    store
        .apply_resources(account, work, vec![], vec![href("/cal/a/work/1.ics")], T)
        .await
        .unwrap();
    store
        .purge_dav_orphans(account, UnixTime::from_secs(T.secs() + 1))
        .await
        .unwrap();
    assert!(store.fired_alerts(account, at(0)).await.unwrap().is_empty());
}

/// Owner, share mode and sharees are kept with the collection; a change of
/// any of them is a visible change (an update of the calendar).
#[tokio::test]
async fn sharing_is_stored_and_visible() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let mut shared = calendar("/cal/alice/work/", "Work");
    shared.sharing = CollectionSharing {
        owner: None,
        mode: Some(ShareMode::Acl),
        shares: vec![
            (
                Sharee::new("https://dav.example.com/pal/bob/").unwrap(),
                Rights::READ.union(Rights::READ_FREE_BUSY),
            ),
            (
                Sharee::new("mailto:carol@example.com").unwrap(),
                Rights::READ
                    .union(Rights::WRITE_CONTENT)
                    .union(Rights::WRITE_ACL),
            ),
        ],
    };
    let mut theirs = calendar("/cal/bob/home/", "Bob");
    theirs.sharing.owner = Some(href("/pal/bob/"));
    // Listed first, but someone else's: never the default.
    let (_, live) = store
        .apply_collections(account, vec![theirs.clone(), shared.clone()], T)
        .await
        .unwrap();
    assert_eq!(live.len(), 2);
    assert!(
        live.iter()
            .all(|c| c.is_default == c.props.sharing.owner.is_none())
    );
    let stored = store.dav_collections(account).await.unwrap();
    assert_eq!(stored[1].props.sharing, shared.sharing);
    assert_eq!(stored[0].props.sharing.owner, Some(href("/pal/bob/")));
    assert_eq!(stored[0].props.sharing.mode, None);
    // Unsharing is an update.
    shared.sharing.shares.clear();
    let (recorded, _) = store
        .apply_collections(account, vec![theirs, shared.clone()], T)
        .await
        .unwrap();
    assert!(recorded.state(DataType::Calendar).is_some());
    let stored = store.dav_collections(account).await.unwrap();
    assert!(stored[1].props.sharing.shares.is_empty());
}

fn many_shares(count: usize) -> Vec<(Sharee, Rights)> {
    (0..count)
        .map(|n| {
            (
                Sharee::new(&format!("https://dav.example.com/pal/u{n}/")).unwrap(),
                Rights::READ,
            )
        })
        .collect()
}

/// More than 256 sharees are stored and read back as they are.
#[tokio::test]
async fn shares_round_trip_past_256() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let mut shared = calendar("/cal/alice/work/", "Work");
    shared.sharing.mode = Some(ShareMode::Acl);
    shared.sharing.shares = many_shares(300);
    store
        .apply_collections(account, vec![shared.clone()], T)
        .await
        .unwrap();
    let stored = store.dav_collections(account).await.unwrap();
    assert_eq!(stored[0].props.sharing, shared.sharing);
}

/// Past the limit: refused when writing, corrupt when reading, never cut.
#[test]
fn shares_over_the_limit_are_refused_not_truncated() {
    let sharing = CollectionSharing {
        shares: many_shares(MAX_SHARES + 1),
        ..CollectionSharing::default()
    };
    assert!(matches!(
        sharing.shares_to_db(),
        Err(StoreError::Invalid("shares"))
    ));
    let ok = CollectionSharing {
        shares: many_shares(MAX_SHARES),
        ..CollectionSharing::default()
    };
    let text = ok.shares_to_db().unwrap().unwrap();
    assert_eq!(
        CollectionSharing::shares_from_db(Some(&text))
            .unwrap()
            .len(),
        MAX_SHARES
    );
    let one = ok
        .shares
        .first()
        .map(|(s, r)| (s.as_str(), r.to_db()))
        .unwrap();
    let list: Vec<(&str, i32)> = vec![one; MAX_SHARES + 1];
    let over = serde_json::to_string(&list).unwrap();
    assert!(matches!(
        CollectionSharing::shares_from_db(Some(&over)),
        Err(StoreError::Corrupt("shares"))
    ));
}

/// Someone else's collection can't be made the default (JMAP
/// `onSuccessSetIsDefault` on a shared calendar): the default stays.
#[tokio::test]
async fn someone_elses_collection_is_never_default() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let mut theirs = calendar("/cal/bob/home/", "Bob");
    theirs.sharing.owner = Some(href("/pal/bob/"));
    let (_, live) = store
        .apply_collections(
            account,
            vec![calendar("/cal/alice/work/", "Work"), theirs],
            T,
        )
        .await
        .unwrap();
    let own = live
        .iter()
        .find(|c| c.props.sharing.owner.is_none())
        .unwrap();
    let bobs = live
        .iter()
        .find(|c| c.props.sharing.owner.is_some())
        .unwrap();
    assert!(own.is_default);
    assert!(matches!(
        store
            .set_default_collection(account, bobs.id, CollectionKind::Calendar)
            .await,
        Err(StoreError::NotFound(_))
    ));
    let stored = store.dav_collections(account).await.unwrap();
    assert!(stored.iter().find(|c| c.id == own.id).unwrap().is_default);
    assert!(!stored.iter().find(|c| c.id == bobs.id).unwrap().is_default);
}

/// The two sealed link columns of a collection.
type LinkColumns = (Option<Vec<u8>>, Option<Vec<u8>>);

/// Nextcloud's extras are kept, compared (a new public link is a JMAP
/// update) and read back; the links only as the sealed bytes given.
#[tokio::test]
async fn calendar_extras_round_trip() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let sealed = |bytes: &[u8]| SealedLink::new(bytes.to_vec()).unwrap();
    let mut work = calendar("/cal/alice/work/", "Work");
    work.extras = CalendarExtras {
        publishable: true,
        publish_url: None,
        is_subscription: false,
        subscription_source: None,
    };
    let mut holidays = calendar("/cal/alice/sub1/", "Holidays");
    holidays.extras = CalendarExtras {
        publishable: false,
        publish_url: None,
        is_subscription: true,
        subscription_source: Some(sealed(b"sealed source")),
    };
    let (_, live) = store
        .apply_collections(account, vec![work.clone(), holidays.clone()], T)
        .await
        .unwrap();
    assert_eq!(live[0].props.extras, work.extras);
    assert_eq!(live[1].props.extras, holidays.extras);
    let before = since(&store, account, DataType::Calendar, Seq::ZERO).await;
    work.extras.publish_url = Some(sealed(b"sealed link"));
    let (_, live) = store
        .apply_collections(account, vec![work.clone(), holidays.clone()], T)
        .await
        .unwrap();
    assert_eq!(live[0].props.extras, work.extras);
    let after = since(&store, account, DataType::Calendar, before.new_state).await;
    assert_eq!(after.updated.len(), 1);
    // The same bytes again: no update.
    store
        .apply_collections(account, vec![work, holidays], T)
        .await
        .unwrap();
    let again = since(&store, account, DataType::Calendar, after.new_state).await;
    assert!(again.updated.is_empty());
    // Only bytes are stored: no column holds a link in clear.
    let stored: Vec<LinkColumns> = store
        .read(move |conn| {
            use crate::schema::dav_collections as c;
            use diesel::{QueryDsl as _, RunQueryDsl as _};
            Ok(c::table
                .select((c::publish_url_sealed, c::subscription_source_sealed))
                .load(conn)?)
        })
        .await
        .unwrap();
    assert_eq!(
        stored,
        vec![
            (Some(b"sealed link".to_vec()), None),
            (None, Some(b"sealed source".to_vec()))
        ]
    );
    assert!(SealedLink::new(Vec::new()).is_err());
    assert!(SealedLink::new(vec![0; SealedLink::MAX_LEN + 1]).is_err());
    assert_eq!(format!("{:?}", sealed(b"x")), "SealedLink(..)");
    for bad in ["ftp://x/", "https://a b/", "javascript:alert(1)"] {
        assert!(ExternalUrl::new(bad).is_err(), "{bad}");
    }
    assert_eq!(
        format!("{:?}", ExternalUrl::new("https://x/secret").unwrap()),
        "ExternalUrl(..)"
    );
}

/// A collection the user may no longer read (free/busy only) is hidden as
/// if it had left the server, and comes back synced from scratch.
#[tokio::test(flavor = "multi_thread")]
async fn collections_without_read_are_hidden() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let mut theirs = calendar("/cal/bob/home/", "Bob");
    theirs.sharing.owner = Some(href("/pal/bob/"));
    theirs.rights = Some(Rights::READ);
    let (_, live) = store
        .apply_collections(account, vec![theirs.clone()], T)
        .await
        .unwrap();
    let id = live[0].id;
    store
        .apply_resources(
            account,
            id,
            vec![
                event("/cal/bob/home/a.ics", "a", "1"),
                event("/cal/bob/home/b.ics", "b", "1"),
            ],
            Vec::new(),
            T,
        )
        .await
        .unwrap();
    store
        .set_collection_synced(account, id, Some("token".to_owned()), None, T)
        .await
        .unwrap();
    let resources: Vec<_> = store
        .dav_resources(account, id)
        .await
        .unwrap()
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(resources.len(), 2);
    let calendars = store.state(account, DataType::Calendar).await.unwrap();
    let events = store.state(account, DataType::CalendarEvent).await.unwrap();

    let mut free_busy = theirs.clone();
    free_busy.rights = Some(Rights::READ_FREE_BUSY);
    let (_, live) = store
        .apply_collections(account, vec![free_busy.clone()], T)
        .await
        .unwrap();
    assert!(live.is_empty());
    for &resource in &resources {
        let found = store
            .visible_resource(account, CollectionKind::Calendar, resource)
            .await
            .unwrap();
        assert!(found.is_none());
    }
    let seen = since(&store, account, DataType::Calendar, calendars).await;
    assert_eq!(seen.destroyed, [id.get()]);
    let mut gone = since(&store, account, DataType::CalendarEvent, events)
        .await
        .destroyed;
    gone.sort_unstable();
    let ids: Vec<i64> = resources.iter().map(|r| r.get()).collect();
    assert_eq!(gone, ids);

    // Read again: back, and synced from scratch.
    let (_, live) = store
        .apply_collections(account, vec![theirs], T)
        .await
        .unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, id);
    assert_eq!(live[0].sync.sync_token, None);

    // A new collection without read is never created; rights not said are
    // allowed.
    let mut other = calendar("/cal/carol/home/", "Carol");
    other.rights = Some(Rights::READ_FREE_BUSY);
    let mut unsaid = calendar("/cal/alice/home/", "Home");
    unsaid.rights = None;
    let (_, live) = store
        .apply_collections(account, vec![free_busy, other, unsaid.clone()], T)
        .await
        .unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].props.href, unsaid.href);
    assert_eq!(store.dav_collections(account).await.unwrap().len(), 2);
}

/// The default is always one of the user's own writable collections of
/// the same kind, also when an older database marked a shared one.
#[tokio::test(flavor = "multi_thread")]
async fn defaults_are_own_writable_and_of_the_kind() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let mut theirs = calendar("/cal/bob/home/", "Bob");
    theirs.sharing.owner = Some(href("/pal/bob/"));
    let mut read_only = calendar("/cal/alice/birthdays/", "Birthdays");
    read_only.rights = Some(Rights::READ);
    let mut subscription = calendar("/cal/alice/sub1/", "Holidays");
    subscription.extras.is_subscription = true;
    let own = calendar("/cal/alice/home/", "Home");
    let listed = vec![
        theirs.clone(),
        read_only.clone(),
        subscription.clone(),
        own.clone(),
        book("/card/alice/default/"),
    ];
    let (_, live) = store
        .apply_collections(account, listed.clone(), T)
        .await
        .unwrap();
    let id_of = |path: &str| live.iter().find(|c| c.props.href == href(path)).unwrap().id;
    let (shared, birthdays, holidays, home, contacts) = (
        id_of("/cal/bob/home/"),
        id_of("/cal/alice/birthdays/"),
        id_of("/cal/alice/sub1/"),
        id_of("/cal/alice/home/"),
        id_of("/card/alice/default/"),
    );
    let defaults = |collections: &[Collection]| -> Vec<CollectionId> {
        collections
            .iter()
            .filter(|c| c.is_default)
            .map(|c| c.id)
            .collect()
    };
    assert_eq!(defaults(&live), [home, contacts]);

    for (id, kind) in [
        (shared, CollectionKind::Calendar),
        (contacts, CollectionKind::Calendar),
        (home, CollectionKind::AddressBook),
    ] {
        assert!(matches!(
            store.set_default_collection(account, id, kind).await,
            Err(StoreError::NotFound(_))
        ));
    }
    for id in [birthdays, holidays] {
        assert!(matches!(
            store
                .set_default_collection(account, id, CollectionKind::Calendar)
                .await,
            Err(StoreError::Invalid(_))
        ));
    }
    let stored = store.dav_collections(account).await.unwrap();
    assert_eq!(defaults(&stored), [home, contacts]);

    // An older version could mark a shared calendar: the next listing
    // clears it and reports the update.
    store
        .write(move |conn| {
            use diesel::{ExpressionMethods as _, QueryDsl as _, RunQueryDsl as _};
            diesel::update(crate::schema::dav_collections::table.find(shared.get()))
                .set(crate::schema::dav_collections::is_default.eq(true))
                .execute(conn)?;
            Ok(())
        })
        .await
        .unwrap();
    let before = store.state(account, DataType::Calendar).await.unwrap();
    let (_, live) = store.apply_collections(account, listed, T).await.unwrap();
    assert_eq!(defaults(&live), [home, contacts]);
    let seen = since(&store, account, DataType::Calendar, before).await;
    assert_eq!(seen.updated, [shared.get()]);
}
