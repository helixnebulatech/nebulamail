// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Adding a raw message to a mailbox (drafts, `Email/import`, the copy in
//! Sent) and finding the email it became.
//!
//! With UIDPLUS the server answers `APPENDUID` and we fetch that UID's
//! metadata at once. Without it, the mailbox is synced and the new email is
//! found by its Message-ID (the newest match).
//!
//! Several messages for one mailbox go in one command with MULTIAPPEND
//! (RFC 3502), in batches of at most [`MAX_BATCH_BYTES`] (the command is
//! built in memory). The server adds a batch all or none, so if it refuses
//! one we append that batch's messages one by one to tell which one it
//! doesn't want; the batches already added stay as they are.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::Arc;

use nebula_imap::SelectOptions;
use nebula_imap::proto::command::AppendMessage;
use nebula_imap::proto::types::Capability;
use nebula_imap::proto::types::{Flag, InternalDate, Uid, UidValidity};
use nebula_jmap_proto::Keyword;
use nebula_store::{EmailId, Mailbox, MailboxId, MailboxSyncState, UnixTime};

use crate::MailError;
use crate::core::account::MailAccount;
use crate::core::slot::Reselect;
use crate::model::headers;
use crate::model::keywords::to_flag;
use crate::sync::mailbox::fetch_metadata;

/// Whether an appended message is known to be a new email.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOrigin {
    /// The server gave a UID the store didn't have, and the email has no
    /// other copy: the APPEND made it, and nothing else of the user's is
    /// joined to it.
    New,
    /// Maybe an email that was already there: a server that deduplicates
    /// (Gmail, by Message-ID) answers with it, or the store joined the copy
    /// to an existing email (same EMAILID/OBJECTID/X-GM-MSGID), or the
    /// message was found by Message-ID (no APPENDUID).
    MaybeExisting,
}

/// The email an APPEND became.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    /// The email.
    pub email: EmailId,
    /// Whether it is known to be new (see [`MailAccount::discard_appended`]).
    pub origin: AppendOrigin,
}

impl Appended {
    const fn maybe_existing(email: EmailId) -> Self {
        Self {
            email,
            origin: AppendOrigin::MaybeExisting,
        }
    }
}

/// One message for [`MailAccount::append_many`].
#[derive(Debug, Clone, Copy)]
pub struct AppendItem<'a> {
    /// The RFC 5322 message.
    pub raw: &'a [u8],
    /// Its keywords.
    pub keywords: &'a BTreeSet<Keyword>,
    /// Receive time (now if `None`).
    pub received_at: Option<UnixTime>,
}

/// Most message bytes in one MULTIAPPEND (a bigger message goes alone).
const MAX_BATCH_BYTES: usize = 16 * 1024 * 1024;

/// Splits messages of these sizes, in order, into batches of at most `max`
/// bytes each; a message bigger than `max` is a batch by itself.
fn batches(sizes: &[usize], max: usize) -> Vec<Range<usize>> {
    let mut list = Vec::new();
    let (mut start, mut bytes) = (0, 0_usize);
    for (index, size) in sizes.iter().enumerate() {
        if index > start && bytes.saturating_add(*size) > max {
            list.push(start..index);
            start = index;
            bytes = 0;
        }
        bytes = bytes.saturating_add(*size);
    }
    if start < sizes.len() {
        list.push(start..sizes.len());
    }
    list
}

/// Whether a refused batch is worth sending again one message at a time:
/// the server said NO/BAD, or one message can't be encoded (that fails
/// before anything is written, so the connection is fine).
fn batch_falls_back(error: &MailError) -> bool {
    matches!(error, MailError::Refused(_) | MailError::Invalid(_))
}

/// Flags and date of an item, checked.
/// (The error is what [`MailError::Invalid`] will say.)
type Prepared = Result<(Vec<Flag>, Option<InternalDate>), &'static str>;

fn prepare(item: &AppendItem<'_>) -> Prepared {
    let flags: Vec<Flag> = item
        .keywords
        .iter()
        .map(|keyword| to_flag(keyword).ok_or("keyword"))
        .collect::<Result<_, _>>()?;
    let date = item
        .received_at
        .map(|time| InternalDate::from_unix_timestamp(time.secs()))
        .transpose()
        .map_err(|_| "receivedAt")?;
    Ok((flags, date))
}

impl MailAccount {
    /// Appends several messages to one mailbox, in one command when the
    /// server has MULTIAPPEND. One result per item, in order.
    ///
    /// # Errors
    /// The outer error: unknown or unselectable mailbox, or the connection
    /// failed during the batch. Per item: as [`Self::append`].
    pub async fn append_many(
        &self,
        mailbox: MailboxId,
        items: &[AppendItem<'_>],
    ) -> Result<Vec<Result<Appended, MailError>>, MailError> {
        let target = self.store().mailbox(self.id, mailbox).await?;
        if !target.selectable {
            return Err(MailError::Invalid("mailbox can't hold messages"));
        }
        let prepared: Vec<_> = items.iter().map(prepare).collect();
        let ready: Vec<usize> = prepared
            .iter()
            .enumerate()
            .filter_map(|(index, p)| p.is_ok().then_some(index))
            .collect();
        let mut results: Vec<Option<Result<Appended, MailError>>> = prepared
            .iter()
            .map(|p| p.as_ref().err().map(|what| Err(MailError::Invalid(what))))
            .collect();

        let sizes: Vec<usize> = ready
            .iter()
            .map(|index| items.get(*index).map_or(0, |item| item.raw.len()))
            .collect();
        // Each batch falls back on its own: going one by one over the whole
        // list after a later batch fails would add the earlier ones twice.
        for range in batches(&sizes, MAX_BATCH_BYTES) {
            let indices = ready.get(range).unwrap_or_default();
            let batch = if indices.len() > 1 {
                self.append_batch(&target, items, &prepared, indices)
                    .await?
            } else {
                None
            };
            match batch {
                Some(emails) => {
                    for (index, email) in indices.iter().zip(emails) {
                        if let Some(slot) = results.get_mut(*index) {
                            *slot = Some(email);
                        }
                    }
                }
                None => {
                    for index in indices {
                        let (Some(item), Some(slot)) = (items.get(*index), results.get_mut(*index))
                        else {
                            continue;
                        };
                        *slot = Some(
                            self.append(mailbox, item.raw, item.keywords, item.received_at)
                                .await,
                        );
                    }
                }
            }
        }
        Ok(results
            .into_iter()
            .map(|result| result.unwrap_or(Err(MailError::Invalid("message"))))
            .collect())
    }

    /// One MULTIAPPEND of the `ready` items (one batch). `None` if the
    /// server can't do it or refused the batch (the caller goes one by one
    /// over this batch).
    async fn append_batch(
        &self,
        target: &Mailbox,
        items: &[AppendItem<'_>],
        prepared: &[Prepared],
        ready: &[usize],
    ) -> Result<Option<Vec<Result<Appended, MailError>>>, MailError> {
        let mut messages = Vec::with_capacity(ready.len());
        let mut raws = Vec::with_capacity(ready.len());
        for index in ready {
            let (Some(item), Some(Ok((flags, date)))) = (items.get(*index), prepared.get(*index))
            else {
                return Ok(None);
            };
            messages.push(AppendMessage {
                flags,
                internal_date: *date,
                message: item.raw,
            });
            raws.push(item.raw);
        }
        let mut slot = self.slot().await;
        let session = slot.session(self).await?;
        if !session.capabilities().has(&Capability::MultiAppend) {
            return Ok(None);
        }
        let result = session.append_many(&target.imap_name, &messages).await;
        let appended = match slot.check(result) {
            Ok(appended) => appended,
            Err(error) if batch_falls_back(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        drop(slot);
        let mut emails = Vec::with_capacity(raws.len());
        for (position, raw) in raws.iter().enumerate() {
            let uid = appended
                .uids
                .as_ref()
                .and_then(|(validity, uids)| Some((*validity, *uids.get(position)?)));
            let adopted = match uid {
                Some((validity, uid)) => self.adopt_appended(target, validity, uid).await,
                None => Ok(None),
            };
            let email = match adopted {
                Ok(Some(email)) => Ok(email),
                Ok(None) => self
                    .find_appended(target, raw)
                    .await
                    .map(Appended::maybe_existing),
                Err(error) => Err(error),
            };
            if let Ok(appended) = &email {
                self.shared
                    .cache
                    .insert(self.id, appended.email, Arc::from(*raw));
            }
            emails.push(email);
        }
        Ok(Some(emails))
    }

    /// Appends a message to a mailbox with these keywords and receive time
    /// (now if `None`) and returns the new email.
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an unknown mailbox, [`MailError::Refused`]
    /// if the server rejects the message (too big, over quota…),
    /// [`MailError::Invalid`] for a bad keyword or date, and connection
    /// errors.
    pub async fn append(
        &self,
        mailbox: MailboxId,
        raw: &[u8],
        keywords: &BTreeSet<Keyword>,
        received_at: Option<UnixTime>,
    ) -> Result<Appended, MailError> {
        let target = self.store().mailbox(self.id, mailbox).await?;
        if !target.selectable {
            return Err(MailError::Invalid("mailbox can't hold messages"));
        }
        let flags: Vec<Flag> = keywords
            .iter()
            .map(|keyword| to_flag(keyword).ok_or(MailError::Invalid("keyword")))
            .collect::<Result<_, _>>()?;
        let date = received_at
            .map(|time| InternalDate::from_unix_timestamp(time.secs()))
            .transpose()
            .map_err(|_| MailError::Invalid("receivedAt"))?;

        let mut slot = self.slot().await;
        let session = slot.session(self).await?;
        let result = session.append(&target.imap_name, raw, &flags, date).await;
        let appended = slot.check(result)?;
        drop(slot);

        let email = match appended.uid {
            Some((validity, uid)) => self.adopt_appended(&target, validity, uid).await?,
            None => None,
        };
        let appended = match email {
            Some(appended) => appended,
            None => Appended::maybe_existing(self.find_appended(&target, raw).await?),
        };
        self.shared
            .cache
            .insert(self.id, appended.email, Arc::from(raw));
        Ok(appended)
    }

    /// Stores the message at `uid` right away (APPENDUID). `None` if the
    /// mailbox's UIDVALIDITY isn't the one we know (the sync will sort it
    /// out) or the server doesn't show the message.
    async fn adopt_appended(
        &self,
        target: &Mailbox,
        validity: UidValidity,
        uid: Uid,
    ) -> Result<Option<Appended>, MailError> {
        // Everything with the connection ours (as in `move_copy`): the
        // state is read again (a sync may have stored it meanwhile), and a
        // sync can't plan from a state older than our write, reset the
        // mailbox and import the message a second time.
        let mut slot = self.slot().await;
        let current = self.store().mailbox(self.id, target.id).await?;
        match current.sync.uid_validity {
            Some(known) if known != validity => return Ok(None),
            Some(_) => {}
            None => {
                let state = MailboxSyncState {
                    uid_validity: Some(validity),
                    ..MailboxSyncState::default()
                };
                self.store()
                    .set_mailbox_sync_state(self.id, target.id, state)
                    .await?;
            }
        }
        let (client, _) = slot
            .select(
                self,
                &target.imap_name,
                SelectOptions::default(),
                Reselect::IfOther,
            )
            .await?;
        if client.mailbox().uid_validity != validity {
            return Ok(None);
        }
        // A UID we already have: a server that deduplicates answered with
        // a message that was there before (or a sync got it first).
        let known_uid = self
            .store()
            .email_at(self.id, target.id, uid)
            .await?
            .is_some();
        let messages = fetch_metadata(&mut slot, &[uid]).await?.messages;
        if messages.is_empty() {
            return Ok(None);
        }
        let mut recorded = self
            .store()
            .add_messages(self.id, target.id, messages, UnixTime::now()?)
            .await?;
        // Our own write, not a delivery: a StateChange, no EmailPush.
        recorded.not_delivered();
        let Some(email) = self.store().email_at(self.id, target.id, uid).await? else {
            drop(slot);
            self.emit(&recorded);
            return Ok(None);
        };
        // Other copies: the store joined it to an email already there.
        let locations = self.store().locations(self.id, email).await?;
        drop(slot);
        self.emit(&recorded);
        let only_this_copy = matches!(
            locations.as_slice(),
            [location] if location.mailbox == target.id && location.uid == uid
        );
        let origin = if only_this_copy && !known_uid {
            AppendOrigin::New
        } else {
            AppendOrigin::MaybeExisting
        };
        Ok(Some(Appended { email, origin }))
    }

    /// Without APPENDUID: sync the mailbox and look the message up by its
    /// Message-ID.
    async fn find_appended(&self, target: &Mailbox, raw: &[u8]) -> Result<EmailId, MailError> {
        self.sync_mailbox(target.id).await?;
        let message_id = headers::parse(raw)
            .message_id
            .ok_or(MailError::NotFound("appended message (no Message-ID)"))?;
        self.store()
            .emails_by_message_id(self.id, target.id, message_id)
            .await?
            .into_iter()
            .max()
            .ok_or(MailError::NotFound("appended message"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_by_bytes() {
        assert_eq!(batches(&[], 10), Vec::<Range<usize>>::new());
        assert_eq!(batches(&[3, 3, 3], 10), vec![0..3]);
        assert_eq!(batches(&[4, 4, 4, 4], 10), vec![0..2, 2..4]);
        // A message over the limit goes alone.
        assert_eq!(batches(&[2, 30, 2, 2], 10), vec![0..1, 1..2, 2..4]);
        assert_eq!(batches(&[30], 10), vec![0..1]);
        // Batches are contiguous, cover everything once and stay small.
        let sizes = [5, 1, 9, 2, 2, 7, 11, 1];
        let list = batches(&sizes, 10);
        let covered: Vec<usize> = list.iter().cloned().flatten().collect();
        assert_eq!(covered, (0..sizes.len()).collect::<Vec<_>>());
        for range in list {
            let bytes: usize = sizes.get(range.clone()).unwrap_or_default().iter().sum();
            assert!(bytes <= 10 || range.len() == 1, "{range:?}");
        }
    }

    #[test]
    fn which_failures_go_one_by_one() {
        let refusal = nebula_imap::Refusal {
            command: "APPEND",
            status: nebula_imap::proto::response::Completion::No,
            code: None,
            text: "too big".to_owned(),
        };
        assert!(batch_falls_back(&MailError::Refused(refusal)));
        assert!(batch_falls_back(&MailError::Invalid("value")));
        assert!(!batch_falls_back(&MailError::AuthFailed));
        assert!(!batch_falls_back(&MailError::Unavailable(
            crate::Unavailable::Backoff
        )));
    }
}
