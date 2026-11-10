// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Mailboxes: the list from the server, renames, sync state and counts.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use diesel::sql_types::BigInt;
use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, Queryable, RunQueryDsl as _,
    Selectable, SelectableHelper as _, SqliteConnection,
};
use nebula_imap_proto::types::{MailboxName, UidValidity};

use super::changeset::{ChangeSet, Counts};
use super::convert::{
    delimiter_from_db, delimiter_to_db, mailbox_id, mod_seq_from_db, mod_seq_to_db, name_from_db,
    name_to_db, opt, u32_from_db, uid_from_db, uid_to_db, validity_from_db, validity_to_db,
};
use super::types::{ListedMailbox, Mailbox, MailboxCounts, MailboxSyncState, Recorded, Role};
use crate::schema::{email_locations, mailboxes};
use crate::{AccountId, ChangeKind, MailboxId, Store, StoreError, UnixTime};

#[derive(Queryable, Selectable)]
#[diesel(table_name = mailboxes, check_for_backend(diesel::sqlite::Sqlite))]
pub(crate) struct MailboxRow {
    id: i64,
    imap_name: String,
    name: String,
    delimiter: Option<String>,
    parent_id: Option<i64>,
    role: Option<i32>,
    subscribed: bool,
    selectable: bool,
    sort_order: i64,
    uid_validity: Option<i64>,
    uid_next: Option<i64>,
    highest_modseq: Option<i64>,
    messages: Option<i64>,
    synced_at: Option<i64>,
    no_inferiors: bool,
}

impl TryFrom<MailboxRow> for Mailbox {
    type Error = StoreError;

    fn try_from(row: MailboxRow) -> Result<Self, StoreError> {
        Ok(Self {
            id: mailbox_id(row.id)?,
            imap_name: name_from_db(&row.imap_name)?,
            name: row.name,
            delimiter: delimiter_from_db(row.delimiter.as_deref())?,
            parent: opt(row.parent_id, mailbox_id)?,
            role: opt(row.role, Role::from_code)?,
            subscribed: row.subscribed,
            selectable: row.selectable,
            no_inferiors: row.no_inferiors,
            sort_order: u32_from_db(row.sort_order, "sort order")?,
            sync: MailboxSyncState {
                uid_validity: opt(row.uid_validity, validity_from_db)?,
                uid_next: opt(row.uid_next, uid_from_db)?,
                highest_mod_seq: opt(row.highest_modseq, mod_seq_from_db)?,
                messages: opt(row.messages, |n| u32_from_db(n, "message count"))?,
                synced_at: row.synced_at.map(UnixTime::from_secs),
            },
        })
    }
}

/// The mailbox with this id, checking it belongs to the account.
pub(crate) fn load_mailbox(
    conn: &mut SqliteConnection,
    account: AccountId,
    id: MailboxId,
) -> Result<Mailbox, StoreError> {
    mailboxes::table
        .find(id.get())
        .filter(mailboxes::account_id.eq(account.get()))
        .select(MailboxRow::as_select())
        .first(conn)
        .optional()?
        .ok_or(StoreError::NotFound("mailbox"))?
        .try_into()
}

fn load_all(conn: &mut SqliteConnection, account: AccountId) -> Result<Vec<Mailbox>, StoreError> {
    mailboxes::table
        .filter(mailboxes::account_id.eq(account.get()))
        .order(mailboxes::id.asc())
        .select(MailboxRow::as_select())
        .load(conn)?
        .into_iter()
        .map(Mailbox::try_from)
        .collect()
}

/// Removes every location of a mailbox (its emails may disappear).
pub(crate) fn clear_locations(
    conn: &mut SqliteConnection,
    changes: &mut ChangeSet,
    mailbox: i64,
) -> Result<(), StoreError> {
    let emails: Vec<i64> = email_locations::table
        .filter(email_locations::mailbox_id.eq(mailbox))
        .select(email_locations::email_id)
        .distinct()
        .load(conn)?;
    for email in emails {
        changes.touch_email(conn, email, Counts::MayChange)?;
    }
    diesel::delete(email_locations::table.filter(email_locations::mailbox_id.eq(mailbox)))
        .execute(conn)?;
    changes.mailbox(mailbox, ChangeKind::CountsUpdated);
    Ok(())
}

/// Gives each role to one mailbox only: the first that claims it.
fn unique_roles(listed: &[ListedMailbox]) -> Vec<Option<Role>> {
    let mut taken = BTreeSet::new();
    listed
        .iter()
        .map(|mailbox| mailbox.role.filter(|role| taken.insert(*role)))
        .collect()
}

fn sync_list(
    conn: &mut SqliteConnection,
    account: AccountId,
    listed: &[ListedMailbox],
    now: UnixTime,
) -> Result<Recorded, StoreError> {
    let mut changes = ChangeSet::new(account, now);
    let existing: HashMap<String, Mailbox> = load_all(conn, account)?
        .into_iter()
        .map(|mailbox| (name_to_db(&mailbox.imap_name), mailbox))
        .collect();
    let roles = unique_roles(listed);

    // Free the roles that move first, so the unique index never trips.
    for (mailbox, role) in listed.iter().zip(&roles) {
        if let Some(old) = existing.get(&name_to_db(&mailbox.imap_name))
            && old.role != *role
            && old.role.is_some()
        {
            diesel::update(mailboxes::table.find(old.id.get()))
                .set(mailboxes::role.eq(None::<i32>))
                .execute(conn)?;
        }
    }
    let listed_names: BTreeSet<String> = listed.iter().map(|m| name_to_db(&m.imap_name)).collect();
    for (name, old) in &existing {
        if !listed_names.contains(name) && old.role.is_some() {
            diesel::update(mailboxes::table.find(old.id.get()))
                .set(mailboxes::role.eq(None::<i32>))
                .execute(conn)?;
        }
    }

    let mut ids: HashMap<String, i64> = HashMap::new();
    for (mailbox, role) in listed.iter().zip(&roles) {
        let key = name_to_db(&mailbox.imap_name);
        let id = upsert(
            conn,
            account,
            &mut changes,
            existing.get(&key),
            mailbox,
            *role,
        )?;
        ids.insert(key, id);
    }

    // Parents, now that every listed mailbox has an id.
    for mailbox in listed {
        let key = name_to_db(&mailbox.imap_name);
        let Some(&id) = ids.get(&key) else { continue };
        let parent = mailbox
            .parent
            .as_ref()
            .and_then(|parent| ids.get(&name_to_db(parent)).copied())
            .filter(|&parent| parent != id);
        let old_parent = existing
            .get(&key)
            .and_then(|old| old.parent.map(MailboxId::get));
        if !existing.contains_key(&key) || old_parent != parent {
            diesel::update(mailboxes::table.find(id))
                .set(mailboxes::parent_id.eq(parent))
                .execute(conn)?;
            changes.mailbox(id, ChangeKind::Updated);
        }
    }

    // Gone from the server: their emails go with them.
    for (name, old) in &existing {
        if listed_names.contains(name) {
            continue;
        }
        let id = old.id.get();
        clear_locations(conn, &mut changes, id)?;
        diesel::update(mailboxes::table.filter(mailboxes::parent_id.eq(id)))
            .set(mailboxes::parent_id.eq(None::<i64>))
            .execute(conn)?;
        diesel::delete(mailboxes::table.find(id)).execute(conn)?;
        changes.mailbox(id, ChangeKind::Destroyed);
    }
    changes.commit(conn)
}

/// Inserts or updates one listed mailbox (not its parent) and returns its id.
fn upsert(
    conn: &mut SqliteConnection,
    account: AccountId,
    changes: &mut ChangeSet,
    old: Option<&Mailbox>,
    mailbox: &ListedMailbox,
    role: Option<Role>,
) -> Result<i64, StoreError> {
    let sort_order = i64::from(mailbox.sort_order);
    let values = (
        mailboxes::name.eq(&mailbox.name),
        mailboxes::delimiter.eq(delimiter_to_db(mailbox.delimiter)),
        mailboxes::role.eq(role.map(Role::code)),
        mailboxes::subscribed.eq(mailbox.subscribed),
        mailboxes::selectable.eq(mailbox.selectable),
        mailboxes::no_inferiors.eq(mailbox.no_inferiors),
        mailboxes::sort_order.eq(sort_order),
    );
    let Some(old) = old else {
        let id: i64 = diesel::insert_into(mailboxes::table)
            .values((
                mailboxes::account_id.eq(account.get()),
                mailboxes::imap_name.eq(name_to_db(&mailbox.imap_name)),
                values,
            ))
            .returning(mailboxes::id)
            .get_result(conn)?;
        changes.mailbox(id, ChangeKind::Created);
        return Ok(id);
    };
    let id = old.id.get();
    let same = old.name == mailbox.name
        && old.delimiter == mailbox.delimiter
        && old.role == role
        && old.subscribed == mailbox.subscribed
        && old.selectable == mailbox.selectable
        && old.no_inferiors == mailbox.no_inferiors
        && old.sort_order == mailbox.sort_order;
    if !same {
        diesel::update(mailboxes::table.find(id))
            .set(values)
            .execute(conn)?;
        changes.mailbox(id, ChangeKind::Updated);
    }
    Ok(id)
}

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = BigInt)]
    mailbox: i64,
    #[diesel(sql_type = BigInt)]
    total: i64,
    #[diesel(sql_type = BigInt)]
    unread: i64,
    #[diesel(sql_type = BigInt)]
    threads: i64,
}

#[derive(diesel::QueryableByName)]
struct UnreadThreadsRow {
    #[diesel(sql_type = BigInt)]
    mailbox: i64,
    #[diesel(sql_type = BigInt)]
    unread_threads: i64,
}

/// Emails without `$seen` and without `$draft` (RFC 8621 §2 "unread").
const UNREAD: &str = "NOT EXISTS (SELECT 1 FROM email_keywords k WHERE k.email_id = e.id \
                      AND k.keyword IN ('$seen', '$draft'))";

fn counts(
    conn: &mut SqliteConnection,
    account: AccountId,
) -> Result<BTreeMap<MailboxId, MailboxCounts>, StoreError> {
    let rows: Vec<CountRow> = diesel::sql_query(format!(
        "SELECT l.mailbox_id AS mailbox, COUNT(DISTINCT l.email_id) AS total, \
         COUNT(DISTINCT CASE WHEN {UNREAD} THEN l.email_id END) AS unread, \
         COUNT(DISTINCT e.thread_id) AS threads \
         FROM email_locations l JOIN mailboxes m ON m.id = l.mailbox_id \
         JOIN emails e ON e.id = l.email_id \
         WHERE m.account_id = ? GROUP BY l.mailbox_id"
    ))
    .bind::<BigInt, _>(account.get())
    .load(conn)?;
    // A thread is unread in a mailbox if one of its emails is there and an
    // unread one is anywhere; emails in Trash only count for Trash.
    let trash = Role::Trash.code();
    let unread_threads: Vec<UnreadThreadsRow> = diesel::sql_query(format!(
        "WITH trash AS (SELECT id FROM mailboxes WHERE account_id = ?1 AND role = {trash}), \
         unread AS (SELECT e.thread_id AS thread_id, \
             EXISTS (SELECT 1 FROM email_locations l WHERE l.email_id = e.id \
                     AND l.mailbox_id IN (SELECT id FROM trash)) AS in_trash \
           FROM emails e WHERE e.account_id = ?1 AND {UNREAD} \
           AND EXISTS (SELECT 1 FROM email_locations l WHERE l.email_id = e.id)) \
         SELECT l.mailbox_id AS mailbox, COUNT(DISTINCT e.thread_id) AS unread_threads \
         FROM email_locations l JOIN mailboxes m ON m.id = l.mailbox_id \
         JOIN emails e ON e.id = l.email_id \
         WHERE m.account_id = ?1 AND EXISTS (SELECT 1 FROM unread u \
           WHERE u.thread_id = e.thread_id \
           AND u.in_trash = (l.mailbox_id IN (SELECT id FROM trash))) \
         GROUP BY l.mailbox_id"
    ))
    .bind::<BigInt, _>(account.get())
    .load(conn)?;

    let count = |n: i64| u64::try_from(n).map_err(|_| StoreError::Corrupt("count"));
    let mut map = BTreeMap::new();
    for row in rows {
        map.insert(
            mailbox_id(row.mailbox)?,
            MailboxCounts {
                total_emails: count(row.total)?,
                unread_emails: count(row.unread)?,
                total_threads: count(row.threads)?,
                unread_threads: 0,
            },
        );
    }
    for row in unread_threads {
        if let Some(entry) = map.get_mut(&mailbox_id(row.mailbox)?) {
            entry.unread_threads = count(row.unread_threads)?;
        }
    }
    Ok(map)
}

impl Store {
    /// The mailboxes of an account, oldest first.
    ///
    /// # Errors
    /// On database errors.
    pub async fn mailboxes(&self, account: AccountId) -> Result<Vec<Mailbox>, StoreError> {
        self.read(move |conn| load_all(conn, account)).await
    }

    /// One mailbox of the account.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if it doesn't exist in this account.
    pub async fn mailbox(&self, account: AccountId, id: MailboxId) -> Result<Mailbox, StoreError> {
        self.read(move |conn| load_mailbox(conn, account, id)).await
    }

    /// The mailbox with this IMAP name, if any.
    ///
    /// # Errors
    /// On database errors.
    pub async fn mailbox_by_name(
        &self,
        account: AccountId,
        name: &MailboxName,
    ) -> Result<Option<Mailbox>, StoreError> {
        let name = name_to_db(name);
        self.read(move |conn| {
            mailboxes::table
                .filter(mailboxes::account_id.eq(account.get()))
                .filter(mailboxes::imap_name.eq(name))
                .select(MailboxRow::as_select())
                .first(conn)
                .optional()?
                .map(Mailbox::try_from)
                .transpose()
        })
        .await
    }

    /// The mailbox with this role, if any.
    ///
    /// # Errors
    /// On database errors.
    pub async fn mailbox_by_role(
        &self,
        account: AccountId,
        role: Role,
    ) -> Result<Option<Mailbox>, StoreError> {
        self.read(move |conn| {
            mailboxes::table
                .filter(mailboxes::account_id.eq(account.get()))
                .filter(mailboxes::role.eq(role.code()))
                .select(MailboxRow::as_select())
                .first(conn)
                .optional()?
                .map(Mailbox::try_from)
                .transpose()
        })
        .await
    }

    /// Email and thread counts per mailbox. Mailboxes without emails are
    /// missing from the map (all their counts are 0).
    ///
    /// # Errors
    /// On database errors.
    pub async fn mailbox_counts(
        &self,
        account: AccountId,
    ) -> Result<BTreeMap<MailboxId, MailboxCounts>, StoreError> {
        self.read(move |conn| counts(conn, account)).await
    }

    /// Replaces the account's mailboxes with the list from the server:
    /// new ones are created, changed ones updated, missing ones destroyed
    /// with their emails. A folder renamed by another client looks like a
    /// delete and a create.
    ///
    /// # Errors
    /// On database errors.
    pub async fn sync_mailboxes(
        &self,
        account: AccountId,
        listed: Vec<ListedMailbox>,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| sync_list(conn, account, &listed, now))
            .await
    }

    /// After a successful IMAP RENAME: the mailbox (and every mailbox below
    /// it) gets the new name, keeping its id.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn rename_mailbox(
        &self,
        account: AccountId,
        id: MailboxId,
        new_name: MailboxName,
        display_name: String,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            let old = load_mailbox(conn, account, id)?;
            let old_key = name_to_db(&old.imap_name);
            let new_key = name_to_db(&new_name);
            let mut changes = ChangeSet::new(account, now);
            if let Some(delimiter) = old.delimiter {
                let prefix = format!("{old_key}{delimiter}");
                for child in load_all(conn, account)? {
                    let key = name_to_db(&child.imap_name);
                    if let Some(rest) = key.strip_prefix(&prefix) {
                        diesel::update(mailboxes::table.find(child.id.get()))
                            .set(mailboxes::imap_name.eq(format!("{new_key}{delimiter}{rest}")))
                            .execute(conn)?;
                    }
                }
            }
            diesel::update(mailboxes::table.find(id.get()))
                .set((
                    mailboxes::imap_name.eq(new_key),
                    mailboxes::name.eq(display_name),
                ))
                .execute(conn)?;
            changes.mailbox(id.get(), ChangeKind::Updated);
            changes.commit(conn)
        })
        .await
    }

    /// Saves the IMAP sync state of a mailbox (not visible over JMAP).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn set_mailbox_sync_state(
        &self,
        account: AccountId,
        id: MailboxId,
        state: MailboxSyncState,
    ) -> Result<(), StoreError> {
        self.write(move |conn| {
            let current = load_mailbox(conn, account, id)?;
            if current.sync.uid_validity.is_some()
                && current.sync.uid_validity != state.uid_validity
            {
                // A different UIDVALIDITY must go through `reset_mailbox`.
                return Err(StoreError::Invalid("UIDVALIDITY change without reset"));
            }
            diesel::update(mailboxes::table.find(id.get()))
                .set((
                    mailboxes::uid_validity.eq(state.uid_validity.map(validity_to_db)),
                    mailboxes::uid_next.eq(state.uid_next.map(uid_to_db)),
                    mailboxes::highest_modseq.eq(opt(state.highest_mod_seq, mod_seq_to_db)?),
                    mailboxes::messages.eq(state.messages.map(i64::from)),
                    mailboxes::synced_at.eq(state.synced_at.map(UnixTime::secs)),
                ))
                .execute(conn)?;
            Ok(())
        })
        .await
    }

    /// The server's UIDVALIDITY changed (or is first seen): every stored
    /// location of the mailbox is dropped and its sync state starts over
    /// (never synced: `synced_at` too).
    /// Emails with a server id are kept hidden for a while, so the resync
    /// gives them their old ids back.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn reset_mailbox(
        &self,
        account: AccountId,
        id: MailboxId,
        uid_validity: UidValidity,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            load_mailbox(conn, account, id)?;
            let mut changes = ChangeSet::new(account, now);
            clear_locations(conn, &mut changes, id.get())?;
            diesel::update(mailboxes::table.find(id.get()))
                .set((
                    mailboxes::uid_validity.eq(Some(validity_to_db(uid_validity))),
                    mailboxes::uid_next.eq(None::<i64>),
                    mailboxes::highest_modseq.eq(None::<i64>),
                    mailboxes::messages.eq(None::<i64>),
                    // A copy cut short after a reset is still the first.
                    mailboxes::synced_at.eq(None::<i64>),
                ))
                .execute(conn)?;
            changes.commit(conn)
        })
        .await
    }

    /// After IMAP SUBSCRIBE / UNSUBSCRIBE.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn set_mailbox_subscribed(
        &self,
        account: AccountId,
        id: MailboxId,
        subscribed: bool,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            let current = load_mailbox(conn, account, id)?;
            let mut changes = ChangeSet::new(account, now);
            if current.subscribed != subscribed {
                diesel::update(mailboxes::table.find(id.get()))
                    .set(mailboxes::subscribed.eq(subscribed))
                    .execute(conn)?;
                changes.mailbox(id.get(), ChangeKind::Updated);
            }
            changes.commit(conn)
        })
        .await
    }
}
