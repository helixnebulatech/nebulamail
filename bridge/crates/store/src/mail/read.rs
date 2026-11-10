// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Reading emails, threads and locations for the JMAP layer. Only visible
//! emails (with at least one location) and threads are returned.

use std::collections::{BTreeMap, BTreeSet};

use diesel::sql_types::{BigInt, Text};
use diesel::{
    ExpressionMethods as _, QueryDsl as _, Queryable, RunQueryDsl as _, Selectable,
    SelectableHelper as _, SqliteConnection,
};
use nebula_imap_proto::types::Uid;

use super::convert::{
    addresses_from_db, email_id, id_list, mailbox_id, mod_seq_from_db, name_from_db, opt,
    strings_from_db, thread_id, u64_from_db, uid_from_db, uid_to_db, validity_from_db,
};
use super::mailbox::load_mailbox;
use super::message::keywords_of;
use super::types::{Email, EmailMeta, Location, SentAt, ServerId};
use crate::schema::{email_locations, emails, mailboxes};
use crate::{AccountId, EmailId, MailboxId, Store, StoreError, ThreadId, UnixTime};

#[derive(Queryable, Selectable)]
#[diesel(table_name = emails, check_for_backend(diesel::sqlite::Sqlite))]
struct EmailRow {
    id: i64,
    thread_id: i64,
    server_id: Option<String>,
    message_id: Option<String>,
    in_reply_to: String,
    references_ids: String,
    subject: Option<String>,
    from_addresses: Option<String>,
    to_addresses: Option<String>,
    cc_addresses: Option<String>,
    bcc_addresses: Option<String>,
    reply_to_addresses: Option<String>,
    sender_addresses: Option<String>,
    sent_at: Option<i64>,
    sent_offset: Option<i32>,
    received_at: i64,
    size: i64,
    has_attachment: bool,
    preview: Option<String>,
    headers: Vec<u8>,
}

impl EmailRow {
    fn meta(&self) -> Result<EmailMeta, StoreError> {
        let sent_at = match (self.sent_at, self.sent_offset) {
            (Some(secs), Some(offset)) => Some(SentAt {
                utc: UnixTime::from_secs(secs),
                offset_minutes: i16::try_from(offset)
                    .map_err(|_| StoreError::Corrupt("date offset"))?,
            }),
            _ => None,
        };
        Ok(EmailMeta {
            message_id: self.message_id.clone(),
            in_reply_to: strings_from_db(&self.in_reply_to)?,
            references: strings_from_db(&self.references_ids)?,
            subject: self.subject.clone(),
            from: addresses_from_db(self.from_addresses.as_deref())?,
            to: addresses_from_db(self.to_addresses.as_deref())?,
            cc: addresses_from_db(self.cc_addresses.as_deref())?,
            bcc: addresses_from_db(self.bcc_addresses.as_deref())?,
            reply_to: addresses_from_db(self.reply_to_addresses.as_deref())?,
            sender: addresses_from_db(self.sender_addresses.as_deref())?,
            sent_at,
            received_at: UnixTime::from_secs(self.received_at),
            size: u64_from_db(self.size, "size")?,
            has_attachment: self.has_attachment,
            preview: self.preview.clone(),
            headers: self.headers.clone(),
        })
    }
}

fn load_emails(
    conn: &mut SqliteConnection,
    account: AccountId,
    ids: &[EmailId],
) -> Result<Vec<Email>, StoreError> {
    let raw: Vec<i64> = ids.iter().map(|id| id.get()).collect();
    let mut rows: Vec<EmailRow> = Vec::new();
    // In chunks: SQLite limits the number of bound values.
    for chunk in raw.chunks(500) {
        rows.extend(
            emails::table
                .filter(emails::account_id.eq(account.get()))
                .filter(emails::id.eq_any(chunk))
                .select(EmailRow::as_select())
                .load(conn)?,
        );
    }
    let mut by_id = BTreeMap::new();
    for row in rows {
        let mailboxes: Vec<i64> = email_locations::table
            .filter(email_locations::email_id.eq(row.id))
            .select(email_locations::mailbox_id)
            .distinct()
            .load(conn)?;
        if mailboxes.is_empty() {
            continue;
        }
        let email = Email {
            id: email_id(row.id)?,
            thread: thread_id(row.thread_id)?,
            server_id: row.server_id.as_deref().map(ServerId::new).transpose()?,
            mailboxes: mailboxes
                .into_iter()
                .map(mailbox_id)
                .collect::<Result<_, _>>()?,
            keywords: keywords_of(conn, row.id)?,
            meta: row.meta()?,
        };
        by_id.insert(row.id, email);
    }
    // In the order asked for, without duplicates.
    let mut seen = BTreeSet::new();
    Ok(raw
        .iter()
        .filter(|id| seen.insert(**id))
        .filter_map(|id| by_id.remove(id))
        .collect())
}

/// mailbox, IMAP name, UIDVALIDITY, UID, MODSEQ.
type LocationRow = (i64, String, Option<i64>, i64, Option<i64>);

#[derive(diesel::QueryableByName)]
struct IdRow {
    #[diesel(sql_type = BigInt)]
    id: i64,
}

#[derive(diesel::QueryableByName)]
struct ThreadEmailRow {
    #[diesel(sql_type = BigInt)]
    thread_id: i64,
    #[diesel(sql_type = BigInt)]
    id: i64,
}

impl Store {
    /// Visible emails by id, in the order given; unknown, hidden or other
    /// accounts' ids are left out (JMAP `notFound`).
    ///
    /// # Errors
    /// On database errors.
    pub async fn emails(
        &self,
        account: AccountId,
        ids: Vec<EmailId>,
    ) -> Result<Vec<Email>, StoreError> {
        self.read(move |conn| load_emails(conn, account, &ids))
            .await
    }

    /// The visible emails of each thread, oldest `receivedAt` first (RFC
    /// 8621 §3). Threads without visible emails are left out.
    ///
    /// # Errors
    /// On database errors.
    pub async fn thread_emails(
        &self,
        account: AccountId,
        threads: Vec<ThreadId>,
    ) -> Result<Vec<(ThreadId, Vec<EmailId>)>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<ThreadEmailRow> = diesel::sql_query(
                "SELECT e.thread_id AS thread_id, e.id AS id FROM emails e \
                 WHERE e.account_id = ? AND e.thread_id IN (SELECT value FROM json_each(?)) \
                 AND EXISTS (SELECT 1 FROM email_locations l WHERE l.email_id = e.id) \
                 ORDER BY e.thread_id, e.received_at ASC, e.id ASC",
            )
            .bind::<BigInt, _>(account.get())
            .bind::<Text, _>(id_list(threads.iter().map(|id| id.get())))
            .load(conn)?;
            let mut map: BTreeMap<i64, Vec<EmailId>> = BTreeMap::new();
            for row in rows {
                map.entry(row.thread_id)
                    .or_default()
                    .push(email_id(row.id)?);
            }
            let mut seen = BTreeSet::new();
            threads
                .iter()
                .filter(|id| seen.insert(**id))
                .filter_map(|id| map.remove(&id.get()).map(|list| (*id, list)))
                .map(Ok)
                .collect()
        })
        .await
    }

    /// The visible threads of the account, by id, at most `limit` (for
    /// `Thread/get` with `ids: null`). `None` if there are more.
    ///
    /// # Errors
    /// On database errors.
    pub async fn thread_ids(
        &self,
        account: AccountId,
        limit: usize,
    ) -> Result<Option<Vec<ThreadId>>, StoreError> {
        self.read(move |conn| {
            // One more than asked, to know whether there are more.
            let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
            let rows: Vec<IdRow> = diesel::sql_query(
                "SELECT DISTINCT e.thread_id AS id FROM emails e WHERE e.account_id = ? \
                 AND EXISTS (SELECT 1 FROM email_locations l WHERE l.email_id = e.id) \
                 ORDER BY e.thread_id LIMIT ?",
            )
            .bind::<BigInt, _>(account.get())
            .bind::<BigInt, _>(fetch)
            .load(conn)?;
            if rows.len() > limit {
                return Ok(None);
            }
            rows.into_iter()
                .map(|row| thread_id(row.id))
                .collect::<Result<Vec<_>, _>>()
                .map(Some)
        })
        .await
    }

    /// Where the copies of an email are, to act on them over IMAP.
    ///
    /// # Errors
    /// On database errors. An unknown email has no locations.
    pub async fn locations(
        &self,
        account: AccountId,
        email: EmailId,
    ) -> Result<Vec<Location>, StoreError> {
        self.read(move |conn| {
            let rows: Vec<LocationRow> = email_locations::table
                .inner_join(mailboxes::table)
                .filter(email_locations::email_id.eq(email.get()))
                .filter(mailboxes::account_id.eq(account.get()))
                .order((
                    email_locations::mailbox_id.asc(),
                    email_locations::uid.asc(),
                ))
                .select((
                    email_locations::mailbox_id,
                    mailboxes::imap_name,
                    mailboxes::uid_validity,
                    email_locations::uid,
                    email_locations::modseq,
                ))
                .load(conn)?;
            rows.into_iter()
                .map(|(mailbox, name, validity, uid, mod_seq)| {
                    Ok(Location {
                        mailbox: mailbox_id(mailbox)?,
                        imap_name: name_from_db(&name)?,
                        uid_validity: opt(validity, validity_from_db)?,
                        uid: uid_from_db(uid)?,
                        mod_seq: opt(mod_seq, mod_seq_from_db)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// The emails at these UIDs of a mailbox (for IMAP SEARCH results).
    /// Unknown UIDs are skipped.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn emails_at(
        &self,
        account: AccountId,
        mailbox: MailboxId,
        uids: Vec<Uid>,
    ) -> Result<Vec<EmailId>, StoreError> {
        self.read(move |conn| {
            load_mailbox(conn, account, mailbox)?;
            let mut found = BTreeSet::new();
            // In chunks: SQLite limits the number of bound values.
            for chunk in uids.chunks(500) {
                let raw: Vec<i64> = chunk.iter().copied().map(uid_to_db).collect();
                let rows: Vec<i64> = email_locations::table
                    .filter(email_locations::mailbox_id.eq(mailbox.get()))
                    .filter(email_locations::uid.eq_any(raw))
                    .select(email_locations::email_id)
                    .load(conn)?;
                for row in rows {
                    found.insert(email_id(row)?);
                }
            }
            Ok(found.into_iter().collect())
        })
        .await
    }
}
