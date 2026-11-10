// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! LIST, extended when the server supports it.

use std::collections::{HashMap, HashSet};

use nebula_imap_proto::command::{List, ListPattern, ListReturn, Lsub, StatusItem};
use nebula_imap_proto::response::{ListEntry, MailboxStatus, Untagged};
use nebula_imap_proto::types::{Capability, MailboxAttribute};

use crate::Error;
use crate::core::client::{Client, LoggedIn};
use crate::core::connection::{StrayContinuation, expect_ok};

/// A mailbox from LIST, with its STATUS when the server returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListedMailbox {
    /// Name, delimiter and attributes (special use included).
    pub entry: ListEntry,
    /// STATUS data (`LIST-STATUS`), if asked for and available.
    pub status: Option<MailboxStatus>,
    /// Whether the user subscribed to it: `\Subscribed` from `RETURN
    /// (SUBSCRIBED)` with LIST-EXTENDED, else from a separate LSUB.
    pub subscribed: bool,
}

impl<S: LoggedIn> Client<S> {
    /// Lists mailboxes matching `pattern`.
    ///
    /// Asks for special-use attributes with `RETURN (SPECIAL-USE)` when the
    /// server has LIST-EXTENDED and SPECIAL-USE (servers with SPECIAL-USE
    /// alone send them anyway), and for `status_items` with `RETURN (STATUS
    /// ...)` when it has LIST-STATUS. Without LIST-STATUS, `status` is
    /// `None`: call [`Client::status`] per mailbox if needed.
    ///
    /// Subscriptions come with `RETURN (SUBSCRIBED)` (LIST-EXTENDED); without
    /// it, an `LSUB` with the same pattern follows.
    ///
    /// # Errors
    ///
    /// NO/BAD or connection errors.
    pub async fn list(
        &mut self,
        pattern: &ListPattern,
        status_items: &[StatusItem],
    ) -> Result<Vec<ListedMailbox>, Error> {
        let session = &self.session;
        let mut returns = Vec::new();
        let extended = session.has(&Capability::ListExtended);
        if extended {
            returns.push(ListReturn::Subscribed);
        }
        if extended && session.has(&Capability::SpecialUse) {
            returns.push(ListReturn::SpecialUse);
        }
        if session.has(&Capability::ListStatus) && !status_items.is_empty() {
            let condstore = session.has(&Capability::CondStore);
            let items = status_items
                .iter()
                .copied()
                .filter(|item| condstore || *item != StatusItem::HighestModSeq)
                .collect();
            returns.push(ListReturn::Status(items));
        }
        let command = List {
            pattern,
            returns: &returns,
        };
        let mut entries: Vec<ListedMailbox> = Vec::new();
        // Name → index in `entries`, so STATUS finds its LIST entry in
        // O(1) (a long list must not cost n × m).
        let mut by_name: HashMap<Box<str>, usize> = HashMap::new();
        let mut statuses: Vec<MailboxStatus> = Vec::new();
        let tagged = self
            .run_with(&command, StrayContinuation::Reject, |data| match data {
                Untagged::List(entry) => {
                    let subscribed = entry.attributes.contains(&MailboxAttribute::Subscribed);
                    by_name
                        .entry(entry.name.as_str().into())
                        .or_insert(entries.len());
                    entries.push(ListedMailbox {
                        entry,
                        status: None,
                        subscribed,
                    });
                    None
                }
                Untagged::MailboxStatus(status) => {
                    statuses.push(status);
                    None
                }
                other => Some(other),
            })
            .await?;
        expect_ok("LIST", tagged)?;
        if !extended {
            let subscribed = self.lsub(pattern).await?;
            for listed in &mut entries {
                listed.subscribed = subscribed.contains(listed.entry.name.as_str());
            }
        }
        for status in statuses {
            // A STATUS without its LIST entry is ignored.
            if let Some(listed) = by_name
                .get(status.mailbox.as_str())
                .and_then(|&index| entries.get_mut(index))
            {
                listed.status = Some(status);
            }
        }
        Ok(entries)
    }

    /// `LSUB "" pattern`: the names of the subscribed mailboxes.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] on an `IMAP4rev2`-only server, which has no
    /// LSUB (RFC 9051 Appendix E item 19; [`Self::list`] uses `RETURN
    /// (SUBSCRIBED)` there, as LIST-EXTENDED is part of `IMAP4rev2`). NO/BAD
    /// or connection errors.
    pub async fn lsub(&mut self, pattern: &ListPattern) -> Result<HashSet<String>, Error> {
        if self.session.capabilities.is_rev2_only() {
            return Err(Error::Unsupported("LSUB"));
        }
        let mut names = HashSet::new();
        let tagged = self
            .run_with(
                &Lsub { pattern },
                StrayContinuation::Reject,
                |data| match data {
                    // `imap-proto` reads LSUB lines as LIST data.
                    Untagged::List(entry) => {
                        names.insert(entry.name.as_str().to_owned());
                        None
                    }
                    other => Some(other),
                },
            )
            .await?;
        expect_ok("LSUB", tagged)?;
        Ok(names)
    }
}
