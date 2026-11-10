// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Which thread a new email joins.
//!
//! - With a server thread id (THREADID, X-GM-THRID): that thread.
//! - Without: the bridge links emails by Message-ID, In-Reply-To and
//!   References (`thread_refs`). An email that links two threads merges
//!   them: the oldest thread stays, the others are destroyed and their
//!   emails move (each one gets an Email `updated`, its `threadId`
//!   changed). The subject alone never joins threads: two different
//!   conversations called "Hola" stay apart. The headers alone don't
//!   either: a thread is only joined if one of its emails has the same
//!   base subject ([`sort_subject`]: without `Re:`, `Fwd:`, list tags…), as
//!   RFC 8621 §3 suggests, so a message that merely cites the ids of
//!   other conversations (or reuses one's Message-ID) can't fuse them. A
//!   reply whose subject was changed starts a new thread. Every email of
//!   the thread counts, also one gone from every mailbox but still kept
//!   (`orphaned_at`), as its references do.

use std::collections::BTreeSet;

use diesel::sql_types::{BigInt, Text};
use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _,
    SqliteConnection,
};

use super::changeset::{ChangeSet, Counts};
use super::convert::{id_list, sort_subject};
use super::types::{EmailMeta, ServerId};
use crate::schema::{emails, thread_refs, threads};
use crate::{AccountId, StoreError};

/// At most this many ids of one email are used for threading, so a hostile
/// `References` header can't make one insert write thousands of rows.
pub const MAX_THREAD_REFS: usize = 128;

/// The ids an email links to: its own Message-ID first, then In-Reply-To
/// and the newest References.
pub(crate) fn linked_ids(meta: &EmailMeta) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut ids = Vec::new();
    let own = meta.message_id.iter();
    let parents = meta.in_reply_to.iter();
    let references = meta.references.iter().rev();
    for id in own.chain(parents).chain(references) {
        if ids.len() >= MAX_THREAD_REFS {
            break;
        }
        if !id.is_empty() && seen.insert(id.as_str()) {
            ids.push(id.clone());
        }
    }
    ids
}

/// Finds or creates the thread for a new email, merging threads it links.
pub(crate) fn resolve(
    conn: &mut SqliteConnection,
    account: AccountId,
    changes: &mut ChangeSet,
    server_thread: Option<&ServerId>,
    meta: &EmailMeta,
) -> Result<i64, StoreError> {
    let thread = match server_thread {
        Some(server_id) => server_thread_row(conn, account, changes, server_id)?,
        None => local_thread(
            conn,
            account,
            changes,
            &linked_ids(meta),
            &sort_subject(meta.subject.as_deref()),
        )?,
    };
    changes.touch_thread(conn, thread)?;
    Ok(thread)
}

fn server_thread_row(
    conn: &mut SqliteConnection,
    account: AccountId,
    changes: &mut ChangeSet,
    server_id: &ServerId,
) -> Result<i64, StoreError> {
    let found: Option<i64> = threads::table
        .filter(threads::account_id.eq(account.get()))
        .filter(threads::server_id.eq(server_id.as_str()))
        .select(threads::id)
        .first(conn)
        .optional()?;
    if let Some(id) = found {
        return Ok(id);
    }
    let id = diesel::insert_into(threads::table)
        .values((
            threads::account_id.eq(account.get()),
            threads::server_id.eq(server_id.as_str()),
        ))
        .returning(threads::id)
        .get_result(conn)?;
    changes.new_thread(id);
    Ok(id)
}

#[derive(diesel::QueryableByName)]
struct ThreadRow {
    #[diesel(sql_type = BigInt)]
    thread_id: i64,
}

fn local_thread(
    conn: &mut SqliteConnection,
    account: AccountId,
    changes: &mut ChangeSet,
    ids: &[String],
    subject: &str,
) -> Result<i64, StoreError> {
    let json = serde_json::to_string(ids).map_err(|_| StoreError::Invalid("message ids"))?;
    let found: Vec<ThreadRow> = diesel::sql_query(
        "SELECT DISTINCT r.thread_id AS thread_id FROM thread_refs r WHERE r.account_id = ? \
         AND r.message_id IN (SELECT value FROM json_each(?)) \
         AND EXISTS (SELECT 1 FROM emails e WHERE e.thread_id = r.thread_id \
         AND e.sort_subject = ?) ORDER BY r.thread_id",
    )
    .bind::<BigInt, _>(account.get())
    .bind::<Text, _>(json)
    .bind::<Text, _>(subject)
    .load(conn)?;
    let mut found = found.into_iter().map(|row| row.thread_id);
    let thread = if let Some(oldest) = found.next() {
        let others: Vec<i64> = found.collect();
        if !others.is_empty() {
            merge(conn, changes, oldest, &others)?;
        }
        oldest
    } else {
        let id = diesel::insert_into(threads::table)
            .values(threads::account_id.eq(account.get()))
            .returning(threads::id)
            .get_result(conn)?;
        changes.new_thread(id);
        id
    };
    for id in ids {
        diesel::insert_into(thread_refs::table)
            .values((
                thread_refs::account_id.eq(account.get()),
                thread_refs::message_id.eq(id),
                thread_refs::thread_id.eq(thread),
            ))
            .on_conflict_do_nothing()
            .execute(conn)?;
    }
    Ok(thread)
}

/// Moves the emails and references of `others` into `into`; the change set
/// then sees `others` lose all their emails (destroyed) and `into` gain
/// them (updated).
fn merge(
    conn: &mut SqliteConnection,
    changes: &mut ChangeSet,
    into: i64,
    others: &[i64],
) -> Result<(), StoreError> {
    changes.touch_thread(conn, into)?;
    for &other in others {
        changes.touch_thread(conn, other)?;
    }
    let moved: Vec<i64> = emails::table
        .filter(emails::thread_id.eq_any(others))
        .select(emails::id)
        .load(conn)?;
    for &email in &moved {
        changes.touch_email(conn, email, Counts::Unchanged)?;
    }
    diesel::update(emails::table.filter(emails::thread_id.eq_any(others)))
        .set(emails::thread_id.eq(into))
        .execute(conn)?;
    diesel::update(thread_refs::table.filter(thread_refs::thread_id.eq_any(others)))
        .set(thread_refs::thread_id.eq(into))
        .execute(conn)?;
    tracing::debug!(
        threads = %id_list(others.iter().copied()),
        into,
        "merged threads"
    );
    Ok(())
}
