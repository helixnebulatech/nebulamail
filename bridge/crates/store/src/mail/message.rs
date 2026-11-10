// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Writing messages: new UIDs, flag changes, expunges, moves and copies.
//! Each call is one transaction that also records its JMAP changes.

use std::collections::BTreeSet;

use diesel::{
    ExpressionMethods as _, OptionalExtension as _, QueryDsl as _, RunQueryDsl as _,
    SqliteConnection,
};
use nebula_imap_proto::types::{ModSeq, Uid};
use nebula_jmap_proto::Keyword;

use super::changeset::{ChangeSet, Counts};
use super::convert::{
    addresses_to_db, email_id, mod_seq_from_db, mod_seq_to_db, opt, sort_from, sort_subject,
    strings_to_db, u64_to_db, uid_from_db, uid_to_db,
};
use super::mailbox::load_mailbox;
use super::thread;
use super::types::{FlagUpdate, NewMessage, Recorded, ServerId, StoredUid};
use crate::schema::{email_keywords, email_locations, emails, threads};
use crate::{AccountId, ChangeKind, EmailId, MailboxId, Store, StoreError, UnixTime};

/// What [`Store::update_flags`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlagsApplied {
    /// The states that moved.
    pub recorded: Recorded,
    /// UIDs the store doesn't know: fetch their metadata and add them.
    pub unknown: Vec<Uid>,
}

pub(crate) fn keywords_of(
    conn: &mut SqliteConnection,
    email: i64,
) -> Result<BTreeSet<Keyword>, StoreError> {
    let rows: Vec<String> = email_keywords::table
        .filter(email_keywords::email_id.eq(email))
        .select(email_keywords::keyword)
        .load(conn)?;
    rows.iter()
        .map(|text| text.parse().map_err(|_| StoreError::Corrupt("keyword")))
        .collect()
}

fn affects_counts(old: &BTreeSet<Keyword>, new: &BTreeSet<Keyword>) -> Counts {
    let counted = [Keyword::seen(), Keyword::draft()];
    if counted
        .iter()
        .any(|keyword| old.contains(keyword) != new.contains(keyword))
    {
        Counts::MayChange
    } else {
        Counts::Unchanged
    }
}

/// Replaces the keywords of an email if they differ.
fn set_keywords(
    conn: &mut SqliteConnection,
    changes: &mut ChangeSet,
    email: i64,
    keywords: &BTreeSet<Keyword>,
) -> Result<(), StoreError> {
    let old = keywords_of(conn, email)?;
    if old == *keywords {
        return Ok(());
    }
    changes.touch_email(conn, email, affects_counts(&old, keywords))?;
    diesel::delete(email_keywords::table.filter(email_keywords::email_id.eq(email)))
        .execute(conn)?;
    for keyword in keywords {
        diesel::insert_into(email_keywords::table)
            .values((
                email_keywords::email_id.eq(email),
                email_keywords::keyword.eq(keyword.as_str()),
            ))
            .execute(conn)?;
    }
    Ok(())
}

fn location(
    conn: &mut SqliteConnection,
    mailbox: i64,
    uid: Uid,
) -> Result<Option<(i64, Option<i64>)>, StoreError> {
    Ok(email_locations::table
        .find((mailbox, uid_to_db(uid)))
        .select((email_locations::email_id, email_locations::modseq))
        .first(conn)
        .optional()?)
}

fn insert_location(
    conn: &mut SqliteConnection,
    changes: &mut ChangeSet,
    email: i64,
    mailbox: i64,
    uid: Uid,
    mod_seq: Option<ModSeq>,
) -> Result<(), StoreError> {
    changes.touch_email(conn, email, Counts::MayChange)?;
    diesel::insert_into(email_locations::table)
        .values((
            email_locations::mailbox_id.eq(mailbox),
            email_locations::uid.eq(uid_to_db(uid)),
            email_locations::email_id.eq(email),
            email_locations::modseq.eq(opt(mod_seq, mod_seq_to_db)?),
        ))
        .execute(conn)?;
    changes.mailbox(mailbox, ChangeKind::CountsUpdated);
    Ok(())
}

fn add_one(
    conn: &mut SqliteConnection,
    account: AccountId,
    changes: &mut ChangeSet,
    mailbox: i64,
    message: &NewMessage,
) -> Result<(), StoreError> {
    if location(conn, mailbox, message.uid)?.is_some() {
        // Already known (a retried batch): only the flags may be new.
        let update = FlagUpdate {
            uid: message.uid,
            mod_seq: message.mod_seq,
            keywords: message.keywords.clone(),
        };
        apply_flags(conn, changes, mailbox, &update)?;
        return Ok(());
    }
    let existing: Option<i64> = match &message.server.email {
        Some(server_id) => emails::table
            .filter(emails::account_id.eq(account.get()))
            .filter(emails::server_id.eq(server_id.as_str()))
            .select(emails::id)
            .first(conn)
            .optional()?,
        None => None,
    };
    let email = if let Some(email) = existing {
        // Another copy of a known email (Gmail label, or it came back).
        if let Some(preview) = &message.meta.preview {
            diesel::update(emails::table.find(email).filter(emails::preview.is_null()))
                .set(emails::preview.eq(preview))
                .execute(conn)?;
        }
        email
    } else {
        let thread = thread::resolve(
            conn,
            account,
            changes,
            message.server.thread.as_ref(),
            &message.meta,
        )?;
        let email = insert_email(conn, account, thread, message)?;
        changes.new_email(email);
        email
    };
    insert_location(conn, changes, email, mailbox, message.uid, message.mod_seq)?;
    set_keywords(conn, changes, email, &message.keywords)
}

fn insert_email(
    conn: &mut SqliteConnection,
    account: AccountId,
    thread: i64,
    message: &NewMessage,
) -> Result<i64, StoreError> {
    let meta = &message.meta;
    let id = diesel::insert_into(emails::table)
        .values((
            (
                emails::account_id.eq(account.get()),
                emails::thread_id.eq(thread),
                emails::server_id.eq(message.server.email.as_ref().map(ServerId::as_str)),
                emails::message_id.eq(meta.message_id.as_deref()),
                emails::in_reply_to.eq(strings_to_db(&meta.in_reply_to)?),
                emails::references_ids.eq(strings_to_db(&meta.references)?),
                emails::subject.eq(meta.subject.as_deref()),
                emails::from_addresses.eq(addresses_to_db(meta.from.as_ref())?),
                emails::to_addresses.eq(addresses_to_db(meta.to.as_ref())?),
                emails::cc_addresses.eq(addresses_to_db(meta.cc.as_ref())?),
                emails::bcc_addresses.eq(addresses_to_db(meta.bcc.as_ref())?),
                emails::reply_to_addresses.eq(addresses_to_db(meta.reply_to.as_ref())?),
            ),
            (
                emails::sender_addresses.eq(addresses_to_db(meta.sender.as_ref())?),
                emails::sent_at.eq(meta.sent_at.map(|sent| sent.utc.secs())),
                emails::sent_offset.eq(meta.sent_at.map(|sent| i32::from(sent.offset_minutes))),
                emails::received_at.eq(meta.received_at.secs()),
                emails::size.eq(u64_to_db(meta.size, "size")?),
                emails::has_attachment.eq(meta.has_attachment),
                emails::preview.eq(meta.preview.as_deref()),
                emails::headers.eq(&meta.headers),
                emails::sort_subject.eq(sort_subject(meta.subject.as_deref())),
                emails::sort_from.eq(sort_from(meta.from.as_ref())),
            ),
        ))
        .returning(emails::id)
        .get_result(conn)?;
    Ok(id)
}

/// Applies new flags to a known location. Returns `false` if the UID is
/// unknown.
fn apply_flags(
    conn: &mut SqliteConnection,
    changes: &mut ChangeSet,
    mailbox: i64,
    update: &FlagUpdate,
) -> Result<bool, StoreError> {
    let Some((email, stored)) = location(conn, mailbox, update.uid)? else {
        return Ok(false);
    };
    // No "older MODSEQ loses" rule: servers don't all keep per-message
    // MODSEQs consistent with HIGHESTMODSEQ (Stalwart 0.16 reports a FETCH
    // MODSEQ above the mailbox's HIGHESTMODSEQ). A stale update from a
    // sync racing our own STORE is fixed by the next sync, which starts
    // from the HIGHESTMODSEQ seen before the race.
    let stored = opt(stored, mod_seq_from_db)?;
    if update.mod_seq.is_some() && update.mod_seq != stored {
        diesel::update(email_locations::table.find((mailbox, uid_to_db(update.uid))))
            .set(email_locations::modseq.eq(opt(update.mod_seq, mod_seq_to_db)?))
            .execute(conn)?;
    }
    set_keywords(conn, changes, email, &update.keywords)?;
    Ok(true)
}

fn remove_location(
    conn: &mut SqliteConnection,
    changes: &mut ChangeSet,
    mailbox: i64,
    uid: Uid,
) -> Result<(), StoreError> {
    let Some((email, _)) = location(conn, mailbox, uid)? else {
        return Ok(());
    };
    changes.touch_email(conn, email, Counts::MayChange)?;
    diesel::delete(email_locations::table.find((mailbox, uid_to_db(uid)))).execute(conn)?;
    changes.mailbox(mailbox, ChangeKind::CountsUpdated);
    Ok(())
}

impl Store {
    /// Adds messages found in a mailbox. A message with a known server id
    /// becomes one more mailbox of that email; otherwise it is a new email
    /// in a new or existing thread. Known UIDs only get their flags updated,
    /// so a retried batch is harmless.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn add_messages(
        &self,
        account: AccountId,
        mailbox: MailboxId,
        messages: Vec<NewMessage>,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            load_mailbox(conn, account, mailbox)?;
            let mut changes = ChangeSet::new(account, now);
            for message in &messages {
                add_one(conn, account, &mut changes, mailbox.get(), message)?;
            }
            changes.commit(conn)
        })
        .await
    }

    /// Applies flag changes reported by the server.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn update_flags(
        &self,
        account: AccountId,
        mailbox: MailboxId,
        updates: Vec<FlagUpdate>,
        now: UnixTime,
    ) -> Result<FlagsApplied, StoreError> {
        self.write(move |conn| {
            load_mailbox(conn, account, mailbox)?;
            let mut changes = ChangeSet::new(account, now);
            let mut unknown = Vec::new();
            for update in &updates {
                if !apply_flags(conn, &mut changes, mailbox.get(), update)? {
                    unknown.push(update.uid);
                }
            }
            Ok(FlagsApplied {
                recorded: changes.commit(conn)?,
                unknown,
            })
        })
        .await
    }

    /// Removes copies expunged on the server (or marked `\Deleted`).
    /// Unknown UIDs are ignored.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn remove_messages(
        &self,
        account: AccountId,
        mailbox: MailboxId,
        uids: Vec<Uid>,
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            load_mailbox(conn, account, mailbox)?;
            let mut changes = ChangeSet::new(account, now);
            for uid in uids {
                remove_location(conn, &mut changes, mailbox.get(), uid)?;
            }
            changes.commit(conn)
        })
        .await
    }

    /// After `UID MOVE` with `COPYUID`: the copy changes mailbox and UID but
    /// stays the same email (same id).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if a mailbox isn't in this account or the
    /// source UID is unknown.
    pub async fn move_message(
        &self,
        account: AccountId,
        from: (MailboxId, Uid),
        to: (MailboxId, Uid),
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            load_mailbox(conn, account, from.0)?;
            load_mailbox(conn, account, to.0)?;
            let (email, _) =
                location(conn, from.0.get(), from.1)?.ok_or(StoreError::NotFound("message"))?;
            let mut changes = ChangeSet::new(account, now);
            remove_location(conn, &mut changes, from.0.get(), from.1)?;
            if location(conn, to.0.get(), to.1)?.is_none() {
                insert_location(conn, &mut changes, email, to.0.get(), to.1, None)?;
            }
            changes.commit(conn)
        })
        .await
    }

    /// After `UID COPY` with `COPYUID`: the email is now in one more
    /// mailbox.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account or the
    /// email isn't visible.
    pub async fn add_location(
        &self,
        account: AccountId,
        email: EmailId,
        to: (MailboxId, Uid),
        now: UnixTime,
    ) -> Result<Recorded, StoreError> {
        self.write(move |conn| {
            load_mailbox(conn, account, to.0)?;
            check_email(conn, account, email)?;
            let mut changes = ChangeSet::new(account, now);
            if location(conn, to.0.get(), to.1)?.is_none() {
                insert_location(conn, &mut changes, email.get(), to.0.get(), to.1, None)?;
            }
            changes.commit(conn)
        })
        .await
    }

    /// The copies stored for a mailbox, by UID.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn mailbox_uids(
        &self,
        account: AccountId,
        mailbox: MailboxId,
    ) -> Result<Vec<StoredUid>, StoreError> {
        self.read(move |conn| {
            load_mailbox(conn, account, mailbox)?;
            let rows: Vec<(i64, i64, Option<i64>)> = email_locations::table
                .filter(email_locations::mailbox_id.eq(mailbox.get()))
                .order(email_locations::uid.asc())
                .select((
                    email_locations::uid,
                    email_locations::email_id,
                    email_locations::modseq,
                ))
                .load(conn)?;
            rows.into_iter()
                .map(|(uid, email, mod_seq)| {
                    Ok(StoredUid {
                        uid: uid_from_db(uid)?,
                        email: email_id(email)?,
                        mod_seq: opt(mod_seq, mod_seq_from_db)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// The email stored at a UID, if any.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn email_at(
        &self,
        account: AccountId,
        mailbox: MailboxId,
        uid: Uid,
    ) -> Result<Option<EmailId>, StoreError> {
        self.read(move |conn| {
            load_mailbox(conn, account, mailbox)?;
            location(conn, mailbox.get(), uid)?
                .map(|(email, _)| email_id(email))
                .transpose()
        })
        .await
    }

    /// Visible emails of a mailbox with this Message-ID (to find a message
    /// appended without UIDPLUS).
    ///
    /// # Errors
    /// [`StoreError::NotFound`] if the mailbox isn't in this account.
    pub async fn emails_by_message_id(
        &self,
        account: AccountId,
        mailbox: MailboxId,
        message_id: String,
    ) -> Result<Vec<EmailId>, StoreError> {
        self.read(move |conn| {
            load_mailbox(conn, account, mailbox)?;
            let rows: Vec<i64> = email_locations::table
                .inner_join(emails::table)
                .filter(email_locations::mailbox_id.eq(mailbox.get()))
                .filter(emails::message_id.eq(message_id))
                .select(emails::id)
                .distinct()
                .load(conn)?;
            rows.into_iter().map(email_id).collect()
        })
        .await
    }

    /// Fills in the preview of an email that has none (computed from the
    /// body when it was first fetched). Not a JMAP change: the preview is
    /// an immutable property that was just unknown before.
    ///
    /// # Errors
    /// On database errors.
    pub async fn set_preview(
        &self,
        account: AccountId,
        email: EmailId,
        preview: String,
    ) -> Result<(), StoreError> {
        self.write(move |conn| {
            diesel::update(
                emails::table
                    .find(email.get())
                    .filter(emails::account_id.eq(account.get()))
                    .filter(emails::preview.is_null()),
            )
            .set(emails::preview.eq(preview))
            .execute(conn)?;
            Ok(())
        })
        .await
    }

    /// Deletes emails hidden since before `before` (no location left; kept
    /// for their server id) and threads left without emails. Their
    /// `destroyed` changes were recorded when they were hidden.
    ///
    /// # Errors
    /// On database errors.
    pub async fn purge_orphans(
        &self,
        account: AccountId,
        before: UnixTime,
    ) -> Result<usize, StoreError> {
        self.write(move |conn| {
            let purged = diesel::delete(
                emails::table
                    .filter(emails::account_id.eq(account.get()))
                    .filter(emails::orphaned_at.lt(before.secs())),
            )
            .execute(conn)?;
            diesel::delete(
                threads::table
                    .filter(threads::account_id.eq(account.get()))
                    .filter(diesel::dsl::not(diesel::dsl::exists(
                        emails::table.filter(emails::thread_id.eq(threads::id)),
                    ))),
            )
            .execute(conn)?;
            Ok(purged)
        })
        .await
    }
}

/// Fails unless the email is visible in this account.
pub(crate) fn check_email(
    conn: &mut SqliteConnection,
    account: AccountId,
    email: EmailId,
) -> Result<(), StoreError> {
    let found: Option<i64> = emails::table
        .find(email.get())
        .filter(emails::account_id.eq(account.get()))
        .filter(diesel::dsl::exists(
            email_locations::table.filter(email_locations::email_id.eq(emails::id)),
        ))
        .select(emails::id)
        .first(conn)
        .optional()?;
    found.map(|_| ()).ok_or(StoreError::NotFound("email"))
}
