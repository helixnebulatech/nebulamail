// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Collects what a write transaction touched and turns it into JMAP change
//! records at the end, in the same transaction.
//!
//! Callers "touch" an object *before* changing it, so its old state is
//! known; [`ChangeSet::commit`] compares with the new state:
//!
//! - an email is visible while it has a location: invisible → visible is
//!   `created`, visible → visible `updated`, visible → invisible `destroyed`;
//! - a thread is compared by its list of visible emails;
//! - mailboxes get `updated` when their counts may have changed: every
//!   mailbox holding an email of a thread whose emails or `$seen`/`$draft`
//!   keywords changed (unread threads count emails anywhere in the thread).
//!   Such an update is recorded as [`ChangeKind::CountsUpdated`] unless a
//!   real property changed too, so `Mailbox/changes` can say only the
//!   counters changed (`updatedProperties`).

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use diesel::sql_types::BigInt;
use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _,
    SqliteConnection,
};

use crate::mail::types::Recorded;
use crate::record::record_in;
use crate::schema::{email_locations, emails, threads};
use crate::{AccountId, Change, ChangeKind, DataType, StoreError, UnixTime};

/// Whether a change to an email can change mailbox counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Counts {
    /// Locations, `$seen` or `$draft` changed.
    MayChange,
    /// Only other keywords changed.
    Unchanged,
}

pub(crate) struct ChangeSet {
    account: AccountId,
    now: UnixTime,
    /// Email → visible before the transaction.
    emails: BTreeMap<i64, bool>,
    /// Thread → its visible emails before the transaction.
    threads: BTreeMap<i64, BTreeSet<i64>>,
    /// Explicit mailbox changes.
    mailboxes: BTreeMap<i64, ChangeKind>,
    /// Threads whose mailboxes' counts may have changed.
    count_threads: BTreeSet<i64>,
}

impl ChangeSet {
    pub(crate) fn new(account: AccountId, now: UnixTime) -> Self {
        Self {
            account,
            now,
            emails: BTreeMap::new(),
            threads: BTreeMap::new(),
            mailboxes: BTreeMap::new(),
            count_threads: BTreeSet::new(),
        }
    }

    /// Call before changing an existing email.
    pub(crate) fn touch_email(
        &mut self,
        conn: &mut SqliteConnection,
        email: i64,
        counts: Counts,
    ) -> Result<(), StoreError> {
        if let Entry::Vacant(entry) = self.emails.entry(email) {
            entry.insert(is_visible(conn, email)?);
        }
        if counts == Counts::MayChange {
            let thread: i64 = emails::table
                .find(email)
                .select(emails::thread_id)
                .first(conn)?;
            self.touch_thread(conn, thread)?;
        }
        Ok(())
    }

    /// Call right after inserting an email (it was not visible before).
    pub(crate) fn new_email(&mut self, email: i64) {
        self.emails.entry(email).or_insert(false);
    }

    /// Call before changing which emails a thread holds.
    pub(crate) fn touch_thread(
        &mut self,
        conn: &mut SqliteConnection,
        thread: i64,
    ) -> Result<(), StoreError> {
        if let Entry::Vacant(entry) = self.threads.entry(thread) {
            entry.insert(visible_emails(conn, thread)?);
        }
        self.count_threads.insert(thread);
        Ok(())
    }

    /// Call right after inserting a thread.
    pub(crate) fn new_thread(&mut self, thread: i64) {
        self.threads.entry(thread).or_default();
        self.count_threads.insert(thread);
    }

    /// Records a mailbox change. `created` then `destroyed` cancel out;
    /// `updated` never hides `created` or `destroyed`, and a counts-only
    /// update never hides a real one.
    pub(crate) fn mailbox(&mut self, mailbox: i64, kind: ChangeKind) {
        use ChangeKind::{CountsUpdated, Created, Destroyed, Updated};
        let merged = match (self.mailboxes.get(&mailbox), kind) {
            (None, kind) => Some(kind),
            (Some(Created), Destroyed) => None,
            (Some(Created), Created | Updated | CountsUpdated) => Some(Created),
            (Some(Destroyed), _) | (Some(Updated | CountsUpdated), Destroyed) => Some(Destroyed),
            (Some(Updated), Created | Updated | CountsUpdated)
            | (Some(CountsUpdated), Created | Updated) => Some(Updated),
            (Some(CountsUpdated), CountsUpdated) => Some(CountsUpdated),
        };
        match merged {
            Some(kind) => self.mailboxes.insert(mailbox, kind),
            None => self.mailboxes.remove(&mailbox),
        };
    }

    /// Writes the change records (and removes rows nothing points to).
    pub(crate) fn commit(mut self, conn: &mut SqliteConnection) -> Result<Recorded, StoreError> {
        let email_changes = self.settle_emails(conn)?;
        let thread_changes = self.settle_threads(conn)?;
        self.count_mailboxes(conn)?;
        let mailbox_changes: Vec<Change> = self
            .mailboxes
            .iter()
            .map(|(&object, &kind)| Change { object, kind })
            .collect();

        // New emails also move `EmailDelivery` (RFC 8621 §1.5).
        let delivered: Vec<Change> = email_changes
            .iter()
            .filter(|change| change.kind == ChangeKind::Created)
            .copied()
            .collect();
        let mut recorded = Recorded::default();
        recorded.set_delivered(
            delivered
                .iter()
                .map(|change| {
                    crate::EmailId::from_raw(change.object).ok_or(StoreError::Corrupt("email id"))
                })
                .collect::<Result<_, _>>()?,
        );
        for (data_type, list) in [
            (DataType::Mailbox, mailbox_changes),
            (DataType::Email, email_changes),
            (DataType::Thread, thread_changes),
            (DataType::EmailDelivery, delivered),
        ] {
            if !list.is_empty() {
                let seq = record_in(conn, self.account, data_type, &list)?;
                recorded.push(data_type, seq);
            }
        }
        Ok(recorded)
    }

    fn settle_emails(&mut self, conn: &mut SqliteConnection) -> Result<Vec<Change>, StoreError> {
        let mut list = Vec::new();
        let touched: Vec<(i64, bool)> = self.emails.iter().map(|(&id, &v)| (id, v)).collect();
        for (email, before) in touched {
            let after = is_visible(conn, email)?;
            if after {
                diesel::update(emails::table.find(email))
                    .set(emails::orphaned_at.eq(None::<i64>))
                    .execute(conn)?;
            } else {
                self.park_or_delete(conn, email)?;
            }
            let kind = match (before, after) {
                (false, true) => ChangeKind::Created,
                (true, true) => ChangeKind::Updated,
                (true, false) => ChangeKind::Destroyed,
                (false, false) => continue,
            };
            list.push(Change {
                object: email,
                kind,
            });
        }
        Ok(list)
    }

    /// An email without locations: kept (hidden) for a while if the server
    /// gave it an id, so it can come back with the same JMAP id; deleted
    /// otherwise.
    fn park_or_delete(
        &mut self,
        conn: &mut SqliteConnection,
        email: i64,
    ) -> Result<(), StoreError> {
        let row: Option<(Option<String>, i64)> = emails::table
            .find(email)
            .select((emails::server_id, emails::thread_id))
            .first(conn)
            .optional()?;
        let Some((server_id, thread)) = row else {
            return Ok(());
        };
        if server_id.is_some() {
            diesel::update(
                emails::table
                    .find(email)
                    .filter(emails::orphaned_at.is_null()),
            )
            .set(emails::orphaned_at.eq(self.now.secs()))
            .execute(conn)?;
        } else {
            diesel::delete(emails::table.find(email)).execute(conn)?;
        }
        // Its thread may be empty now: settle it too.
        self.threads.entry(thread).or_default();
        Ok(())
    }

    fn settle_threads(&self, conn: &mut SqliteConnection) -> Result<Vec<Change>, StoreError> {
        let mut list = Vec::new();
        for (&thread, before) in &self.threads {
            let after = visible_emails(conn, thread)?;
            let kind = match (before.is_empty(), after.is_empty()) {
                _ if *before == after => None,
                (true, false) => Some(ChangeKind::Created),
                (false, true) => Some(ChangeKind::Destroyed),
                (false, false) => Some(ChangeKind::Updated),
                (true, true) => None,
            };
            if let Some(kind) = kind {
                list.push(Change {
                    object: thread,
                    kind,
                });
            }
            let rows: i64 = emails::table
                .filter(emails::thread_id.eq(thread))
                .count()
                .get_result(conn)?;
            if rows == 0 {
                diesel::delete(threads::table.find(thread)).execute(conn)?;
            }
        }
        Ok(list)
    }

    fn count_mailboxes(&mut self, conn: &mut SqliteConnection) -> Result<(), StoreError> {
        for &thread in &self.count_threads.clone() {
            let mailboxes: Vec<i64> = email_locations::table
                .inner_join(emails::table)
                .filter(emails::thread_id.eq(thread))
                .select(email_locations::mailbox_id)
                .distinct()
                .load(conn)?;
            for mailbox in mailboxes {
                self.mailbox(mailbox, ChangeKind::CountsUpdated);
            }
        }
        Ok(())
    }
}

pub(crate) fn is_visible(conn: &mut SqliteConnection, email: i64) -> Result<bool, StoreError> {
    let found: Option<i64> = email_locations::table
        .filter(email_locations::email_id.eq(email))
        .select(email_locations::email_id)
        .first(conn)
        .optional()?;
    Ok(found.is_some())
}

#[derive(diesel::QueryableByName)]
struct IdRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
}

fn visible_emails(conn: &mut SqliteConnection, thread: i64) -> Result<BTreeSet<i64>, StoreError> {
    let rows: Vec<IdRow> = diesel::sql_query(
        "SELECT e.id AS id FROM emails e WHERE e.thread_id = ? \
         AND EXISTS (SELECT 1 FROM email_locations l WHERE l.email_id = e.id)",
    )
    .bind::<BigInt, _>(thread)
    .load(conn)?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}
