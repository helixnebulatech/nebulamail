// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP state and change log (RFC 8620 §5.2). Each account
//! and data type has a sequence number that goes up with every change, and
//! one row per changed object so `*/changes` can answer "what changed since
//! state N".

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use diesel::{
    BoolExpressionMethods as _, ExpressionMethods as _, OptionalExtension as _, QueryDsl as _,
    RunQueryDsl as _, SqliteConnection,
};

use crate::record::record_in;
use crate::schema::{changes, states};
use crate::{AccountId, Store, StoreError};

/// The kinds of data with their own JMAP state. The number is what the
/// database stores: never reuse one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DataType {
    /// `Mailbox`.
    Mailbox,
    /// `Email`.
    Email,
    /// `Thread`.
    Thread,
    /// `Identity`.
    Identity,
    /// `EmailSubmission`.
    EmailSubmission,
    /// `Calendar` (a CalDAV calendar).
    Calendar,
    /// `CalendarEvent` (an iCalendar object of a calendar).
    CalendarEvent,
    /// `AddressBook` (a CardDAV address book).
    AddressBook,
    /// `ContactCard` (a vCard of an address book).
    ContactCard,
    /// `ParticipantIdentity` (from `calendar-user-address-set`).
    ParticipantIdentity,
    /// `EmailDelivery` (RFC 8621 §1.5): its state moves only when new
    /// emails appear (delivered, appended, imported or copied in), as in
    /// Stalwart; flags, moves and deletions leave it. Push only: no
    /// `/changes`.
    EmailDelivery,
    /// `SieveScript` (RFC 9661; a script on the ManageSieve server).
    SieveScript,
}

impl DataType {
    pub(crate) fn code(self) -> i32 {
        match self {
            Self::Mailbox => 1,
            Self::Email => 2,
            Self::Thread => 3,
            Self::Identity => 4,
            Self::EmailSubmission => 5,
            Self::Calendar => 6,
            Self::CalendarEvent => 7,
            Self::AddressBook => 8,
            Self::ContactCard => 9,
            Self::ParticipantIdentity => 10,
            Self::EmailDelivery => 11,
            Self::SieveScript => 12,
        }
    }
}

/// What happened to an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// It appeared.
    Created,
    /// One of its properties changed.
    Updated,
    /// It's gone.
    Destroyed,
    /// Only its counters changed (mailboxes: `totalEmails`,
    /// `unreadEmails`, `totalThreads`, `unreadThreads`). Reported as
    /// updated; lets `Mailbox/changes` send `updatedProperties`.
    CountsUpdated,
}

impl ChangeKind {
    pub(crate) fn code(self) -> i32 {
        match self {
            Self::Created => 1,
            Self::Updated => 2,
            Self::Destroyed => 3,
            Self::CountsUpdated => 4,
        }
    }

    fn from_code(code: i32) -> Result<Self, StoreError> {
        match code {
            1 => Ok(Self::Created),
            2 => Ok(Self::Updated),
            3 => Ok(Self::Destroyed),
            4 => Ok(Self::CountsUpdated),
            _ => Err(StoreError::Corrupt("change kind")),
        }
    }
}

/// A position in the change log of one account and data type. JMAP sends it
/// as the `state` string (its decimal form). 0 means "nothing yet".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(u64);

impl Seq {
    /// The state before any change.
    pub const ZERO: Self = Self(0);

    /// The sequence number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }

    /// The state from its number (e.g. parsed from a JMAP `state` string).
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) fn from_db(value: i64) -> Result<Self, StoreError> {
        u64::try_from(value)
            .map(Self)
            .map_err(|_| StoreError::Corrupt("state"))
    }

    fn to_db(self) -> Result<i64, StoreError> {
        i64::try_from(self.0).map_err(|_| StoreError::Corrupt("state"))
    }
}

impl std::fmt::Display for Seq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// One change to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// The changed object's row id.
    pub object: i64,
    /// What happened.
    pub kind: ChangeKind,
}

/// The answer to "what changed since state N" (RFC 8620 §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesSince {
    /// The changes, with each object in at most one list.
    Changes(Changes),
    /// The log no longer goes back that far, or the state is from the
    /// future: the client must reload (`cannotCalculateChanges`).
    CannotCalculate,
    /// The first state after `since` alone has more objects than
    /// `max_changes`. A state is never split, so no intermediate state
    /// fits: RFC 8620 §5.2 wants `cannotCalculateChanges` for `/changes`
    /// (§5.6 `tooManyChanges` for `/queryChanges` with `maxChanges`).
    TooMany,
}

/// Changes since a state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Created after the old state (and not destroyed since).
    pub created: Vec<i64>,
    /// Existed at the old state and changed.
    pub updated: Vec<i64>,
    /// Existed at the old state and are gone.
    pub destroyed: Vec<i64>,
    /// The state these changes lead to.
    pub new_state: Seq,
    /// More changes after `new_state`: call again from there.
    pub has_more_changes: bool,
    /// Every object in `updated` only had its counters changed
    /// ([`ChangeKind::CountsUpdated`]); `false` if `updated` is empty.
    pub only_counts_updated: bool,
}

/// How each object ended up within the reported range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Net {
    Created,
    Updated,
    /// Updated, but only its counters.
    CountsUpdated,
    Destroyed,
    /// Created and destroyed inside the range: the client never saw it.
    Gone,
}

impl Net {
    fn first(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Created => Self::Created,
            ChangeKind::Updated => Self::Updated,
            ChangeKind::CountsUpdated => Self::CountsUpdated,
            ChangeKind::Destroyed => Self::Destroyed,
        }
    }

    fn then(self, kind: ChangeKind) -> Self {
        use ChangeKind::{CountsUpdated, Created, Destroyed, Updated};
        match (self, kind) {
            (Self::Created | Self::Gone, Destroyed) => Self::Gone,
            (Self::Updated | Self::CountsUpdated | Self::Destroyed, Destroyed) => Self::Destroyed,
            // Ids are never reused, so "destroyed then created" can't happen;
            // if the log says so anyway, the last word wins.
            (Self::Gone | Self::Created, Created | Updated | CountsUpdated) => Self::Created,
            (Self::CountsUpdated, CountsUpdated) => Self::CountsUpdated,
            (Self::Updated | Self::CountsUpdated | Self::Destroyed, Created | Updated)
            | (Self::Updated | Self::Destroyed, CountsUpdated) => Self::Updated,
        }
    }
}

/// How many log rows to read per call: enough for `max_changes` objects in
/// the usual case; with heavy churn on few objects the answer just has
/// `has_more_changes` set. A state is never cut by this window: if it ends
/// in the middle of one, that state is left for the next call (or, when it
/// is the only one in the window, read whole).
const ROWS_PER_CALL: i64 = 10_000;

impl Store {
    /// The current state of a data type.
    ///
    /// # Errors
    /// On database errors.
    pub async fn state(&self, account: AccountId, data_type: DataType) -> Result<Seq, StoreError> {
        self.read(move |conn| {
            let seq: Option<i64> = states::table
                .find((account.get(), data_type.code()))
                .select(states::seq)
                .first(conn)
                .optional()?;
            seq.map_or(Ok(Seq::ZERO), Seq::from_db)
        })
        .await
    }

    /// Records changes as one new state and returns it. Recording nothing
    /// keeps the state.
    ///
    /// # Errors
    /// On database errors.
    pub async fn record_changes(
        &self,
        account: AccountId,
        data_type: DataType,
        list: &[Change],
    ) -> Result<Seq, StoreError> {
        let list = list.to_vec();
        self.write(move |conn| record_in(conn, account, data_type, &list))
            .await
    }

    /// What changed since `since`, at most `max_changes` objects (RFC 8620
    /// §5.2 "MUST ensure the number of ids returned … does not exceed this
    /// limit"). The changes of one state are never split: if the first one
    /// alone is over the limit, [`ChangesSince::TooMany`].
    ///
    /// `states.oldest` and the log rows are read in one read transaction (a
    /// snapshot in WAL mode): a trim running at the same time can't leave a
    /// replay that looks complete but misses the rows it just deleted.
    ///
    /// # Errors
    /// On database errors.
    pub async fn changes_since(
        &self,
        account: AccountId,
        data_type: DataType,
        since: Seq,
        max_changes: usize,
    ) -> Result<ChangesSince, StoreError> {
        self.changes_since_with(account, data_type, since, max_changes, ROWS_PER_CALL)
            .await
    }

    /// [`Store::changes_since`] reading at most about `rows_limit` log rows.
    async fn changes_since_with(
        &self,
        account: AccountId,
        data_type: DataType,
        since: Seq,
        max_changes: usize,
        rows_limit: i64,
    ) -> Result<ChangesSince, StoreError> {
        self.read(move |conn| {
            let row: Option<(i64, i64)> = states::table
                .find((account.get(), data_type.code()))
                .select((states::seq, states::oldest))
                .first(conn)
                .optional()?;
            let (current, oldest) = match row {
                Some((seq, oldest)) => (Seq::from_db(seq)?, Seq::from_db(oldest)?),
                None => (Seq::ZERO, Seq(1)),
            };
            // `since` must be a state we can still replay from: the change
            // right after it must still be in the log.
            if since > current || since.0.saturating_add(1) < oldest.0 {
                return Ok(ChangesSince::CannotCalculate);
            }

            let mut rows: Vec<(i64, i64, i32)> = changes::table
                .filter(changes::account_id.eq(account.get()))
                .filter(changes::data_type.eq(data_type.code()))
                .filter(changes::seq.gt(since.to_db()?))
                .order((changes::seq.asc(), changes::id.asc()))
                .select((changes::seq, changes::object_id, changes::kind))
                .limit(rows_limit)
                .load(conn)?;
            let window_full = i64::try_from(rows.len()).map_or(true, |n| n >= rows_limit);
            if window_full && let Some(&(last, ..)) = rows.last() {
                // The window may end in the middle of the last state: drop
                // it (the next call starts there), or read it whole when
                // it's the only one. One state is what one transaction
                // wrote, so that read is bounded.
                let first_of_last = rows.partition_point(|row| row.0 < last);
                if first_of_last > 0 {
                    rows.truncate(first_of_last);
                } else {
                    rows = changes::table
                        .filter(changes::account_id.eq(account.get()))
                        .filter(changes::data_type.eq(data_type.code()))
                        .filter(changes::seq.eq(last))
                        .order(changes::id.asc())
                        .select((changes::seq, changes::object_id, changes::kind))
                        .load(conn)?;
                }
            }
            summarize(&rows, since, current, max_changes.max(1), !window_full)
        })
        .await
    }

    /// The state `other` was at when `data_type` was at `since`: the newest
    /// `other` change logged before the first `data_type` change after
    /// `since`. One change set writes its rows in one transaction, `Email`
    /// before `Thread` (`mail/changeset.rs`), and row ids only grow, so the
    /// `other` changes after the returned state are those of the same
    /// change sets or later ones (later ones only add, which callers that
    /// over-report may take).
    ///
    /// `None` when that can't be told any more: the `other` log was trimmed
    /// past it. With no `data_type` change after `since`, the current
    /// `other` state. Read in one transaction, as [`Store::changes_since`].
    ///
    /// # Errors
    /// On database errors.
    pub async fn state_alongside(
        &self,
        account: AccountId,
        data_type: DataType,
        since: Seq,
        other: DataType,
    ) -> Result<Option<Seq>, StoreError> {
        self.read(move |conn| {
            let other_state: Option<(i64, i64)> = states::table
                .find((account.get(), other.code()))
                .select((states::seq, states::oldest))
                .first(conn)
                .optional()?;
            let (current, oldest) = match other_state {
                Some((seq, oldest)) => (Seq::from_db(seq)?, Seq::from_db(oldest)?),
                None => (Seq::ZERO, Seq(1)),
            };
            let first_after: Option<i64> = changes::table
                .filter(changes::account_id.eq(account.get()))
                .filter(changes::data_type.eq(data_type.code()))
                .filter(changes::seq.gt(since.to_db()?))
                .select(diesel::dsl::min(changes::id))
                .first(conn)?;
            let Some(first_after) = first_after else {
                return Ok(Some(current));
            };
            let before: Option<i64> = changes::table
                .filter(changes::account_id.eq(account.get()))
                .filter(changes::data_type.eq(other.code()))
                .filter(changes::id.lt(first_after))
                .select(diesel::dsl::max(changes::seq))
                .first(conn)?;
            match before {
                Some(seq) => Ok(Some(Seq::from_db(seq)?)),
                // Nothing logged before it: none ever was, or it was
                // trimmed (then the state can't be told).
                None if oldest <= Seq(1) => Ok(Some(Seq::ZERO)),
                None => Ok(None),
            }
        })
        .await
    }

    /// Trims the change log of every account and data type to the newest
    /// `keep_states` states and at most about `keep_rows` rows, without
    /// splitting a state. A client still at a forgotten state gets
    /// [`ChangesSince::CannotCalculate`] and reloads.
    ///
    /// `states.oldest` goes up first, in its own short write transaction, so
    /// clients stop replaying from the trimmed range at once; the rows then
    /// go in batches of `batch`, one write transaction each, so the write
    /// lock is never held for long. Only the (account, type) pairs over a
    /// limit are looked at: `states.log_rows` keeps the row count.
    ///
    /// # Errors
    /// On database errors.
    pub async fn trim_change_logs(&self, limits: ChangeRetention) -> Result<(), StoreError> {
        let keep_states = i64::try_from(limits.keep_states.max(1)).unwrap_or(i64::MAX);
        let keep_rows = i64::try_from(limits.keep_rows.max(1)).unwrap_or(i64::MAX);
        let batch = i64::try_from(limits.batch.max(1)).unwrap_or(i64::MAX);
        let over: Vec<(i64, i32)> = self
            .read(move |conn| {
                Ok(states::table
                    .filter(
                        states::log_rows
                            .gt(keep_rows)
                            .or((states::seq - states::oldest).ge(keep_states)),
                    )
                    .select((states::account_id, states::data_type))
                    .load(conn)?)
            })
            .await?;
        for (account, data_type) in over {
            let oldest = self
                .write(move |conn| raise_oldest(conn, (account, data_type), keep_states, keep_rows))
                .await?;
            let Some(oldest) = oldest else {
                continue;
            };
            loop {
                let deleted = self
                    .write(move |conn| delete_batch(conn, (account, data_type), oldest, batch))
                    .await?;
                if deleted < batch {
                    break;
                }
            }
        }
        Ok(())
    }
}

/// How much of the change log [`Store::trim_change_logs`] keeps, per account
/// and data type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeRetention {
    /// The newest states to keep.
    pub keep_states: u64,
    /// At most this many rows (one state can have thousands).
    pub keep_rows: u64,
    /// Rows deleted per write transaction.
    pub batch: u64,
}

impl Default for ChangeRetention {
    fn default() -> Self {
        Self {
            keep_states: 10_000,
            keep_rows: 100_000,
            batch: 5_000,
        }
    }
}

/// Moves `states.oldest` up to what the limits keep and returns it, or
/// `None` if nothing is to be trimmed.
fn raise_oldest(
    conn: &mut SqliteConnection,
    (account, data_type): (i64, i32),
    keep_states: i64,
    keep_rows: i64,
) -> Result<Option<i64>, StoreError> {
    let Some((seq, old_oldest, log_rows)): Option<(i64, i64, i64)> = states::table
        .find((account, data_type))
        .select((states::seq, states::oldest, states::log_rows))
        .first(conn)
        .optional()?
    else {
        return Ok(None);
    };
    let mut oldest = seq
        .saturating_sub(keep_states)
        .saturating_add(1)
        .max(old_oldest);
    if log_rows > keep_rows {
        // The state of the row just past the newest `keep_rows`: it and
        // everything older go, so no state is split.
        let cut: Option<i64> = changes::table
            .filter(changes::account_id.eq(account))
            .filter(changes::data_type.eq(data_type))
            .order(changes::seq.desc())
            .select(changes::seq)
            .offset(keep_rows)
            .first(conn)
            .optional()?;
        if let Some(cut) = cut {
            oldest = oldest.max(cut.saturating_add(1));
        }
    }
    if oldest > old_oldest {
        diesel::update(states::table.find((account, data_type)))
            .set(states::oldest.eq(oldest))
            .execute(conn)?;
    }
    // Rows below `oldest` may be left from an earlier, interrupted trim.
    Ok(Some(oldest))
}

/// Deletes up to `batch` log rows older than `oldest` and returns how many.
fn delete_batch(
    conn: &mut SqliteConnection,
    (account, data_type): (i64, i32),
    oldest: i64,
    batch: i64,
) -> Result<i64, StoreError> {
    let ids: Vec<i64> = changes::table
        .filter(changes::account_id.eq(account))
        .filter(changes::data_type.eq(data_type))
        .filter(changes::seq.lt(oldest))
        .select(changes::id)
        .limit(batch)
        .load(conn)?;
    if ids.is_empty() {
        return Ok(0);
    }
    let deleted = diesel::delete(changes::table.filter(changes::id.eq_any(&ids))).execute(conn)?;
    let deleted = i64::try_from(deleted).map_err(|_| StoreError::Corrupt("change count"))?;
    diesel::update(states::table.find((account, data_type)))
        .set(states::log_rows.eq(states::log_rows - deleted))
        .execute(conn)?;
    Ok(deleted)
}

/// Folds log rows (ordered by seq, every state whole) into a [`Changes`]
/// answer. `read_all`: the rows are every one after `since` (states that
/// left no rows, up to `current`, are reached too).
fn summarize(
    rows: &[(i64, i64, i32)],
    since: Seq,
    current: Seq,
    max_changes: usize,
    read_all: bool,
) -> Result<ChangesSince, StoreError> {
    let mut order: Vec<i64> = Vec::new();
    let mut net: HashMap<i64, Net> = HashMap::new();
    let mut reached = since;
    let mut stopped_early = false;

    // Rows of one seq are applied together: never split a state.
    for (index, group) in rows.chunk_by(|a, b| a.0 == b.0).enumerate() {
        let Some(&(seq, ..)) = group.first() else {
            continue;
        };
        // An upper bound of the ids returned (`Gone` ones are not), so the
        // answer never goes over `max_changes`.
        let new_objects = group.iter().filter(|row| !net.contains_key(&row.1)).count();
        if index > 0 && order.len().saturating_add(new_objects) > max_changes {
            stopped_early = true;
            break;
        }
        for &(_, object, kind) in group {
            let kind = ChangeKind::from_code(kind)?;
            match net.entry(object) {
                Entry::Occupied(mut entry) => {
                    let then = entry.get().then(kind);
                    entry.insert(then);
                }
                Entry::Vacant(entry) => {
                    entry.insert(Net::first(kind));
                    order.push(object);
                }
            }
        }
        if index == 0 {
            // The first state must fit by itself (exact count: objects
            // created and destroyed inside it are not returned).
            let returned = net.values().filter(|n| **n != Net::Gone).count();
            if returned > max_changes {
                return Ok(ChangesSince::TooMany);
            }
        }
        reached = Seq::from_db(seq)?;
    }

    let has_more_changes = stopped_early || (!read_all && reached < current);
    let new_state = if has_more_changes { reached } else { current };
    let mut changes = Changes {
        new_state,
        has_more_changes,
        ..Changes::default()
    };
    let mut only_counts = true;
    for object in order {
        match net.get(&object) {
            Some(Net::Created) => changes.created.push(object),
            Some(Net::Updated) => {
                only_counts = false;
                changes.updated.push(object);
            }
            Some(Net::CountsUpdated) => changes.updated.push(object),
            Some(Net::Destroyed) => changes.destroyed.push(object),
            Some(Net::Gone) | None => {}
        }
    }
    changes.only_counts_updated = only_counts && !changes.updated.is_empty();
    Ok(ChangesSince::Changes(changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::tests::alice;
    use crate::db::test_support::store;

    fn change(object: i64, kind: ChangeKind) -> Change {
        Change { object, kind }
    }

    fn unwrap(result: ChangesSince) -> Changes {
        match result {
            ChangesSince::Changes(changes) => changes,
            ChangesSince::CannotCalculate | ChangesSince::TooMany => panic!("cannot calculate"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn states_go_up_per_type() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        assert_eq!(
            store.state(account, DataType::Email).await.unwrap(),
            Seq::ZERO
        );
        let s1 = store
            .record_changes(account, DataType::Email, &[change(1, ChangeKind::Created)])
            .await
            .unwrap();
        assert_eq!(s1, Seq::new(1));
        // Nothing to record: same state.
        assert_eq!(
            store
                .record_changes(account, DataType::Email, &[])
                .await
                .unwrap(),
            s1
        );
        assert_eq!(
            store.state(account, DataType::Mailbox).await.unwrap(),
            Seq::ZERO
        );
        assert_eq!(store.state(account, DataType::Email).await.unwrap(), s1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn changes_are_folded_per_object() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Email;
        let s1 = store
            .record_changes(
                account,
                t,
                &[
                    change(1, ChangeKind::Created),
                    change(2, ChangeKind::Created),
                ],
            )
            .await
            .unwrap();
        store
            .record_changes(
                account,
                t,
                &[
                    change(1, ChangeKind::Updated),
                    change(3, ChangeKind::Created),
                ],
            )
            .await
            .unwrap();
        store
            .record_changes(
                account,
                t,
                &[
                    change(2, ChangeKind::Destroyed),
                    change(3, ChangeKind::Destroyed),
                ],
            )
            .await
            .unwrap();
        let s4 = store
            .record_changes(account, t, &[change(4, ChangeKind::Created)])
            .await
            .unwrap();

        let all = unwrap(
            store
                .changes_since(account, t, Seq::ZERO, 100)
                .await
                .unwrap(),
        );
        assert_eq!(all.created, vec![1, 4]);
        assert!(all.updated.is_empty() && all.destroyed.is_empty());
        assert_eq!((all.new_state, all.has_more_changes), (s4, false));

        let from_s1 = unwrap(store.changes_since(account, t, s1, 100).await.unwrap());
        assert_eq!(from_s1.created, vec![4]);
        assert_eq!(from_s1.updated, vec![1]);
        assert_eq!(from_s1.destroyed, vec![2]);

        let none = unwrap(store.changes_since(account, t, s4, 100).await.unwrap());
        assert_eq!(
            none,
            Changes {
                new_state: s4,
                ..Changes::default()
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn paging_never_splits_a_state() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Mailbox;
        let s1 = store
            .record_changes(
                account,
                t,
                &[
                    change(1, ChangeKind::Created),
                    change(2, ChangeKind::Created),
                ],
            )
            .await
            .unwrap();
        let s2 = store
            .record_changes(
                account,
                t,
                &[
                    change(3, ChangeKind::Created),
                    change(4, ChangeKind::Created),
                ],
            )
            .await
            .unwrap();

        let page = unwrap(store.changes_since(account, t, Seq::ZERO, 3).await.unwrap());
        assert_eq!(page.created, vec![1, 2]);
        assert_eq!((page.new_state, page.has_more_changes), (s1, true));
        let rest = unwrap(
            store
                .changes_since(account, t, page.new_state, 3)
                .await
                .unwrap(),
        );
        assert_eq!(rest.created, vec![3, 4]);
        assert_eq!((rest.new_state, rest.has_more_changes), (s2, false));

        // A single state bigger than the limit is never split, and never
        // returned over the limit (RFC 8620 §5.2).
        assert_eq!(
            store.changes_since(account, t, Seq::ZERO, 1).await.unwrap(),
            ChangesSince::TooMany
        );
        // Exactly the limit fits.
        let fits = unwrap(store.changes_since(account, t, Seq::ZERO, 2).await.unwrap());
        assert_eq!((fits.created, fits.new_state), (vec![1, 2], s1));
        // Objects created and destroyed inside the state are not counted.
        let s3 = store
            .record_changes(
                account,
                t,
                &[
                    change(5, ChangeKind::Created),
                    change(5, ChangeKind::Destroyed),
                    change(6, ChangeKind::Created),
                ],
            )
            .await
            .unwrap();
        let gone = unwrap(store.changes_since(account, t, s2, 1).await.unwrap());
        assert_eq!((gone.created, gone.new_state), (vec![6], s3));
    }

    fn retention(keep_states: u64, keep_rows: u64, batch: u64) -> ChangeRetention {
        ChangeRetention {
            keep_states,
            keep_rows,
            batch,
        }
    }

    async fn log_rows(store: &Store, account: AccountId, t: DataType) -> (i64, i64) {
        let code = t.code();
        store
            .read(move |conn| {
                let counted: i64 = changes::table
                    .filter(changes::account_id.eq(account.get()))
                    .filter(changes::data_type.eq(code))
                    .count()
                    .get_result(conn)?;
                let kept: i64 = states::table
                    .find((account.get(), code))
                    .select(states::log_rows)
                    .first(conn)?;
                Ok((counted, kept))
            })
            .await
            .unwrap()
    }

    /// One change set: an Email state, then a Thread state.
    async fn email_then_thread(store: &Store, account: AccountId, email: i64, thread: i64) {
        store
            .record_changes(
                account,
                DataType::Email,
                &[change(email, ChangeKind::Updated)],
            )
            .await
            .unwrap();
        store
            .record_changes(
                account,
                DataType::Thread,
                &[change(thread, ChangeKind::Updated)],
            )
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_thread_state_alongside_an_email_state() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let alongside = async |since: u64| {
            store
                .state_alongside(account, DataType::Email, Seq::new(since), DataType::Thread)
                .await
                .unwrap()
        };
        // Before anything: the start.
        assert_eq!(alongside(0).await, Some(Seq::ZERO));
        for n in 1..=4 {
            email_then_thread(&store, account, n, 100 + n).await;
        }
        // Email 2 was written with Thread 2: the threads changed after it
        // are those of Email states 3 and 4.
        assert_eq!(alongside(2).await, Some(Seq::new(2)));
        let threads = unwrap(
            store
                .changes_since(account, DataType::Thread, Seq::new(2), 10)
                .await
                .unwrap(),
        );
        assert_eq!(threads.updated, vec![103, 104]);
        assert_eq!(alongside(0).await, Some(Seq::ZERO));
        // Nothing after the newest: the current Thread state.
        assert_eq!(alongside(4).await, Some(Seq::new(4)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_trimmed_thread_log_is_not_guessed() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        for n in 1..=4 {
            email_then_thread(&store, account, n, 100 + n).await;
        }
        store
            .trim_change_logs(retention(2, 1000, 100))
            .await
            .unwrap();
        let alongside = async |since: u64| {
            store
                .state_alongside(account, DataType::Email, Seq::new(since), DataType::Thread)
                .await
                .unwrap()
        };
        // States 3 and 4 of each log are left: from Email 3, Thread 3 is
        // still there; from Email 2, the Thread rows before Email 3 are
        // gone, so it can't be told.
        assert_eq!(alongside(3).await, Some(Seq::new(3)));
        assert_eq!(alongside(2).await, None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_state_cut_by_the_row_limit_is_not_lost() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Email;
        let created = |objects: &[i64]| {
            objects
                .iter()
                .map(|&object| change(object, ChangeKind::Created))
                .collect::<Vec<_>>()
        };
        let s1 = store
            .record_changes(account, t, &created(&[1, 2, 3]))
            .await
            .unwrap();
        let s2 = store
            .record_changes(account, t, &created(&[4, 5]))
            .await
            .unwrap();

        // The 4-row window ends inside state 2: only state 1 comes.
        let first = unwrap(
            store
                .changes_since_with(account, t, Seq::ZERO, 100, 4)
                .await
                .unwrap(),
        );
        assert_eq!(first.created, vec![1, 2, 3]);
        assert_eq!((first.new_state, first.has_more_changes), (s1, true));
        let second = unwrap(
            store
                .changes_since_with(account, t, first.new_state, 100, 4)
                .await
                .unwrap(),
        );
        assert_eq!(second.created, vec![4, 5]);
        assert_eq!((second.new_state, second.has_more_changes), (s2, false));

        // A full window with several states, cut earlier by `max_changes`:
        // the answer ends at the last whole state it took.
        let s3 = store
            .record_changes(account, t, &created(&[6]))
            .await
            .unwrap();
        let s4 = store
            .record_changes(account, t, &created(&[7, 8]))
            .await
            .unwrap();
        let cut = unwrap(
            store
                .changes_since_with(account, t, s1, 3, 4)
                .await
                .unwrap(),
        );
        assert_eq!(cut.created, vec![4, 5, 6]);
        assert_eq!((cut.new_state, cut.has_more_changes), (s3, true));
        let rest = unwrap(
            store
                .changes_since_with(account, t, cut.new_state, 3, 4)
                .await
                .unwrap(),
        );
        assert_eq!(rest.created, vec![7, 8]);
        assert_eq!((rest.new_state, rest.has_more_changes), (s4, false));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_single_state_over_the_row_limit_comes_whole() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Email;
        let list: Vec<Change> = (1..=5)
            .map(|object| change(object, ChangeKind::Created))
            .collect();
        let s1 = store.record_changes(account, t, &list).await.unwrap();
        let all = unwrap(
            store
                .changes_since_with(account, t, Seq::ZERO, 100, 3)
                .await
                .unwrap(),
        );
        assert_eq!(all.created, vec![1, 2, 3, 4, 5]);
        assert_eq!((all.new_state, all.has_more_changes), (s1, false));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn trimmed_or_future_states_cannot_be_calculated() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Thread;
        for object in 1..=5 {
            store
                .record_changes(account, t, &[change(object, ChangeKind::Created)])
                .await
                .unwrap();
        }
        store
            .trim_change_logs(retention(2, 1000, 100))
            .await
            .unwrap();
        // Kept states 4 and 5: replaying from 3 still works, from 2 doesn't.
        let from3 = unwrap(
            store
                .changes_since(account, t, Seq::new(3), 10)
                .await
                .unwrap(),
        );
        assert_eq!(from3.created, vec![4, 5]);
        assert_eq!(
            store
                .changes_since(account, t, Seq::new(2), 10)
                .await
                .unwrap(),
            ChangesSince::CannotCalculate
        );
        assert_eq!(
            store
                .changes_since(account, t, Seq::new(9), 10)
                .await
                .unwrap(),
            ChangesSince::CannotCalculate
        );
        assert_eq!(log_rows(&store, account, t).await, (2, 2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_big_state_is_never_split_by_the_row_limit() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Email;
        // State 1: one row; state 2: four rows; state 3: one row.
        store
            .record_changes(account, t, &[change(1, ChangeKind::Created)])
            .await
            .unwrap();
        let big: Vec<Change> = (10..14).map(|o| change(o, ChangeKind::Created)).collect();
        store.record_changes(account, t, &big).await.unwrap();
        store
            .record_changes(account, t, &[change(2, ChangeKind::Created)])
            .await
            .unwrap();
        // Three rows kept would split state 2: it goes whole.
        store.trim_change_logs(retention(100, 3, 1)).await.unwrap();
        assert_eq!(log_rows(&store, account, t).await, (1, 1));
        assert_eq!(
            store
                .changes_since(account, t, Seq::new(1), 10)
                .await
                .unwrap(),
            ChangesSince::CannotCalculate
        );
        let from2 = unwrap(
            store
                .changes_since(account, t, Seq::new(2), 10)
                .await
                .unwrap(),
        );
        assert_eq!(from2.created, vec![2]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batches_give_the_same_result() {
        let (one, _dir1) = store().await;
        let (many, _dir2) = store().await;
        for store in [&one, &many] {
            let account = alice(store).await.id;
            for object in 1..=6 {
                store
                    .record_changes(
                        account,
                        DataType::Mailbox,
                        &[
                            change(object, ChangeKind::Created),
                            change(object + 100, ChangeKind::Created),
                        ],
                    )
                    .await
                    .unwrap();
            }
        }
        one.trim_change_logs(retention(3, 1000, 1)).await.unwrap();
        many.trim_change_logs(retention(3, 1000, 1000))
            .await
            .unwrap();
        for store in [&one, &many] {
            let account = alice(store).await.id;
            assert_eq!(log_rows(store, account, DataType::Mailbox).await, (6, 6));
            let from3 = unwrap(
                store
                    .changes_since(account, DataType::Mailbox, Seq::new(3), 100)
                    .await
                    .unwrap(),
            );
            assert_eq!(from3.created, vec![4, 104, 5, 105, 6, 106]);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_trim_between_pages_is_noticed() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Mailbox;
        for object in 1..=4 {
            store
                .record_changes(account, t, &[change(object, ChangeKind::Created)])
                .await
                .unwrap();
        }
        let page = unwrap(store.changes_since(account, t, Seq::ZERO, 1).await.unwrap());
        assert_eq!((page.new_state, page.has_more_changes), (Seq::new(1), true));
        store
            .trim_change_logs(retention(2, 1000, 100))
            .await
            .unwrap();
        // The next page would miss states 2: the client must reload instead
        // of getting a partial answer.
        assert_eq!(
            store
                .changes_since(account, t, page.new_state, 1)
                .await
                .unwrap(),
            ChangesSince::CannotCalculate
        );
        // A trim with nothing over the limits changes nothing.
        store
            .trim_change_logs(retention(2, 1000, 100))
            .await
            .unwrap();
        assert_eq!(log_rows(&store, account, t).await, (2, 2));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn email_delivery_keeps_no_rows() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let s1 = store
            .record_changes(
                account,
                DataType::EmailDelivery,
                &[change(7, ChangeKind::Created)],
            )
            .await
            .unwrap();
        assert_eq!(s1, Seq::new(1));
        assert_eq!(
            log_rows(&store, account, DataType::EmailDelivery).await,
            (0, 0)
        );
    }

    #[test]
    fn folding_rules() {
        use ChangeKind::{CountsUpdated, Created, Destroyed, Updated};
        assert_eq!(Net::first(Created).then(Updated), Net::Created);
        assert_eq!(Net::first(Created).then(Destroyed), Net::Gone);
        assert_eq!(Net::first(Updated).then(Updated), Net::Updated);
        assert_eq!(Net::first(Updated).then(Destroyed), Net::Destroyed);
        assert_eq!(
            Net::first(CountsUpdated).then(CountsUpdated),
            Net::CountsUpdated
        );
        assert_eq!(Net::first(CountsUpdated).then(Updated), Net::Updated);
        assert_eq!(Net::first(Updated).then(CountsUpdated), Net::Updated);
        assert_eq!(Net::first(Created).then(CountsUpdated), Net::Created);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn counts_only_updates_are_told_apart() {
        let (store, _dir) = store().await;
        let account = alice(&store).await.id;
        let t = DataType::Mailbox;
        let s1 = store
            .record_changes(account, t, &[change(1, ChangeKind::Created)])
            .await
            .unwrap();
        let s2 = store
            .record_changes(
                account,
                t,
                &[
                    change(1, ChangeKind::CountsUpdated),
                    change(2, ChangeKind::Created),
                ],
            )
            .await
            .unwrap();
        let counts = unwrap(store.changes_since(account, t, s1, 10).await.unwrap());
        assert_eq!(counts.updated, vec![1]);
        assert!(counts.only_counts_updated);
        store
            .record_changes(account, t, &[change(1, ChangeKind::Updated)])
            .await
            .unwrap();
        let renamed = unwrap(store.changes_since(account, t, s1, 10).await.unwrap());
        assert!(!renamed.only_counts_updated);
        let after = unwrap(store.changes_since(account, t, s2, 10).await.unwrap());
        assert!(!after.only_counts_updated);
        let none = unwrap(
            store
                .changes_since(account, t, Seq::ZERO, 10)
                .await
                .unwrap(),
        );
        assert!(none.updated.is_empty() && !none.only_counts_updated);
    }
}
