// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What the JMAP layer reads and writes itself:
//! one resource by id, a resource the bridge moved to another collection,
//! and the collection properties only the bridge keeps (`isSubscribed`,
//! `isDefault`). Each write records its JMAP changes in the same
//! transaction.

use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _,
    SelectableHelper as _, SqliteConnection,
};

use super::collection::{Pending, load_collection};
use super::resource::{ResourceRow, change, find, overwrite};
use super::types::{CollectionKind, DavHref, ResourceWrite, StoredResource};
use crate::mail::Recorded;
use crate::schema::{dav_collections, dav_resources};
use crate::{AccountId, ChangeKind, CollectionId, ResourceId, Store, StoreError, UnixTime};

fn load_resource(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: ResourceId,
) -> Result<Option<StoredResource>, StoreError> {
    dav_resources::table
        .find(id.get())
        .filter(dav_resources::account_id.eq(account.get()))
        .select(ResourceRow::as_select())
        .first(conn)
        .optional()?
        .map(StoredResource::try_from)
        .transpose()
}

fn move_resource(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: ResourceId,
    to: CollectionId,
    write: &ResourceWrite,
    now: UnixTime,
) -> Result<Recorded, StoreError> {
    let current = load_resource(conn, account, id)?.ok_or(StoreError::NotFound("resource"))?;
    let from = load_collection(conn, account, current.collection)?;
    let target = load_collection(conn, account, to)?;
    if from.props.kind != target.props.kind {
        return Err(StoreError::Invalid("collection kind"));
    }
    // A hidden row already at the destination (gone from the server a
    // moment ago) gives way: the unique index allows one row per href.
    if let Some((other, visible)) = find(conn, to.get(), &write.href)?
        && other != id.get()
    {
        if visible {
            return Err(StoreError::Invalid("destination taken"));
        }
        diesel::delete(dav_resources::table.find(other)).execute(conn)?;
    }
    overwrite(conn, id.get(), to.get(), write, now)?;
    let mut pending = Pending::default();
    if let Some(kind) = change(current.is_visible(), write.parsed.is_valid()) {
        pending.push(target.props.kind.resource_data_type(), id.get(), kind);
    }
    pending.record(conn, account)
}

fn set_subscribed(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: CollectionId,
    subscribed: bool,
) -> Result<Recorded, StoreError> {
    let collection = load_collection(conn, account, id)?;
    let mut pending = Pending::default();
    if collection.is_subscribed != subscribed {
        diesel::update(dav_collections::table.find(id.get()))
            .set(dav_collections::is_subscribed.eq(subscribed))
            .execute(conn)?;
        pending.push(
            collection.props.kind.data_type(),
            id.get(),
            ChangeKind::Updated,
        );
    }
    pending.record(conn, account)
}

fn set_visible(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: CollectionId,
    visible: bool,
) -> Result<Recorded, StoreError> {
    let collection = load_collection(conn, account, id)?;
    let mut pending = Pending::default();
    if collection.is_visible != visible {
        diesel::update(dav_collections::table.find(id.get()))
            .set(dav_collections::is_visible.eq(visible))
            .execute(conn)?;
        pending.push(
            collection.props.kind.data_type(),
            id.get(),
            ChangeKind::Updated,
        );
    }
    pending.record(conn, account)
}

fn set_default(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: CollectionId,
    kind: CollectionKind,
) -> Result<Recorded, StoreError> {
    let collection = load_collection(conn, account, id)?;
    // Another kind's id, one gone from the server or someone else's: as
    // if it didn't exist.
    if collection.orphaned_at.is_some()
        || collection.props.kind != kind
        || collection.props.sharing.owner.is_some()
    {
        return Err(StoreError::NotFound("collection"));
    }
    if !collection.can_be_default() {
        return Err(StoreError::Invalid("default collection"));
    }
    let mut pending = Pending::default();
    if !collection.is_default {
        let previous: Vec<i64> = dav_collections::table
            .filter(dav_collections::account_id.eq(account.get()))
            .filter(dav_collections::kind.eq(kind.code()))
            .filter(dav_collections::is_default.eq(true))
            .select(dav_collections::id)
            .load(conn)?;
        for other in previous {
            diesel::update(dav_collections::table.find(other))
                .set(dav_collections::is_default.eq(false))
                .execute(conn)?;
            pending.push(kind.data_type(), other, ChangeKind::Updated);
        }
        diesel::update(dav_collections::table.find(id.get()))
            .set(dav_collections::is_default.eq(true))
            .execute(conn)?;
        pending.push(kind.data_type(), id.get(), ChangeKind::Updated);
    }
    pending.record(conn, account)
}

impl Store {
    /// One resource of the account (also hidden ones), without data.
    ///
    /// # Errors
    /// On database errors.
    pub async fn dav_resource(
        &self,
        account: AccountId,
        id: ResourceId,
    ) -> Result<Option<StoredResource>, StoreError> {
        self.read(move |conn| load_resource(conn, account, id))
            .await
    }

    /// The resource at `href` in a collection of the account (also a
    /// hidden one), without data.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account; on
    /// database errors.
    pub async fn dav_resource_at(
        &self,
        account: AccountId,
        collection: CollectionId,
        href: DavHref,
    ) -> Result<Option<StoredResource>, StoreError> {
        self.read(move |conn| {
            load_collection(conn, account, collection)?;
            dav_resources::table
                .filter(dav_resources::collection_id.eq(collection.get()))
                .filter(dav_resources::href.eq(href.as_str()))
                .select(ResourceRow::as_select())
                .first(conn)
                .optional()?
                .map(StoredResource::try_from)
                .transpose()
        })
        .await
    }

    /// A resource the bridge moved to another collection of the same kind
    /// (DAV `MOVE`): it keeps its id, and `write` is what it is now (new
    /// href, etag and text sealed for its new place). Records an update.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a resource or collection of another
    /// account; [`StoreError::Invalid`] if the kinds differ or a visible
    /// resource is already at the new href; on database errors.
    pub async fn move_dav_resource(
        &self,
        account: AccountId,
        id: ResourceId,
        to: CollectionId,
        write: ResourceWrite,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| move_resource(conn, account, id, to, &write, now))
            .await
    }

    /// JMAP `isSubscribed` of a collection (only the bridge keeps it).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account; on
    /// database errors.
    pub async fn set_collection_subscribed(
        &self,
        account: AccountId,
        id: CollectionId,
        subscribed: bool,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| set_subscribed(conn, account, id, subscribed))
            .await
    }

    /// JMAP `isVisible` of a calendar (only the bridge keeps it).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account; on
    /// database errors.
    pub async fn set_collection_visible(
        &self,
        account: AccountId,
        id: CollectionId,
        visible: bool,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| set_visible(conn, account, id, visible))
            .await
    }

    /// Makes a live collection the default of its kind (JMAP
    /// `onSuccessSetIsDefault`; only the bridge keeps it): the previous
    /// default stops being one. Only one of the user's own, of `kind`.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account or
    /// kind, one shared by someone else, or one gone from the server;
    /// [`StoreError::Invalid`] for one that can't take new objects (a
    /// subscription, or read only); on database errors.
    pub async fn set_default_collection(
        &self,
        account: AccountId,
        id: CollectionId,
        kind: CollectionKind,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| set_default(conn, account, id, kind))
            .await
    }
}
