// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Text search on the server (`UID SEARCH`): the JMAP layer turns the
//! result into an `EmailFilter::Ids` condition for the local query.
//!
//! The server may find messages newer than the last sync, which the store
//! doesn't know yet. When a found UID is at or above the stored UIDNEXT,
//! the mailbox is synced before the UIDs are looked up, so new mail shows
//! up in the results. If another sync is running (the first sync of the
//! account can take minutes) we wait for it a little, then go on with what
//! is stored: that sync brings the rest.

use std::collections::BTreeSet;
use std::time::Duration;

use nebula_imap::proto::command::SearchKey;
use nebula_imap::proto::types::Uid;
use nebula_store::{EmailId, Mailbox, MailboxId, Role, StoreError};
use tokio::time::Instant;

use super::select_location_by_name;
use crate::MailError;
use crate::core::account::MailAccount;
use crate::sync::mailbox::sync_locked;
use crate::sync::{Trigger, tracked};

/// Longest wait for a running sync, for the whole search.
const SYNC_WAIT: Duration = Duration::from_secs(2);

/// Whether some UID is newer than what the last complete sync saw.
fn has_unsynced(mailbox: &Mailbox, uids: &[Uid]) -> bool {
    match mailbox.sync.uid_next {
        Some(next) => uids.iter().any(|uid| *uid >= next),
        // Never synced to the end: anything may be missing.
        None => !uids.is_empty(),
    }
}

/// Which part of the message a text condition looks at (RFC 8621 §4.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TextField {
    /// Headers and body (`text`).
    Text,
    /// `from`.
    From,
    /// `to`.
    To,
    /// `cc`.
    Cc,
    /// `bcc`.
    Bcc,
    /// `subject`.
    Subject,
    /// `body`.
    Body,
}

/// One text condition. All conditions of a search must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextCondition {
    /// Where to look.
    pub field: TextField,
    /// What to look for (as typed by the user; sent as an IMAP string).
    pub value: String,
}

fn key(condition: &TextCondition) -> SearchKey {
    let value = condition.value.clone();
    match condition.field {
        TextField::Text => SearchKey::Text(value),
        TextField::From => SearchKey::From(value),
        TextField::To => SearchKey::To(value),
        TextField::Cc => SearchKey::Cc(value),
        TextField::Bcc => SearchKey::Bcc(value),
        TextField::Subject => SearchKey::Subject(value),
        TextField::Body => SearchKey::Body(value),
    }
}

impl MailAccount {
    /// Emails matching every condition, in one mailbox or in all of them.
    ///
    /// # Errors
    /// [`MailError::Invalid`] without conditions or with a value the server
    /// can't be sent; connection errors.
    pub async fn search(
        &self,
        mailbox: Option<MailboxId>,
        conditions: &[TextCondition],
    ) -> Result<BTreeSet<EmailId>, MailError> {
        if conditions.is_empty() {
            return Err(MailError::Invalid("search without conditions"));
        }
        let criteria = SearchKey::And(conditions.iter().map(key).collect());
        let mailboxes = match mailbox {
            Some(id) => match self.store().mailbox(self.id, id).await {
                Ok(mailbox) => vec![mailbox],
                // Not one of ours (or just deleted): nothing is in it, as
                // the local `inMailbox` condition says too.
                Err(StoreError::NotFound(_)) => return Ok(BTreeSet::new()),
                Err(error) => return Err(error.into()),
            },
            None => self.store().mailboxes(self.id).await?,
        };
        let mut found = BTreeSet::new();
        let deadline = Instant::now() + SYNC_WAIT;
        for listed in mailboxes.into_iter().filter(|m| m.selectable) {
            let mut slot = self.slot().await;
            // Its sync state as of now, not of the list: an operation that
            // held the connection may have stored the first UIDVALIDITY
            // (MOVE, APPEND), and the old one would skip the mailbox.
            let mailbox = match self.store().mailbox(self.id, listed.id).await {
                Ok(mailbox) => mailbox,
                Err(StoreError::NotFound(_)) => continue,
                Err(error) => return Err(error.into()),
            };
            let client = match select_location_by_name(self, &mut slot, &mailbox.imap_name).await {
                Ok(client) => client,
                // Gone or not selectable any more (another client deleted
                // it; the next list sync drops it): not a reason to fail the
                // search of every other mailbox.
                Err(MailError::Refused(refusal)) => {
                    tracing::debug!(mailbox = %mailbox.id, %refusal, "search: mailbox skipped");
                    continue;
                }
                Err(error) => return Err(error),
            };
            if mailbox.sync.uid_validity != Some(client.mailbox().uid_validity) {
                // Stored UIDs belong to another UIDVALIDITY and would map
                // to the wrong emails; the next sync of this mailbox fixes it.
                continue;
            }
            let result = client.uid_search(&criteria).await;
            let uids = slot.check(result)?.uids();
            drop(slot);
            if uids.is_empty() {
                continue;
            }
            if has_unsynced(&mailbox, &uids) {
                self.sync_for_search(&mailbox, deadline).await?;
                // The UIDs only mean something with the same UIDVALIDITY
                // (and while the mailbox still exists).
                match self.store().mailbox(self.id, mailbox.id).await {
                    Ok(now) if now.sync.uid_validity == mailbox.sync.uid_validity => {}
                    Ok(_) | Err(StoreError::NotFound(_)) => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            found.extend(self.store().emails_at(self.id, mailbox.id, uids).await?);
        }
        Ok(found)
    }
}

impl MailAccount {
    /// Syncs a mailbox before mapping search results, unless another sync
    /// holds the lock past `deadline`. A failed sync doesn't fail the
    /// search (the mailbox may be gone, or the server busy): the stored
    /// data is used, as without this step. Only a refused login counts.
    /// A mailbox waiting after a failure is not synced (its backoff).
    async fn sync_for_search(&self, mailbox: &Mailbox, deadline: Instant) -> Result<(), MailError> {
        let Ok(_one_sync) = tokio::time::timeout_at(deadline, self.sync_lock.lock()).await else {
            tracing::debug!(account = %self.id, "search: a sync is running, using stored data");
            return Ok(());
        };
        let id = mailbox.id;
        let inbox = mailbox.role == Some(Role::Inbox);
        let synced = tracked(
            &self.mailbox_backoff,
            id,
            inbox,
            Trigger::Background,
            || sync_locked(self, id),
        )
        .await;
        match synced {
            Ok(_) => Ok(()),
            Err(MailError::AuthFailed) => Err(MailError::AuthFailed),
            Err(error) => {
                tracing::debug!(account = %self.id, %error, "search: sync failed, using stored data");
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::types::MailboxName;
    use nebula_store::MailboxSyncState;

    use super::*;

    #[test]
    fn unsynced_uids() {
        let mut mailbox = Mailbox {
            id: MailboxId::from_raw(1).unwrap(),
            imap_name: MailboxName::inbox(),
            name: "Inbox".to_owned(),
            delimiter: Some('/'),
            parent: None,
            role: None,
            subscribed: true,
            selectable: true,
            no_inferiors: false,
            sort_order: 0,
            sync: MailboxSyncState::default(),
        };
        let uids = |list: &[u32]| {
            list.iter()
                .map(|n| Uid::new(*n).unwrap())
                .collect::<Vec<_>>()
        };
        assert!(has_unsynced(&mailbox, &uids(&[3])));
        assert!(!has_unsynced(&mailbox, &[]));
        mailbox.sync.uid_next = Uid::new(10);
        assert!(!has_unsynced(&mailbox, &uids(&[3, 9])));
        assert!(has_unsynced(&mailbox, &uids(&[3, 10])));
    }

    #[test]
    fn conditions_map_to_search_keys() {
        let condition = |field| TextCondition {
            field,
            value: "hola".to_owned(),
        };
        assert_eq!(
            key(&condition(TextField::Subject)),
            SearchKey::Subject("hola".into())
        );
        assert_eq!(
            key(&condition(TextField::Body)),
            SearchKey::Body("hola".into())
        );
        assert_eq!(
            key(&condition(TextField::Text)),
            SearchKey::Text("hola".into())
        );
    }
}
