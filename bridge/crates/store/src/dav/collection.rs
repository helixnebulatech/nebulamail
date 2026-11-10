// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Calendars and address books: the lists of the homes, applied in one
//! transaction with their changes.

use std::collections::{BTreeMap, HashMap, HashSet};

use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, Queryable, RunQueryDsl as _,
    Selectable, SelectableHelper as _, SqliteConnection,
};

use super::types::{
    CalendarExtras, Collection, CollectionKind, CollectionProps, CollectionSharing, CollectionSync,
    Components, DavHref, Rights, SealedLink, ShareMode, VcardVersions,
};
use crate::mail::Recorded;
use crate::record::record_in;
use crate::schema::{dav_collections, dav_resources};
use crate::{AccountId, Change, ChangeKind, CollectionId, DataType, Store, StoreError, UnixTime};

#[derive(Queryable, Selectable)]
#[diesel(table_name = dav_collections, check_for_backend(diesel::sqlite::Sqlite))]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per column of the table"
)]
pub(crate) struct CollectionRow {
    id: i64,
    kind: i32,
    href: String,
    name: Option<String>,
    color: Option<String>,
    description: Option<String>,
    sort_order: Option<i64>,
    time_zone: Option<String>,
    rights: Option<i32>,
    components: Option<i32>,
    vcard_versions: Option<i32>,
    supports_sync: bool,
    sync_token: Option<String>,
    ctag: Option<String>,
    synced_at: Option<i64>,
    orphaned_at: Option<i64>,
    is_visible: bool,
    is_subscribed: bool,
    is_default: bool,
    owner: Option<String>,
    share_mode: Option<i32>,
    shares: Option<String>,
    default_alarms_datetime: Option<String>,
    default_alarms_date: Option<String>,
    publishable: bool,
    is_subscription: bool,
    publish_url_sealed: Option<Vec<u8>>,
    subscription_source_sealed: Option<Vec<u8>>,
}

pub(crate) fn collection_id(raw: i64) -> Result<CollectionId, StoreError> {
    CollectionId::from_raw(raw).ok_or(StoreError::Corrupt("collection id"))
}

impl TryFrom<CollectionRow> for Collection {
    type Error = StoreError;

    fn try_from(row: CollectionRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: collection_id(row.id)?,
            props: CollectionProps {
                kind: CollectionKind::from_code(row.kind)?,
                href: DavHref::from_db(row.href)?,
                name: row.name,
                color: row.color,
                description: row.description,
                sort_order: row.sort_order,
                time_zone: row.time_zone,
                default_alarms_datetime: row.default_alarms_datetime,
                default_alarms_date: row.default_alarms_date,
                rights: row.rights.map(Rights::from_db).transpose()?,
                components: row.components.map(Components::from_db).transpose()?,
                vcard_versions: row.vcard_versions.map(VcardVersions::from_db).transpose()?,
                supports_sync: row.supports_sync,
                sharing: CollectionSharing {
                    owner: row.owner.map(DavHref::from_db).transpose()?,
                    mode: row.share_mode.map(ShareMode::from_code).transpose()?,
                    shares: CollectionSharing::shares_from_db(row.shares.as_deref())?,
                },
                extras: CalendarExtras::from_db(
                    row.publishable,
                    row.publish_url_sealed,
                    row.is_subscription,
                    row.subscription_source_sealed,
                )?,
            },
            sync: CollectionSync {
                sync_token: row.sync_token,
                ctag: row.ctag,
                synced_at: row.synced_at.map(UnixTime::from_secs),
            },
            orphaned_at: row.orphaned_at.map(UnixTime::from_secs),
            is_visible: row.is_visible,
            is_subscribed: row.is_subscribed,
            is_default: row.is_default,
        })
    }
}

fn load_all(
    conn: &mut SqliteConnection,
    account: AccountId,
) -> Result<Vec<Collection>, StoreError> {
    dav_collections::table
        .filter(dav_collections::account_id.eq(account.get()))
        .order(dav_collections::id.asc())
        .select(CollectionRow::as_select())
        .load(conn)?
        .into_iter()
        .map(Collection::try_from)
        .collect()
}

/// The collection with this id, checking it belongs to the account.
pub(crate) fn load_collection(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: CollectionId,
) -> Result<Collection, StoreError> {
    dav_collections::table
        .find(id.get())
        .filter(dav_collections::account_id.eq(account.get()))
        .select(CollectionRow::as_select())
        .first(conn)
        .optional()?
        .ok_or(StoreError::NotFound("collection"))?
        .try_into()
}

/// The changes of one write, per data type.
#[derive(Default)]
pub(crate) struct Pending(BTreeMap<i32, (DataType, Vec<Change>)>);

impl Pending {
    pub(crate) fn push(&mut self, data_type: DataType, object: i64, kind: ChangeKind) {
        self.0
            .entry(data_type.code())
            .or_insert_with(|| (data_type, Vec::new()))
            .1
            .push(Change { object, kind });
    }

    pub(crate) fn record(
        self,
        conn: &mut SqliteConnection,
        account: AccountId,
    ) -> Result<Recorded, StoreError> {
        let mut recorded = Recorded::default();
        for (data_type, list) in self.0.into_values() {
            if !list.is_empty() {
                let seq = record_in(conn, account, data_type, &list)?;
                recorded.push(data_type, seq);
            }
        }
        Ok(recorded)
    }
}

fn set_props(
    conn: &mut SqliteConnection,
    id: i64,
    props: &CollectionProps,
) -> Result<(), StoreError> {
    let shares = props.sharing.shares_to_db()?;
    diesel::update(dav_collections::table.find(id))
        .set((
            dav_collections::name.eq(&props.name),
            dav_collections::color.eq(&props.color),
            dav_collections::description.eq(&props.description),
            dav_collections::sort_order.eq(props.sort_order),
            dav_collections::time_zone.eq(&props.time_zone),
            dav_collections::default_alarms_datetime.eq(&props.default_alarms_datetime),
            dav_collections::default_alarms_date.eq(&props.default_alarms_date),
            dav_collections::rights.eq(props.rights.map(Rights::to_db)),
            dav_collections::components.eq(props.components.map(Components::to_db)),
            dav_collections::vcard_versions.eq(props.vcard_versions.map(VcardVersions::to_db)),
            dav_collections::supports_sync.eq(props.supports_sync),
            dav_collections::owner.eq(props.sharing.owner.as_ref().map(DavHref::as_str)),
            dav_collections::share_mode.eq(props.sharing.mode.map(ShareMode::code)),
            dav_collections::shares.eq(shares),
            dav_collections::publishable.eq(props.extras.publishable),
            dav_collections::publish_url_sealed.eq(props
                .extras
                .publish_url
                .as_ref()
                .map(SealedLink::as_bytes)),
            dav_collections::is_subscription.eq(props.extras.is_subscription),
            dav_collections::subscription_source_sealed.eq(props
                .extras
                .subscription_source
                .as_ref()
                .map(SealedLink::as_bytes)),
        ))
        .execute(conn)?;
    Ok(())
}

fn insert(
    conn: &mut SqliteConnection,
    account: AccountId,
    props: &CollectionProps,
) -> Result<i64, StoreError> {
    let id: i64 = diesel::insert_into(dav_collections::table)
        .values((
            dav_collections::account_id.eq(account.get()),
            dav_collections::kind.eq(props.kind.code()),
            dav_collections::href.eq(props.href.as_str()),
            dav_collections::supports_sync.eq(props.supports_sync),
        ))
        .returning(dav_collections::id)
        .get_result(conn)?;
    set_props(conn, id, props)?;
    Ok(id)
}

/// Hides a collection that left the server, and its resources with it.
fn orphan(
    conn: &mut SqliteConnection,
    pending: &mut Pending,
    collection: &Collection,
    now: UnixTime,
) -> Result<(), StoreError> {
    let id = collection.id.get();
    let kind = collection.props.kind;
    let visible: Vec<i64> = dav_resources::table
        .filter(dav_resources::collection_id.eq(id))
        .filter(dav_resources::orphaned_at.is_null())
        .filter(dav_resources::invalid.eq(false))
        .select(dav_resources::id)
        .load(conn)?;
    for resource in visible {
        pending.push(kind.resource_data_type(), resource, ChangeKind::Destroyed);
    }
    diesel::update(
        dav_resources::table
            .filter(dav_resources::collection_id.eq(id))
            .filter(dav_resources::orphaned_at.is_null()),
    )
    .set(dav_resources::orphaned_at.eq(now.secs()))
    .execute(conn)?;
    diesel::update(dav_collections::table.find(id))
        .set((
            dav_collections::orphaned_at.eq(now.secs()),
            dav_collections::is_default.eq(false),
        ))
        .execute(conn)?;
    pending.push(kind.data_type(), id, ChangeKind::Destroyed);
    Ok(())
}

/// One default per kind among the user's own live collections that can
/// take new objects ([`Collection::can_be_default`]; never one shared by
/// someone else): the one already marked, else the first by `sort_order`
/// (then by id). A mark on any other collection (an older version allowed
/// shared ones) is cleared.
fn fix_defaults(
    conn: &mut SqliteConnection,
    account: AccountId,
    pending: &mut Pending,
) -> Result<(), StoreError> {
    let all = load_all(conn, account)?;
    for wrong in all.iter().filter(|c| c.is_default && !c.can_be_default()) {
        diesel::update(dav_collections::table.find(wrong.id.get()))
            .set(dav_collections::is_default.eq(false))
            .execute(conn)?;
        if wrong.orphaned_at.is_none() {
            pending.push(
                wrong.props.kind.data_type(),
                wrong.id.get(),
                ChangeKind::Updated,
            );
        }
    }
    let live: Vec<&Collection> = all.iter().filter(|c| c.can_be_default()).collect();
    for kind in [CollectionKind::Calendar, CollectionKind::AddressBook] {
        let of_kind = live.iter().filter(|c| c.props.kind == kind);
        if of_kind.clone().any(|c| c.is_default) {
            continue;
        }
        let first =
            of_kind.min_by_key(|c| (c.props.sort_order.is_none(), c.props.sort_order, c.id));
        if let Some(first) = first {
            diesel::update(dav_collections::table.find(first.id.get()))
                .set(dav_collections::is_default.eq(true))
                .execute(conn)?;
            pending.push(kind.data_type(), first.id.get(), ChangeKind::Updated);
        }
    }
    Ok(())
}

impl Collection {
    /// Whether it may be the default of its kind, where new objects go:
    /// live, the user's own, not a subscription, and writable (rights not
    /// said, or `bind` or `write-content`).
    pub(crate) fn can_be_default(&self) -> bool {
        self.orphaned_at.is_none()
            && self.props.sharing.owner.is_none()
            && !self.props.extras.is_subscription
            && self
                .props
                .rights
                .is_none_or(|r| r.contains(Rights::BIND) || r.contains(Rights::WRITE_CONTENT))
    }
}

fn apply(
    conn: &mut SqliteConnection,
    account: AccountId,
    listed: &[CollectionProps],
    now: UnixTime,
) -> Result<(Recorded, Vec<Collection>), StoreError> {
    let existing: HashMap<DavHref, Collection> = load_all(conn, account)?
        .into_iter()
        .map(|c| (c.props.href.clone(), c))
        .collect();
    let mut pending = Pending::default();
    let mut seen: HashSet<&DavHref> = HashSet::new();
    for props in listed {
        // Listed without `read` (e.g. a calendar shared for free/busy only):
        // treated as not on the server, so nothing of it is cached or shown,
        // and a cached copy is hidden with its resources.
        if props.rights.is_some_and(|r| !r.contains(Rights::READ)) {
            continue;
        }
        if !seen.insert(&props.href) {
            continue;
        }
        let Some(old) = existing.get(&props.href) else {
            let id = insert(conn, account, props)?;
            pending.push(props.kind.data_type(), id, ChangeKind::Created);
            continue;
        };
        // A calendar that became an address book (or the reverse) at the
        // same href: keep the old one, a JMAP object never changes type.
        if old.props.kind != props.kind {
            tracing::warn!("dav: a collection changed its kind; ignored");
            continue;
        }
        let id = old.id.get();
        let (differs, visible) = old.props.needs_update(props);
        if differs {
            set_props(conn, id, props)?;
        }
        if old.orphaned_at.is_some() {
            // Back on the server: shown again, and synced from scratch (its
            // resources were hidden with it; a full sync brings back the
            // ones still there by etag).
            diesel::update(dav_collections::table.find(id))
                .set((
                    dav_collections::orphaned_at.eq(None::<i64>),
                    dav_collections::sync_token.eq(None::<String>),
                    dav_collections::ctag.eq(None::<String>),
                    dav_collections::synced_at.eq(None::<i64>),
                ))
                .execute(conn)?;
            pending.push(props.kind.data_type(), id, ChangeKind::Created);
        } else if visible {
            pending.push(props.kind.data_type(), id, ChangeKind::Updated);
        }
    }
    for old in existing.values() {
        if old.orphaned_at.is_none() && !seen.contains(&old.props.href) {
            orphan(conn, &mut pending, old, now)?;
        }
    }
    fix_defaults(conn, account, &mut pending)?;
    let recorded = pending.record(conn, account)?;
    let live = load_all(conn, account)?
        .into_iter()
        .filter(|c| c.orphaned_at.is_none())
        .collect();
    Ok((recorded, live))
}

impl Store {
    /// Applies what the homes list now (every home of the account: what
    /// is not listed has left the server, and so has one listed without
    /// [`Rights::READ`]): new collections are created,
    /// changed ones updated, missing ones hidden with their resources (and
    /// deleted by [`Store::purge_dav_orphans`] unless they come back). One
    /// collection per kind is the default. Returns the recorded states and
    /// the live collections.
    ///
    /// # Errors
    /// On database errors.
    pub async fn apply_collections(
        &self,
        account: AccountId,
        listed: Vec<CollectionProps>,
        now: UnixTime,
    ) -> Result<(Recorded, Vec<Collection>), StoreError> {
        self.write(move |conn| apply(conn, account, &listed, now))
            .await
    }

    /// Every collection of the account, also hidden ones.
    ///
    /// # Errors
    /// On database errors.
    pub async fn dav_collections(&self, account: AccountId) -> Result<Vec<Collection>, StoreError> {
        self.read(move |conn| load_all(conn, account)).await
    }

    /// Records that a sync of the collection finished with this token and
    /// ctag (only then: an interrupted sync starts again from the old ones).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] for a collection of another account; on
    /// database errors.
    pub async fn set_collection_synced(
        &self,
        account: AccountId,
        collection: CollectionId,
        sync_token: Option<String>,
        ctag: Option<String>,
        now: UnixTime,
    ) -> Result<(), StoreError> {
        self.write(move |conn| {
            let changed = diesel::update(
                dav_collections::table
                    .find(collection.get())
                    .filter(dav_collections::account_id.eq(account.get())),
            )
            .set((
                dav_collections::sync_token.eq(sync_token),
                dav_collections::ctag.eq(ctag),
                dav_collections::synced_at.eq(now.secs()),
            ))
            .execute(conn)?;
            if changed == 0 {
                return Err(StoreError::NotFound("collection"));
            }
            Ok(())
        })
        .await
    }

    /// Deletes collections and resources hidden before `before` (with
    /// their alerts). Their ids are never given out again.
    ///
    /// # Errors
    /// On database errors.
    pub async fn purge_dav_orphans(
        &self,
        account: AccountId,
        before: UnixTime,
    ) -> Result<(), StoreError> {
        self.write(move |conn| {
            diesel::delete(
                dav_resources::table
                    .filter(dav_resources::account_id.eq(account.get()))
                    .filter(dav_resources::orphaned_at.lt(before.secs())),
            )
            .execute(conn)?;
            diesel::delete(
                dav_collections::table
                    .filter(dav_collections::account_id.eq(account.get()))
                    .filter(dav_collections::orphaned_at.lt(before.secs())),
            )
            .execute(conn)?;
            Ok(())
        })
        .await
    }
}
