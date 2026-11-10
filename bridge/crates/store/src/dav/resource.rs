// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! iCalendar objects and vCards of a collection.
//!
//! A resource is visible over JMAP while it is on the server and parsed.
//! Writes compare the old and new visibility in the same transaction:
//! invisible → visible is `created`, visible → visible `updated`, visible
//! → invisible `destroyed`. A resource that leaves the server is hidden
//! for a while before it is deleted; if a resource with the same `UID`
//! appears meanwhile in a collection of the same kind (another client
//! moved it), it keeps its id, so the client sees it change calendars
//! instead of a delete and a new event.

use diesel::{
    ExpressionMethods as _, JoinOnDsl as _, OptionalExtension as _, QueryDsl as _, Queryable,
    RunQueryDsl as _, Selectable, SelectableHelper as _, SqliteConnection,
};

use super::collection::{Pending, collection_id, load_collection};
use super::types::{CollectionKind, DavHref, Parsed, ResourceIndex, ResourceWrite, StoredResource};
use crate::mail::Recorded;
use crate::schema::{dav_collections, dav_resources};
use crate::{AccountId, ChangeKind, CollectionId, ResourceId, Store, StoreError, UnixTime};

#[derive(Queryable, Selectable)]
#[diesel(table_name = dav_resources, check_for_backend(diesel::sqlite::Sqlite))]
pub(super) struct ResourceRow {
    id: i64,
    collection_id: i64,
    href: String,
    etag: Option<String>,
    uid: Option<String>,
    size: i64,
    utc_start: Option<i64>,
    utc_end: Option<i64>,
    recurring: bool,
    sort_name: Option<String>,
    invalid: bool,
    updated_at: i64,
    orphaned_at: Option<i64>,
}

impl TryFrom<ResourceRow> for StoredResource {
    type Error = StoreError;

    fn try_from(row: ResourceRow) -> Result<Self, StoreError> {
        let parsed = if row.invalid {
            Parsed::Invalid
        } else {
            Parsed::Valid(ResourceIndex {
                uid: row.uid,
                utc_start: row.utc_start.map(UnixTime::from_secs),
                utc_end: row.utc_end.map(UnixTime::from_secs),
                recurring: row.recurring,
                sort_name: row.sort_name,
            })
        };
        Ok(Self {
            id: ResourceId::from_raw(row.id).ok_or(StoreError::Corrupt("resource id"))?,
            collection: collection_id(row.collection_id)?,
            href: DavHref::from_db(row.href)?,
            etag: row.etag,
            size: u64::try_from(row.size).map_err(|_| StoreError::Corrupt("resource size"))?,
            parsed,
            updated_at: UnixTime::from_secs(row.updated_at),
            orphaned_at: row.orphaned_at.map(UnixTime::from_secs),
        })
    }
}

/// The current row at `(collection, href)`: id, visible.
pub(super) fn find(
    conn: &mut SqliteConnection,
    collection: i64,
    href: &DavHref,
) -> Result<Option<(i64, bool)>, StoreError> {
    let row: Option<(i64, Option<i64>, bool)> = dav_resources::table
        .filter(dav_resources::collection_id.eq(collection))
        .filter(dav_resources::href.eq(href.as_str()))
        .select((
            dav_resources::id,
            dav_resources::orphaned_at,
            dav_resources::invalid,
        ))
        .first(conn)
        .optional()?;
    Ok(row.map(|(id, orphaned, invalid)| (id, orphaned.is_none() && !invalid)))
}

/// A hidden resource of the account with this `UID`, in a collection of
/// the same kind: the one that another client just moved.
fn moved_away(
    conn: &mut SqliteConnection,
    account: AccountId,
    kind: CollectionKind,
    uid: &str,
) -> Result<Option<i64>, StoreError> {
    Ok(dav_resources::table
        .inner_join(dav_collections::table.on(dav_collections::id.eq(dav_resources::collection_id)))
        .filter(dav_resources::account_id.eq(account.get()))
        .filter(dav_resources::uid.eq(uid))
        .filter(dav_resources::orphaned_at.is_not_null())
        .filter(dav_resources::invalid.eq(false))
        .filter(dav_collections::kind.eq(kind.code()))
        .order(dav_resources::orphaned_at.desc())
        .select(dav_resources::id)
        .first(conn)
        .optional()?)
}

fn size_to_db(size: u64) -> Result<i64, StoreError> {
    i64::try_from(size).map_err(|_| StoreError::Invalid("resource size"))
}

/// Writes every column of `write` into row `id` (now on the server).
pub(super) fn overwrite(
    conn: &mut SqliteConnection,
    id: i64,
    collection: i64,
    write: &ResourceWrite,
    now: UnixTime,
) -> Result<(), StoreError> {
    let index = match &write.parsed {
        Parsed::Valid(index) => index.clone(),
        Parsed::Invalid => ResourceIndex::default(),
    };
    diesel::update(dav_resources::table.find(id))
        .set((
            dav_resources::collection_id.eq(collection),
            dav_resources::href.eq(write.href.as_str()),
            dav_resources::etag.eq(&write.etag),
            dav_resources::uid.eq(&index.uid),
            dav_resources::data.eq(&write.sealed),
            dav_resources::size.eq(size_to_db(write.size)?),
            dav_resources::utc_start.eq(index.utc_start.map(UnixTime::secs)),
            dav_resources::utc_end.eq(index.utc_end.map(UnixTime::secs)),
            dav_resources::recurring.eq(index.recurring),
            dav_resources::sort_name.eq(&index.sort_name),
            dav_resources::invalid.eq(!write.parsed.is_valid()),
            dav_resources::updated_at.eq(now.secs()),
            dav_resources::orphaned_at.eq(None::<i64>),
        ))
        .execute(conn)?;
    Ok(())
}

fn insert(
    conn: &mut SqliteConnection,
    account: AccountId,
    collection: i64,
    write: &ResourceWrite,
    now: UnixTime,
) -> Result<i64, StoreError> {
    let id: i64 = diesel::insert_into(dav_resources::table)
        .values((
            dav_resources::account_id.eq(account.get()),
            dav_resources::collection_id.eq(collection),
            dav_resources::href.eq(write.href.as_str()),
            dav_resources::data.eq(&write.sealed),
            dav_resources::size.eq(size_to_db(write.size)?),
            dav_resources::recurring.eq(false),
            dav_resources::invalid.eq(true),
            dav_resources::updated_at.eq(now.secs()),
        ))
        .returning(dav_resources::id)
        .get_result(conn)?;
    overwrite(conn, id, collection, write, now)?;
    Ok(id)
}

pub(super) fn change(was_visible: bool, is_visible: bool) -> Option<ChangeKind> {
    match (was_visible, is_visible) {
        (false, true) => Some(ChangeKind::Created),
        (true, true) => Some(ChangeKind::Updated),
        (true, false) => Some(ChangeKind::Destroyed),
        (false, false) => None,
    }
}

fn apply(
    conn: &mut SqliteConnection,
    account: AccountId,
    collection: CollectionId,
    writes: &[ResourceWrite],
    removed: &[DavHref],
    now: UnixTime,
) -> Result<Recorded, StoreError> {
    let kind = load_collection(conn, account, collection)?.props.kind;
    let data_type = kind.resource_data_type();
    let collection = collection.get();
    let mut pending = Pending::default();
    for write in writes {
        let visible = write.parsed.is_valid();
        let found = match find(conn, collection, &write.href)? {
            Some(found) => Some(found),
            None => match write.parsed.uid() {
                Some(uid) => moved_away(conn, account, kind, uid)?.map(|id| (id, false)),
                None => None,
            },
        };
        let (id, was_visible) = if let Some((id, was_visible)) = found {
            overwrite(conn, id, collection, write, now)?;
            (id, was_visible)
        } else {
            (insert(conn, account, collection, write, now)?, false)
        };
        if let Some(kind) = change(was_visible, visible) {
            pending.push(data_type, id, kind);
        }
    }
    for href in removed {
        let Some((id, was_visible)) = find(conn, collection, href)? else {
            continue;
        };
        diesel::update(
            dav_resources::table
                .find(id)
                .filter(dav_resources::orphaned_at.is_null()),
        )
        .set(dav_resources::orphaned_at.eq(now.secs()))
        .execute(conn)?;
        if was_visible {
            pending.push(data_type, id, ChangeKind::Destroyed);
        }
    }
    pending.record(conn, account)
}

impl Store {
    /// Every resource of a collection (also hidden ones), without data.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account; on
    /// database errors.
    pub async fn dav_resources(
        &self,
        account: AccountId,
        collection: CollectionId,
    ) -> Result<Vec<StoredResource>, StoreError> {
        self.read(move |conn| {
            load_collection(conn, account, collection)?;
            dav_resources::table
                .filter(dav_resources::collection_id.eq(collection.get()))
                .order(dav_resources::id.asc())
                .select(ResourceRow::as_select())
                .load(conn)?
                .into_iter()
                .map(StoredResource::try_from)
                .collect()
        })
        .await
    }

    /// The sealed text of a resource of the account, if it exists.
    ///
    /// # Errors
    /// On database errors.
    pub async fn dav_resource_data(
        &self,
        account: AccountId,
        resource: ResourceId,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        self.read(move |conn| {
            Ok(dav_resources::table
                .find(resource.get())
                .filter(dav_resources::account_id.eq(account.get()))
                .select(dav_resources::data)
                .first(conn)
                .optional()?)
        })
        .await
    }

    /// Replaces the etags of resources whose text did not change (the
    /// server gave a new etag for the same bytes): nothing visible, so
    /// nothing is recorded.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account; on
    /// database errors.
    pub async fn set_resource_etags(
        &self,
        account: AccountId,
        collection: CollectionId,
        etags: Vec<(DavHref, Option<String>)>,
    ) -> Result<(), StoreError> {
        self.write(move |conn| {
            load_collection(conn, account, collection)?;
            for (href, etag) in etags {
                diesel::update(
                    dav_resources::table
                        .filter(dav_resources::collection_id.eq(collection.get()))
                        .filter(dav_resources::href.eq(href.as_str())),
                )
                .set(dav_resources::etag.eq(etag))
                .execute(conn)?;
            }
            Ok(())
        })
        .await
    }

    /// Applies fetched resources and removed hrefs of a collection in one
    /// transaction, recording the changes (`CalendarEvent` or
    /// `ContactCard`).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account; on
    /// database errors.
    pub async fn apply_resources(
        &self,
        account: AccountId,
        collection: CollectionId,
        writes: Vec<ResourceWrite>,
        removed: Vec<DavHref>,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| apply(conn, account, collection, &writes, &removed, now))
            .await
    }
}
