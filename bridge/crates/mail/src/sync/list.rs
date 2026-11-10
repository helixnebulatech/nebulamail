// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! LIST → the stored mailboxes (with SPECIAL-USE, and LIST-STATUS when the
//! server has it, so one command also tells which mailboxes changed).

use std::collections::{HashMap, HashSet};

use nebula_imap::proto::command::StatusItem;
use nebula_imap::proto::response::MailboxStatus;
use nebula_imap::proto::types::{Capability, MailboxName};
use nebula_store::{Recorded, UnixTime};

use crate::MailError;
use crate::core::account::MailAccount;
use crate::model::hierarchy::{self, Namespace};

pub(crate) const STATUS_ITEMS: &[StatusItem] = &[
    StatusItem::Messages,
    StatusItem::UidNext,
    StatusItem::UidValidity,
    StatusItem::HighestModSeq,
];

/// The result of a mailbox list sync.
#[derive(Debug, Default)]
pub(crate) struct Listed {
    pub(crate) recorded: Recorded,
    /// STATUS of each mailbox by name, when LIST-STATUS gave it.
    pub(crate) statuses: HashMap<String, MailboxStatus>,
    /// The mailbox our connection had selected during LIST. Its STATUS
    /// can be what this session last saw rather than the server's state
    /// (Dovecot 2.4 answers so: mail another client added doesn't count
    /// until the session syncs it), so it is not to be trusted.
    pub(crate) selected: Option<MailboxName>,
}

pub(crate) async fn sync_list(account: &MailAccount) -> Result<Listed, MailError> {
    let now = UnixTime::now()?;
    let mut slot = account.slot().await;
    let session = slot.session(account).await?;
    let namespace = if session.capabilities().has(&Capability::Namespace) {
        let result = session.namespace().await;
        match slot.check(result) {
            Ok(namespaces) => Namespace {
                prefix: namespaces
                    .personal
                    .first()
                    .map(|entry| entry.prefix.clone())
                    .filter(|prefix| !prefix.is_empty()),
                others: namespaces
                    .other_users
                    .iter()
                    .chain(&namespaces.shared)
                    .map(|entry| entry.prefix.clone())
                    .filter(|prefix| !prefix.is_empty())
                    .collect(),
            },
            // Not worth failing the sync for: assume no prefix.
            Err(MailError::Refused(_) | MailError::Unsupported(_)) => Namespace::default(),
            Err(error) => return Err(error),
        }
    } else {
        Namespace::default()
    };
    let session = slot.session(account).await?;
    let selected = session.selected_name().cloned();
    let result = session.list(STATUS_ITEMS).await;
    let listed = slot.check(result)?;

    // Applied with the connection still ours (see `mailbox::plan`): a
    // RENAME in between would be undone by the list read before it.
    let entries: Vec<_> = listed.iter().map(|m| m.entry.clone()).collect();
    let subscribed: HashSet<String> = listed
        .iter()
        .filter(|m| m.subscribed)
        .map(|m| m.entry.name.as_str().to_owned())
        .collect();
    let statuses = listed
        .into_iter()
        .filter_map(|m| {
            let status = m.status?;
            Some((m.entry.name.as_str().to_owned(), status))
        })
        .collect();
    let mailboxes = hierarchy::build(&entries, &subscribed, &namespace);
    let recorded = account
        .store()
        .sync_mailboxes(account.id, mailboxes, now)
        .await?;
    drop(slot);
    account.emit(&recorded);
    Ok(Listed {
        recorded,
        statuses,
        selected,
    })
}
