// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sync from IMAP to the store: the mailbox list, then each mailbox.

pub(crate) mod backoff;
pub(crate) mod list;
pub(crate) mod mailbox;

use std::collections::HashSet;

use nebula_imap::Error as ImapError;
use nebula_imap::proto::response::MailboxStatus;
use nebula_imap::proto::types::ModSeq;
use nebula_store::{Mailbox, MailboxId, Recorded, Role};
use tokio::time::Instant;

use self::backoff::{Failure, MailboxBackoff};
use crate::core::account::MailAccount;
use crate::{MailError, Unavailable};

/// INBOX first, then the other roles, then the rest: the mailboxes the
/// user looks at first are ready first.
fn sync_order(mut mailboxes: Vec<Mailbox>) -> Vec<MailboxId> {
    mailboxes.retain(|m| m.selectable);
    mailboxes.sort_by_key(|m| (m.role != Some(Role::Inbox), m.sort_order, m.id));
    mailboxes.into_iter().map(|m| m.id).collect()
}

/// Who asks for a mailbox sync, for its backoff ([`backoff`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// The full sync, the periodic check, search: skipped while the
    /// mailbox waits; a local failure is logged and skipped.
    Background,
    /// News about this mailbox (IDLE/NOTIFY): as `Background`, but tried
    /// again a minute after its last failure.
    News,
    /// The user waits for the result (APPEND, MOVE): always tried, and a
    /// local failure is returned (and noted).
    Requested,
}

/// Whether an error syncing one mailbox concerns only that mailbox (the
/// server refused SELECT or STATUS, sent nonsense for it, or the mailbox
/// went away): it is logged and the sync goes on with the next one, so a
/// broken mailbox doesn't stop the whole account. Login, connection,
/// stopping, configuration and database errors still end the sync.
///
/// A response too large for the limits, or a SELECT without UIDVALIDITY,
/// also concern only that mailbox: the connection, if it was closed, is
/// made again by the next command.
fn mailbox_error_is_local(error: &MailError) -> bool {
    matches!(
        error,
        MailError::Refused(_)
            | MailError::Unsupported(_)
            | MailError::Invalid(_)
            | MailError::NotFound(_)
            | MailError::Unavailable(Unavailable::Imap(
                ImapError::ResponseTooLarge(_) | ImapError::MissingUidValidity
            ))
    )
}

/// Runs `sync` for mailbox `id` under its backoff (see [`Trigger`]).
/// `inbox` gives it the shorter cap. Errors that are not local are
/// neither noted nor swallowed: they are the account's (its own backoff).
pub(crate) async fn tracked<F, Fut>(
    backoff: &MailboxBackoff,
    id: MailboxId,
    inbox: bool,
    trigger: Trigger,
    sync: F,
) -> Result<Recorded, MailError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Recorded, MailError>>,
{
    let now = Instant::now();
    let due = match trigger {
        Trigger::Background => backoff.due(id, now),
        Trigger::News => backoff.due_for_news(id, now),
        Trigger::Requested => true,
    };
    if !due {
        tracing::debug!(mailbox = %id, "mailbox waiting after a failure; not synced");
        return Ok(Recorded::default());
    }
    match sync().await {
        Ok(recorded) => {
            backoff.succeeded(id);
            Ok(recorded)
        }
        Err(error) if mailbox_error_is_local(&error) => {
            let wait = backoff.failed(id, Instant::now(), Failure::of(&error), inbox);
            if trigger == Trigger::Requested {
                return Err(error);
            }
            tracing::warn!(
                mailbox = %id,
                %error,
                wait_secs = wait.as_secs(),
                "mailbox skipped until its wait is over"
            );
            Ok(Recorded::default())
        }
        Err(error) => Err(error),
    }
}

fn is_inbox(mailbox: &Mailbox) -> bool {
    mailbox.role == Some(Role::Inbox)
}

/// What LIST told about one mailbox, for [`MailAccount::check_mailboxes`].
#[derive(Debug, PartialEq, Eq)]
enum ListedStatus<'a> {
    /// It was selected on our connection: its STATUS may be what our
    /// session last saw, not the server's state (Dovecot 2.4), so it is
    /// synced anyway (cheap with CONDSTORE/QRESYNC, and it is the mailbox
    /// the user last looked at).
    Selected,
    /// LIST-STATUS gave its STATUS.
    Known(&'a MailboxStatus),
    /// Ask STATUS.
    Unknown,
}

fn listed_status<'a>(listed: &'a list::Listed, mailbox: &Mailbox) -> ListedStatus<'a> {
    if listed.selected.as_ref() == Some(&mailbox.imap_name) {
        return ListedStatus::Selected;
    }
    listed
        .statuses
        .get(mailbox.imap_name.as_str())
        .map_or(ListedStatus::Unknown, ListedStatus::Known)
}

/// Whether STATUS shows changes since the last complete sync.
pub(crate) fn status_changed(mailbox: &Mailbox, status: &MailboxStatus) -> bool {
    let sync = &mailbox.sync;
    if sync.synced_at.is_none() {
        return true;
    }
    let differs = |ours: Option<u64>, theirs: Option<u64>| theirs.is_some() && ours != theirs;
    differs(
        sync.uid_validity.map(|v| u64::from(v.get())),
        status.uid_validity.map(|v| u64::from(v.get())),
    ) || differs(
        sync.uid_next.map(|v| u64::from(v.get())),
        status.uid_next.map(|v| u64::from(v.get())),
    ) || differs(sync.messages.map(u64::from), status.messages.map(u64::from))
        || differs(
            sync.highest_mod_seq.map(ModSeq::get),
            status.highest_mod_seq.map(ModSeq::get),
        )
}

impl MailAccount {
    /// Full sync: the mailbox list, then every mailbox (INBOX first).
    ///
    /// # Errors
    /// [`MailError::AuthFailed`] (end the session), [`MailError::Unavailable`]
    /// (retry later; [`crate::Unavailable::Stopping`] when the account is
    /// closing) or database errors. Mailboxes synced before an error keep
    /// their new data. A mailbox the server refuses is skipped.
    pub async fn sync_all(&self) -> Result<Recorded, MailError> {
        self.ensure_running()?;
        let _one_sync = self.sync_lock.lock().await;
        let mut recorded = list::sync_list(self).await?.recorded;
        recorded.merge(self.sync_stored_mailboxes().await?);
        Ok(recorded)
    }

    /// The second half of [`Self::sync_all`], after LIST: every stored
    /// mailbox, INBOX first. The caller holds `sync_lock`. Apart from
    /// `sync_all`, only tests call it, to change the server between the
    /// two halves (a mailbox deleted after LIST).
    pub(crate) async fn sync_stored_mailboxes(&self) -> Result<Recorded, MailError> {
        let mut recorded = Recorded::default();
        let mailboxes = self.store().mailboxes(self.id).await?;
        self.forget_gone_mailboxes(&mailboxes);
        let inbox = mailboxes.iter().find(|m| is_inbox(m)).map(|m| m.id);
        for id in sync_order(mailboxes) {
            // Closing: stop between mailboxes, not after all of them.
            self.ensure_running()?;
            let synced = tracked(
                &self.mailbox_backoff,
                id,
                inbox == Some(id),
                Trigger::Background,
                || mailbox::sync_locked(self, id),
            )
            .await?;
            recorded.merge(synced);
        }
        Ok(recorded)
    }

    /// Syncs one mailbox now, whatever its backoff, for an operation the
    /// user waits for.
    ///
    /// # Errors
    /// As [`Self::sync_all`]; [`MailError::Store`] with `NotFound` for an
    /// unknown mailbox. A mailbox the server refuses fails too (and waits
    /// before the background syncs try it again).
    pub async fn sync_mailbox(&self, id: MailboxId) -> Result<Recorded, MailError> {
        self.sync_mailbox_for(id, Trigger::Requested).await
    }

    /// Syncs one mailbox under its backoff (see [`Trigger`]).
    pub(crate) async fn sync_mailbox_for(
        &self,
        id: MailboxId,
        trigger: Trigger,
    ) -> Result<Recorded, MailError> {
        self.ensure_running()?;
        let inbox = is_inbox(&self.store().mailbox(self.id, id).await?);
        tracked(&self.mailbox_backoff, id, inbox, trigger, || {
            mailbox::sync_mailbox(self, id)
        })
        .await
    }

    /// Forgets the backoff of mailboxes that are gone (after LIST).
    fn forget_gone_mailboxes(&self, mailboxes: &[Mailbox]) {
        let alive: HashSet<MailboxId> = mailboxes.iter().map(|m| m.id).collect();
        self.mailbox_backoff.retain(&alive);
    }

    /// The cheap periodic check: LIST (with STATUS when the server has
    /// LIST-STATUS, else one STATUS per mailbox), then a sync of the
    /// mailboxes whose UIDNEXT, MESSAGES or HIGHESTMODSEQ changed, and of
    /// the one selected on our connection (its STATUS may be stale).
    ///
    /// # Errors
    /// As [`Self::sync_all`].
    pub async fn check_mailboxes(&self) -> Result<Recorded, MailError> {
        self.ensure_running()?;
        let _one_sync = self.sync_lock.lock().await;
        let listed = list::sync_list(self).await?;
        self.check_listed(listed).await
    }

    /// The second half of [`Self::check_mailboxes`], after LIST: STATUS
    /// (from `listed` or asked now) and a sync of what changed. The caller
    /// holds `sync_lock`. Apart from `check_mailboxes`, only tests call
    /// it, to change the server between the two halves.
    pub(crate) async fn check_listed(
        &self,
        mut listed: list::Listed,
    ) -> Result<Recorded, MailError> {
        let mut recorded = std::mem::take(&mut listed.recorded);
        let mailboxes = self.store().mailboxes(self.id).await?;
        self.forget_gone_mailboxes(&mailboxes);
        for mailbox in mailboxes.into_iter().filter(|m| m.selectable) {
            self.ensure_running()?;
            // A mailbox that waits gets not even a STATUS; a STATUS that
            // fails on its own counts as a failure of the mailbox.
            let synced = tracked(
                &self.mailbox_backoff,
                mailbox.id,
                is_inbox(&mailbox),
                Trigger::Background,
                || async {
                    let changed = match listed_status(&listed, &mailbox) {
                        ListedStatus::Selected => true,
                        ListedStatus::Known(status) => status_changed(&mailbox, status),
                        ListedStatus::Unknown => {
                            status_changed(&mailbox, &self.status(&mailbox).await?)
                        }
                    };
                    if changed {
                        mailbox::sync_locked(self, mailbox.id).await
                    } else {
                        Ok(Recorded::default())
                    }
                },
            )
            .await?;
            recorded.merge(synced);
        }
        Ok(recorded)
    }

    async fn status(&self, mailbox: &Mailbox) -> Result<MailboxStatus, MailError> {
        let mut slot = self.slot().await;
        let session = slot.session(self).await?;
        let result = session.status(&mailbox.imap_name, list::STATUS_ITEMS).await;
        slot.check(result)
    }
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::types::{MailboxName, Uid, UidValidity};
    use nebula_store::{MailboxSyncState, UnixTime};

    use super::*;

    fn mailbox(id: i64, name: &str, role: Option<Role>, sync: MailboxSyncState) -> Mailbox {
        Mailbox {
            id: MailboxId::from_raw(id).unwrap(),
            imap_name: MailboxName::new(name).unwrap(),
            name: name.to_owned(),
            delimiter: Some('/'),
            parent: None,
            role,
            subscribed: true,
            selectable: true,
            no_inferiors: false,
            sort_order: crate::model::role::sort_order(role),
            sync,
        }
    }

    fn status(messages: u32, uid_next: u32, mod_seq: u64) -> MailboxStatus {
        let wire = format!(
            "* STATUS INBOX (MESSAGES {messages} UIDNEXT {uid_next} UIDVALIDITY 7 HIGHESTMODSEQ {mod_seq})\r\n"
        );
        let mut decoder = nebula_imap::proto::response::ResponseDecoder::new(
            nebula_imap::proto::response::FrameLimits::default(),
        );
        decoder.feed(wire.as_bytes());
        match decoder.decode() {
            Ok(Some(nebula_imap::proto::response::Response::Untagged(
                nebula_imap::proto::response::Untagged::MailboxStatus(status),
            ))) => status,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn order_puts_inbox_first() {
        let empty = MailboxSyncState::default();
        let list = vec![
            mailbox(1, "Zeta", None, empty),
            mailbox(2, "Trash", Some(Role::Trash), empty),
            mailbox(3, "INBOX", Some(Role::Inbox), empty),
            mailbox(4, "Sent", Some(Role::Sent), empty),
        ];
        let order: Vec<i64> = sync_order(list).into_iter().map(MailboxId::get).collect();
        assert_eq!(order, vec![3, 4, 2, 1]);
    }

    #[test]
    fn local_mailbox_errors() {
        let refusal = nebula_imap::Refusal {
            command: "SELECT",
            status: nebula_imap::proto::response::Completion::No,
            code: None,
            text: "no".to_owned(),
        };
        assert!(mailbox_error_is_local(&MailError::Refused(refusal)));
        assert!(mailbox_error_is_local(&MailError::Unsupported("x")));
        assert!(mailbox_error_is_local(&MailError::Invalid(
            "too many FETCH"
        )));
        assert!(mailbox_error_is_local(&MailError::NotFound(
            "selected mailbox"
        )));
        assert!(mailbox_error_is_local(&MailError::Unavailable(
            Unavailable::Imap(ImapError::ResponseTooLarge("FETCH"))
        )));
        assert!(mailbox_error_is_local(&MailError::Unavailable(
            Unavailable::Imap(ImapError::MissingUidValidity)
        )));
        assert!(!mailbox_error_is_local(&MailError::Unavailable(
            Unavailable::Imap(ImapError::ConnectionClosed)
        )));
        assert!(!mailbox_error_is_local(&MailError::AuthFailed));
        assert!(!mailbox_error_is_local(&MailError::Unavailable(
            Unavailable::Backoff
        )));
        assert!(!mailbox_error_is_local(&MailError::Store(
            nebula_store::StoreError::NotFound("mailbox")
        )));
        assert!(!mailbox_error_is_local(&MailError::Config(
            nebula_imap::Error::InvalidServerName
        )));
    }

    fn refused() -> MailError {
        MailError::Refused(nebula_imap::Refusal {
            command: "SELECT",
            status: nebula_imap::proto::response::Completion::No,
            code: None,
            text: "broken".to_owned(),
        })
    }

    /// One background round over `ids`, where `broken` fails on its own:
    /// the ones actually tried.
    async fn round(
        backoff: &MailboxBackoff,
        ids: &[MailboxId],
        broken: MailboxId,
    ) -> Result<Vec<MailboxId>, MailError> {
        let tried = std::sync::Mutex::new(Vec::new());
        for id in ids {
            let id = *id;
            tracked(backoff, id, false, Trigger::Background, || async {
                tried.lock().unwrap().push(id);
                if id == broken {
                    Err(refused())
                } else {
                    Ok(Recorded::default())
                }
            })
            .await?;
        }
        Ok(tried.into_inner().unwrap())
    }

    #[tokio::test(start_paused = true)]
    async fn a_broken_mailbox_is_skipped_until_due() {
        let backoff = MailboxBackoff::default();
        let (a, b) = (
            MailboxId::from_raw(1).unwrap(),
            MailboxId::from_raw(2).unwrap(),
        );
        // A fails: B is still synced.
        assert_eq!(round(&backoff, &[a, b], a).await.unwrap(), vec![a, b]);
        // The next round leaves A alone.
        assert_eq!(round(&backoff, &[a, b], a).await.unwrap(), vec![b]);
        tokio::time::advance(backoff::FIRST).await;
        assert_eq!(round(&backoff, &[a, b], a).await.unwrap(), vec![a, b]);
        // Twice now: two minutes.
        tokio::time::advance(backoff::FIRST).await;
        assert_eq!(round(&backoff, &[a, b], a).await.unwrap(), vec![b]);
        // News about A tries it a minute after the last failure.
        let news = tracked(&backoff, a, false, Trigger::News, || async {
            Ok(Recorded::default())
        })
        .await;
        assert!(news.is_ok());
        // Synced: no wait any more.
        assert_eq!(round(&backoff, &[a, b], b).await.unwrap(), vec![a, b]);
    }

    #[tokio::test(start_paused = true)]
    async fn requested_syncs_ignore_the_backoff_but_report() {
        let backoff = MailboxBackoff::default();
        let a = MailboxId::from_raw(1).unwrap();
        backoff.failed(a, Instant::now(), Failure::Local, false);
        let result = tracked(&backoff, a, false, Trigger::Requested, || async {
            Err(refused())
        })
        .await;
        assert!(matches!(result, Err(MailError::Refused(_))));
        // And noted: twice now.
        assert!(!backoff.due(a, Instant::now() + backoff::FIRST));
        tracked(&backoff, a, false, Trigger::Requested, || async {
            Ok(Recorded::default())
        })
        .await
        .unwrap();
        assert!(backoff.due(a, Instant::now()));
    }

    #[tokio::test(start_paused = true)]
    async fn a_global_error_stops_the_round() {
        let backoff = MailboxBackoff::default();
        let (a, b) = (
            MailboxId::from_raw(1).unwrap(),
            MailboxId::from_raw(2).unwrap(),
        );
        let mut tried = Vec::new();
        let mut result = Ok(());
        for id in [a, b] {
            tried.push(id);
            let synced = tracked(&backoff, id, false, Trigger::Background, || async {
                Err(MailError::Unavailable(Unavailable::Imap(
                    ImapError::Timeout("FETCH"),
                )))
            })
            .await;
            if let Err(error) = synced {
                result = Err(error);
                break;
            }
        }
        assert!(matches!(result, Err(MailError::Unavailable(_))));
        assert_eq!(tried, vec![a]);
        // Not the mailbox's fault: not noted.
        assert!(backoff.due(a, Instant::now()));
    }

    #[test]
    fn the_selected_mailbox_is_synced_whatever_its_status() {
        let synced = MailboxSyncState {
            uid_validity: UidValidity::new(7),
            uid_next: Uid::new(10),
            highest_mod_seq: ModSeq::new(100),
            messages: Some(9),
            synced_at: Some(UnixTime::from_secs(1)),
        };
        let inbox = mailbox(1, "INBOX", Some(Role::Inbox), synced);
        let other = mailbox(2, "X", None, synced);
        let same = status(9, 10, 100);
        let listed = list::Listed {
            recorded: Recorded::default(),
            statuses: [("INBOX".to_owned(), same.clone())].into(),
            selected: Some(MailboxName::new("INBOX").unwrap()),
        };
        // Unchanged as far as LIST-STATUS says, but selected: that STATUS
        // may be stale.
        assert_eq!(listed_status(&listed, &inbox), ListedStatus::Selected);
        assert_eq!(listed_status(&listed, &other), ListedStatus::Unknown);
        let listed = list::Listed {
            selected: Some(MailboxName::new("X").unwrap()),
            ..listed
        };
        assert_eq!(listed_status(&listed, &inbox), ListedStatus::Known(&same));
        assert_eq!(listed_status(&listed, &other), ListedStatus::Selected);
    }

    #[test]
    fn status_comparison() {
        let synced = MailboxSyncState {
            uid_validity: UidValidity::new(7),
            uid_next: Uid::new(10),
            highest_mod_seq: ModSeq::new(100),
            messages: Some(9),
            synced_at: Some(UnixTime::from_secs(1)),
        };
        let inbox = mailbox(1, "INBOX", Some(Role::Inbox), synced);
        assert!(!status_changed(&inbox, &status(9, 10, 100)));
        assert!(status_changed(&inbox, &status(9, 10, 101)));
        assert!(status_changed(&inbox, &status(8, 10, 100)));
        assert!(status_changed(&inbox, &status(9, 11, 100)));
        let never = mailbox(2, "X", None, MailboxSyncState::default());
        assert!(status_changed(&never, &status(0, 1, 1)));
    }
}
