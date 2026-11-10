// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! CREATE, DELETE, RENAME, SUBSCRIBE, UNSUBSCRIBE and STATUS.

use nebula_imap_proto::command::{
    Create, Delete, Rename, Status, StatusItem, Subscribe, Unsubscribe,
};
use nebula_imap_proto::response::{MailboxStatus, Untagged};
use nebula_imap_proto::types::{Capability, MailboxName, SpecialUse};

use crate::Error;
use crate::core::client::{Client, LoggedIn};
use crate::core::connection::{StrayContinuation, expect_ok};

impl<S: LoggedIn> Client<S> {
    /// Creates a mailbox, optionally with a special use (needs
    /// `CREATE-SPECIAL-USE`).
    ///
    /// # Errors
    ///
    /// NO (`ALREADYEXISTS`…), missing capability, or connection errors.
    pub async fn create(
        &mut self,
        mailbox: &MailboxName,
        special_use: Option<SpecialUse>,
    ) -> Result<(), Error> {
        if special_use.is_some() && !self.session.has(&Capability::CreateSpecialUse) {
            return Err(Error::Unsupported("CREATE-SPECIAL-USE"));
        }
        self.run_simple(&Create {
            mailbox,
            special_use,
        })
        .await
        .map(|_| ())
    }

    /// Deletes a mailbox.
    ///
    /// # Errors
    ///
    /// NO (`NONEXISTENT`, `INUSE`…) or connection errors.
    pub async fn delete(&mut self, mailbox: &MailboxName) -> Result<(), Error> {
        self.run_simple(&Delete { mailbox }).await.map(|_| ())
    }

    /// Renames a mailbox.
    ///
    /// # Errors
    ///
    /// NO or connection errors.
    pub async fn rename(&mut self, from: &MailboxName, to: &MailboxName) -> Result<(), Error> {
        self.run_simple(&Rename { from, to }).await.map(|_| ())
    }

    /// Subscribes to a mailbox.
    ///
    /// # Errors
    ///
    /// NO or connection errors.
    pub async fn subscribe(&mut self, mailbox: &MailboxName) -> Result<(), Error> {
        self.run_simple(&Subscribe { mailbox }).await.map(|_| ())
    }

    /// Unsubscribes from a mailbox.
    ///
    /// # Errors
    ///
    /// NO or connection errors.
    pub async fn unsubscribe(&mut self, mailbox: &MailboxName) -> Result<(), Error> {
        self.run_simple(&Unsubscribe { mailbox }).await.map(|_| ())
    }

    /// `STATUS`: counters of a mailbox without selecting it. Items the
    /// server cannot answer (HIGHESTMODSEQ without CONDSTORE) are dropped.
    ///
    /// # Errors
    ///
    /// NO or connection errors.
    pub async fn status(
        &mut self,
        mailbox: &MailboxName,
        items: &[StatusItem],
    ) -> Result<MailboxStatus, Error> {
        let condstore = self.session.has(&Capability::CondStore);
        let items: Vec<StatusItem> = items
            .iter()
            .copied()
            .filter(|item| condstore || *item != StatusItem::HighestModSeq)
            .collect();
        let command = Status {
            mailbox,
            items: &items,
        };
        let mut found = None;
        let tagged = self
            .run_with(&command, StrayContinuation::Reject, |data| match data {
                Untagged::MailboxStatus(status)
                    if status.mailbox.as_str() == mailbox.as_str() && found.is_none() =>
                {
                    found = Some(status);
                    None
                }
                other => Some(other),
            })
            .await?;
        expect_ok("STATUS", tagged)?;
        found.ok_or(Error::Protocol("STATUS without data"))
    }
}
