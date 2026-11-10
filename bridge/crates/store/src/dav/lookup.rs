// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What the JMAP methods read: the visible resources of the
//! account, filtered with the clear-text indexes, and their sealed text.
//! Visible = on the server, parsed, and in a collection that is on the
//! server too.

use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, JoinOnDsl as _, OptionalExtension as _,
    QueryDsl as _, RunQueryDsl as _, SelectableHelper as _,
};

use super::resource::ResourceRow;
use super::types::{CollectionKind, StoredResource};
use crate::schema::{dav_collections, dav_resources};
use crate::{AccountId, CollectionId, ResourceId, Store, StoreError, UnixTime};

/// Which visible resources to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceQuery {
    /// Calendars or address books.
    pub kind: CollectionKind,
    /// Only in this collection.
    pub collection: Option<CollectionId>,
    /// Only with exactly this `UID`.
    pub uid: Option<String>,
    /// Only those whose indexed range may overlap `[after, before)`: that
    /// start before `before` and end after `after` (or have no end: open
    /// rule). The index reads floating times as UTC, so widen the range
    /// by 14 h. Resources without any start are left out.
    pub overlapping: Option<(UnixTime, UnixTime)>,
}

impl ResourceQuery {
    /// Every visible resource of this kind.
    #[must_use]
    pub const fn all(kind: CollectionKind) -> Self {
        Self {
            kind,
            collection: None,
            uid: None,
            overlapping: None,
        }
    }
}

impl Store {
    /// The visible resources of the account that match `query`, by id.
    ///
    /// # Errors
    /// On database errors.
    pub async fn visible_resources(
        &self,
        account: AccountId,
        query: ResourceQuery,
    ) -> Result<Vec<StoredResource>, StoreError> {
        self.read(move |conn| {
            let mut select = dav_resources::table
                .inner_join(
                    dav_collections::table.on(dav_collections::id.eq(dav_resources::collection_id)),
                )
                .filter(dav_resources::account_id.eq(account.get()))
                .filter(dav_resources::orphaned_at.is_null())
                .filter(dav_resources::invalid.eq(false))
                .filter(dav_collections::orphaned_at.is_null())
                .filter(dav_collections::kind.eq(query.kind.code()))
                .into_boxed();
            if let Some(collection) = query.collection {
                select = select.filter(dav_resources::collection_id.eq(collection.get()));
            }
            if let Some(uid) = query.uid {
                select = select.filter(dav_resources::uid.eq(uid));
            }
            if let Some((after, before)) = query.overlapping {
                select = select
                    .filter(dav_resources::utc_start.lt(before.secs()))
                    .filter(
                        dav_resources::utc_end
                            .is_null()
                            .or(dav_resources::utc_end.gt(after.secs())),
                    );
            }
            select
                .order(dav_resources::id.asc())
                .select(ResourceRow::as_select())
                .load(conn)?
                .into_iter()
                .map(StoredResource::try_from)
                .collect()
        })
        .await
    }

    /// One visible resource of the account of this kind, if it is one.
    ///
    /// # Errors
    /// On database errors.
    pub async fn visible_resource(
        &self,
        account: AccountId,
        kind: CollectionKind,
        resource: ResourceId,
    ) -> Result<Option<StoredResource>, StoreError> {
        self.read(move |conn| {
            dav_resources::table
                .inner_join(
                    dav_collections::table.on(dav_collections::id.eq(dav_resources::collection_id)),
                )
                .filter(dav_resources::id.eq(resource.get()))
                .filter(dav_resources::account_id.eq(account.get()))
                .filter(dav_resources::orphaned_at.is_null())
                .filter(dav_resources::invalid.eq(false))
                .filter(dav_collections::orphaned_at.is_null())
                .filter(dav_collections::kind.eq(kind.code()))
                .select(ResourceRow::as_select())
                .first(conn)
                .optional()?
                .map(StoredResource::try_from)
                .transpose()
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::tests::alice;
    use crate::db::test_support::store;
    use crate::{
        CollectionProps, CollectionSharing, Components, DavHref, Parsed, ResourceIndex,
        ResourceWrite, Rights,
    };

    fn href(path: &str) -> DavHref {
        DavHref::new(&format!("https://dav.example.com{path}")).unwrap()
    }

    fn calendar(path: &str) -> CollectionProps {
        CollectionProps {
            kind: CollectionKind::Calendar,
            href: href(path),
            name: Some("Work".to_owned()),
            color: None,
            description: None,
            sort_order: None,
            time_zone: None,
            default_alarms_datetime: None,
            default_alarms_date: None,
            rights: Some(Rights::READ),
            components: Some(Components::EVENT),
            vcard_versions: None,
            supports_sync: true,
            sharing: CollectionSharing::default(),
            extras: crate::CalendarExtras::default(),
        }
    }

    fn event(path: &str, uid: &str, range: (i64, Option<i64>)) -> ResourceWrite {
        ResourceWrite {
            href: href(path),
            etag: Some("e".to_owned()),
            sealed: b"sealed".to_vec(),
            size: 6,
            parsed: Parsed::Valid(ResourceIndex {
                uid: Some(uid.to_owned()),
                utc_start: Some(UnixTime::from_secs(range.0)),
                utc_end: range.1.map(UnixTime::from_secs),
                recurring: range.1.is_none(),
                sort_name: None,
            }),
        }
    }

    fn uids(list: Vec<StoredResource>) -> Vec<String> {
        list.into_iter()
            .map(|r| match r.parsed {
                Parsed::Valid(index) => index.uid.unwrap(),
                Parsed::Invalid => "invalid".to_owned(),
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn visible_resources_by_index() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let now = UnixTime::from_secs(5000);
        let (_, live) = store
            .apply_collections(account, vec![calendar("/a/"), calendar("/b/")], now)
            .await
            .unwrap();
        let (a, b) = (live[0].id, live[1].id);
        let writes = vec![
            event("/a/1.ics", "one", (100, Some(200))),
            event("/a/2.ics", "two", (300, None)),
            ResourceWrite {
                parsed: Parsed::Invalid,
                ..event("/a/3.ics", "bad", (100, Some(200)))
            },
        ];
        store
            .apply_resources(account, a, writes, Vec::new(), now)
            .await
            .unwrap();
        let other = vec![event("/b/1.ics", "one", (1000, Some(2000)))];
        store
            .apply_resources(account, b, other, Vec::new(), now)
            .await
            .unwrap();
        let all = ResourceQuery::all(CollectionKind::Calendar);
        let found = store.visible_resources(account, all.clone()).await.unwrap();
        assert_eq!(uids(found.clone()), ["one", "two", "one"]);
        let in_b = ResourceQuery {
            collection: Some(b),
            ..all.clone()
        };
        assert_eq!(
            store.visible_resources(account, in_b).await.unwrap().len(),
            1
        );
        let by_uid = ResourceQuery {
            uid: Some("two".to_owned()),
            ..all.clone()
        };
        assert_eq!(
            uids(store.visible_resources(account, by_uid).await.unwrap()),
            ["two"]
        );
        // [150, 350): the first one and the open-ended one.
        let window = ResourceQuery {
            overlapping: Some((UnixTime::from_secs(150), UnixTime::from_secs(350))),
            ..all.clone()
        };
        assert_eq!(
            uids(store.visible_resources(account, window).await.unwrap()),
            ["one", "two"]
        );
        let books = ResourceQuery::all(CollectionKind::AddressBook);
        assert!(
            store
                .visible_resources(account, books)
                .await
                .unwrap()
                .is_empty()
        );

        let first = found[0].id;
        let get = |kind| store.visible_resource(account, kind, first);
        assert!(get(CollectionKind::Calendar).await.unwrap().is_some());
        assert!(get(CollectionKind::AddressBook).await.unwrap().is_none());
        // A collection that left the server hides its resources.
        store
            .apply_collections(account, vec![calendar("/b/")], now)
            .await
            .unwrap();
        assert!(get(CollectionKind::Calendar).await.unwrap().is_none());
    }
}
