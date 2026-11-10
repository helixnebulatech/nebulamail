// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! NOTIFY (RFC 5465).

use nebula_imap_proto::command::{NotifyGroup, NotifyNone, NotifySet};
use nebula_imap_proto::types::Capability;

use crate::Error;
use crate::core::client::{Client, LoggedIn};

impl<S: LoggedIn> Client<S> {
    /// `NOTIFY SET`: from now on the server also reports events of other
    /// mailboxes, as untagged `STATUS` (new, expunged and changed messages)
    /// and `LIST` (created, renamed, deleted, (un)subscribed mailboxes)
    /// responses. They arrive as events, during IDLE too
    /// ([`IdleEvent::Changes`](crate::IdleEvent)).
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] without NOTIFY; NO (`NOTIFICATIONOVERFLOW`)
    /// or BAD (`BADEVENT`: the server can't watch those events), or
    /// connection errors.
    pub async fn notify(&mut self, groups: &[NotifyGroup<'_>]) -> Result<(), Error> {
        if !self.session.has(&Capability::Notify) {
            return Err(Error::Unsupported("NOTIFY"));
        }
        self.run_simple(&NotifySet { groups }).await.map(|_| ())
    }

    /// `NOTIFY NONE`: stop the notifications.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] without NOTIFY, NO/BAD or connection errors.
    pub async fn notify_none(&mut self) -> Result<(), Error> {
        if !self.session.has(&Capability::Notify) {
            return Err(Error::Unsupported("NOTIFY"));
        }
        self.run_simple(&NotifyNone).await.map(|_| ())
    }
}
