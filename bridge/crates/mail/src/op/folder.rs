// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Creating, renaming, deleting and (un)subscribing mailboxes
//! (`Mailbox/set`). After each change the mailbox list is synced again, so
//! the store matches what the server really did.

use nebula_imap::Error as ImapError;
use nebula_imap::proto::command::StatusItem;
use nebula_imap::proto::types::MailboxName;
use nebula_store::{Mailbox, MailboxId, Recorded, Role, UnixTime};

use crate::MailError;
use crate::core::account::MailAccount;
use crate::sync::list::sync_list;

/// The hierarchy delimiter to use for new names: the server's (from any
/// mailbox), `/` if none is known.
fn delimiter(mailboxes: &[Mailbox]) -> Option<char> {
    mailboxes
        .iter()
        .find(|m| m.role == Some(Role::Inbox))
        .or_else(|| mailboxes.first())
        .and_then(|m| m.delimiter)
}

/// The personal namespace prefix (`INBOX.`) as seen in the stored names:
/// a top-level mailbox whose full name is longer than its display name.
fn prefix(mailboxes: &[Mailbox]) -> String {
    mailboxes
        .iter()
        .filter(|m| m.parent.is_none() && m.role != Some(Role::Inbox))
        .find_map(|m| m.imap_name.as_str().strip_suffix(m.name.as_str()))
        .unwrap_or("")
        .to_owned()
}

/// The full IMAP name for `name` under `parent` (or at the top).
fn full_name(
    mailboxes: &[Mailbox],
    name: &str,
    parent: Option<&Mailbox>,
) -> Result<MailboxName, MailError> {
    let delimiter = parent
        .and_then(|p| p.delimiter)
        .or_else(|| delimiter(mailboxes));
    let bad_char = |c: char| c.is_control() || Some(c) == delimiter;
    if name.trim().is_empty() || name.chars().any(bad_char) || name.chars().count() > 255 {
        return Err(MailError::Invalid("mailbox name"));
    }
    let invalid = |_| MailError::Invalid("mailbox name");
    match (parent, delimiter) {
        // Keeps the form of the parent: a parent the server sent in
        // invalid UTF-7 is kept raw, and so must its child be, or it would
        // name a mailbox below another parent.
        // RFC 9051 §7.3.1: "It is not possible for any child levels of
        // hierarchy to exist under this name".
        (Some(parent), _) if parent.no_inferiors => {
            Err(MailError::Unsupported("folders inside this folder"))
        }
        (Some(parent), Some(delimiter)) => parent.imap_name.child(delimiter, name).map_err(invalid),
        (Some(_), None) => Err(MailError::Unsupported("folders inside folders")),
        (None, _) => MailboxName::new(&format!("{}{name}", prefix(mailboxes))).map_err(invalid),
    }
}

impl MailAccount {
    async fn parent(&self, parent: Option<MailboxId>) -> Result<Option<Mailbox>, MailError> {
        match parent {
            Some(id) => Ok(Some(self.store().mailbox(self.id, id).await?)),
            None => Ok(None),
        }
    }

    /// Creates a mailbox and returns its id.
    ///
    /// # Errors
    /// [`MailError::Invalid`] for a bad name, [`MailError::Refused`] if the
    /// server says no (the name exists…), and connection errors.
    pub async fn create_mailbox(
        &self,
        name: &str,
        parent: Option<MailboxId>,
    ) -> Result<MailboxId, MailError> {
        let mut mailboxes = self.store().mailboxes(self.id).await?;
        // Right after the first sign-in nothing is known yet, and the name
        // depends on the server's separator and prefix.
        if mailboxes.is_empty() {
            sync_list(self).await?;
            mailboxes = self.store().mailboxes(self.id).await?;
        }
        let parent = self.parent(parent).await?;
        let full = full_name(&mailboxes, name, parent.as_ref())?;
        let mut slot = self.slot().await;
        let session = slot.session(self).await?;
        let result = session.create(&full, None).await;
        slot.check(result)?;
        drop(slot);
        sync_list(self).await?;
        self.store()
            .mailbox_by_name(self.id, &full)
            .await?
            .map(|m| m.id)
            .ok_or(MailError::NotFound("created mailbox"))
    }

    /// Renames and/or moves a mailbox, keeping its id (and its children's).
    ///
    /// # Errors
    /// [`MailError::Invalid`] for INBOX or a bad name, [`MailError::Refused`]
    /// if the server says no, and connection errors.
    pub async fn rename_mailbox(
        &self,
        id: MailboxId,
        name: &str,
        parent: Option<MailboxId>,
    ) -> Result<Recorded, MailError> {
        let mailboxes = self.store().mailboxes(self.id).await?;
        let current = self.store().mailbox(self.id, id).await?;
        if current.imap_name.is_inbox() {
            // IMAP's RENAME of INBOX moves its messages instead.
            return Err(MailError::Invalid("INBOX can't be renamed"));
        }
        let parent = self.parent(parent).await?;
        if parent.as_ref().is_some_and(|p| {
            p.id == id || is_below(&p.imap_name, &current.imap_name, current.delimiter)
        }) {
            return Err(MailError::Invalid("a mailbox can't go inside itself"));
        }
        let full = full_name(&mailboxes, name, parent.as_ref())?;
        // Same visible name: nothing to do (the derived equality also
        // compares the raw/decoded form, which may differ for one name).
        if full.as_str() == current.imap_name.as_str() {
            return Ok(Recorded::default());
        }
        let mut slot = self.slot().await;
        slot.leave(&current.imap_name).await?;
        let session = slot.session(self).await?;
        let result = session.rename(&current.imap_name, &full).await;
        slot.check(result)?;
        // The store learns the new name before anyone else uses the
        // connection: a list sync in between would see the old name gone
        // and drop the mailbox (and its copies) instead of renaming it.
        let mut recorded = self
            .store()
            .rename_mailbox(self.id, id, full, name.to_owned(), UnixTime::now()?)
            .await?;
        drop(slot);
        self.emit(&recorded);
        recorded.merge(sync_list(self).await?.recorded);
        Ok(recorded)
    }

    /// Deletes a mailbox on the server. With [`OnDestroy::RemoveEmails`]
    /// its emails go with it; with [`OnDestroy::KeepEmails`] the server is
    /// asked first (`STATUS MESSAGES`, with the connection held until the
    /// DELETE) and a mailbox with any message, synced or not, is kept.
    ///
    /// IMAP has no conditional DELETE: a message that arrives between the
    /// STATUS and the DELETE goes with the mailbox (an accepted, tiny
    /// window).
    ///
    /// # Errors
    /// [`MailError::Invalid`] for INBOX, [`MailError::MailboxHasEmail`] if
    /// it holds messages and they must be kept, [`MailError::Refused`] if
    /// the server says no (it has children…), and connection errors.
    pub async fn delete_mailbox(
        &self,
        id: MailboxId,
        emails: OnDestroy,
    ) -> Result<Recorded, MailError> {
        let current = self.store().mailbox(self.id, id).await?;
        if current.imap_name.is_inbox() {
            return Err(MailError::Invalid("INBOX can't be deleted"));
        }
        let mut slot = self.slot().await;
        // Left first: STATUS of the selected mailbox is not reliable.
        slot.leave(&current.imap_name).await?;
        let session = slot.session(self).await?;
        if emails == OnDestroy::KeepEmails {
            let result = session
                .status(&current.imap_name, &[StatusItem::Messages])
                .await;
            match result {
                // A `\Noselect` mailbox holds no messages, and servers
                // may refuse STATUS on it. Only then is a NO accepted: the
                // store may lag behind, so it is not a reason to skip the
                // STATUS.
                Err(ImapError::Refused(_)) if !current.selectable => {}
                other => {
                    let status = slot.check(other)?;
                    if holds_email(status.messages)? {
                        return Err(MailError::MailboxHasEmail);
                    }
                }
            }
        }
        let session = slot.session(self).await?;
        let result = session.delete(&current.imap_name).await;
        slot.check(result)?;
        drop(slot);
        Ok(sync_list(self).await?.recorded)
    }

    /// Subscribes to or unsubscribes from a mailbox.
    ///
    /// # Errors
    /// [`MailError::Refused`] if the server says no, and connection errors.
    pub async fn set_subscribed(
        &self,
        id: MailboxId,
        subscribed: bool,
    ) -> Result<Recorded, MailError> {
        let current = self.store().mailbox(self.id, id).await?;
        let mut slot = self.slot().await;
        let session = slot.session(self).await?;
        let result = session.subscribe(&current.imap_name, subscribed).await;
        slot.check(result)?;
        // Stored before letting go of the connection, as in `rename_mailbox`.
        let recorded = self
            .store()
            .set_mailbox_subscribed(self.id, id, subscribed, UnixTime::now()?)
            .await?;
        drop(slot);
        self.emit(&recorded);
        Ok(recorded)
    }
}

/// What happens to the emails of a deleted mailbox (JMAP
/// `onDestroyRemoveEmails`). A type, not a `bool`: losing mail is a
/// decision that must read as one at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDestroy {
    /// Refuse to delete a mailbox that still has messages on the server.
    KeepEmails,
    /// Delete it with its messages.
    RemoveEmails,
}

/// Whether the `MESSAGES` of a STATUS says the mailbox holds mail. Messages
/// marked `\Deleted` count: they are still there. No `MESSAGES` at all
/// (a broken server) is an error, not a yes or a no.
fn holds_email(messages: Option<u32>) -> Result<bool, MailError> {
    messages
        .map(|count| count > 0)
        .ok_or(MailError::from(ImapError::Protocol(
            "STATUS without MESSAGES",
        )))
}

/// Whether `name` is below `ancestor` in the hierarchy.
fn is_below(name: &MailboxName, ancestor: &MailboxName, delimiter: Option<char>) -> bool {
    let Some(delimiter) = delimiter else {
        return false;
    };
    name.as_str()
        .strip_prefix(ancestor.as_str())
        .is_some_and(|rest| rest.starts_with(delimiter))
}

#[cfg(test)]
mod tests {
    use nebula_store::MailboxSyncState;

    use super::*;

    #[test]
    fn status_messages_decide() {
        assert!(!holds_email(Some(0)).unwrap());
        assert!(holds_email(Some(1)).unwrap());
        assert!(matches!(holds_email(None), Err(MailError::Unavailable(_))));
    }

    fn mailbox(id: i64, imap: &str, name: &str, delimiter: char, role: Option<Role>) -> Mailbox {
        Mailbox {
            id: MailboxId::from_raw(id).unwrap(),
            imap_name: MailboxName::new(imap).unwrap(),
            name: name.to_owned(),
            delimiter: Some(delimiter),
            parent: None,
            role,
            subscribed: true,
            selectable: true,
            no_inferiors: false,
            sort_order: 0,
            sync: MailboxSyncState::default(),
        }
    }

    #[test]
    fn names_follow_the_server() {
        let plain = vec![
            mailbox(1, "INBOX", "Inbox", '/', Some(Role::Inbox)),
            mailbox(2, "Work", "Work", '/', None),
        ];
        let work = plain.get(1);
        assert_eq!(
            full_name(&plain, "2026", work).unwrap().as_str(),
            "Work/2026"
        );
        assert_eq!(full_name(&plain, "Nuevo", None).unwrap().as_str(), "Nuevo");
        assert!(full_name(&plain, "a/b", None).is_err());
        assert!(full_name(&plain, " ", None).is_err());
        assert!(full_name(&plain, "a\r\nb", None).is_err());

        let prefixed = vec![
            mailbox(1, "INBOX", "Inbox", '.', Some(Role::Inbox)),
            mailbox(2, "INBOX.Sent", "Sent", '.', Some(Role::Sent)),
        ];
        assert_eq!(
            full_name(&prefixed, "Nuevo", None).unwrap().as_str(),
            "INBOX.Nuevo"
        );
    }

    /// RFC 9051 §7.3.1: no child below a `\Noinferiors` mailbox.
    #[test]
    fn no_child_below_no_inferiors() {
        let mut leaf = mailbox(2, "Leaf", "Leaf", '/', None);
        leaf.no_inferiors = true;
        let mailboxes = vec![mailbox(1, "INBOX", "Inbox", '/', Some(Role::Inbox)), leaf];
        assert!(matches!(
            full_name(&mailboxes, "child", mailboxes.get(1)),
            Err(MailError::Unsupported(_))
        ));
        assert!(full_name(&mailboxes, "child", mailboxes.first()).is_ok());
    }

    #[test]
    fn child_of_raw_wire_parent_stays_raw() {
        use nebula_imap::proto::types::NameEncoding;
        let mut raw = mailbox(2, "x", "A&B", '/', None);
        raw.imap_name = MailboxName::from_wire("A&B", NameEncoding::ModifiedUtf7).unwrap();
        let mailboxes = vec![mailbox(1, "INBOX", "Inbox", '/', Some(Role::Inbox)), raw];
        let child = full_name(&mailboxes, "child", mailboxes.get(1)).unwrap();
        assert_eq!(child.to_wire(NameEncoding::ModifiedUtf7), "A&B/child");
        let decoded = mailbox(3, "café", "café", '/', None);
        let child = full_name(&mailboxes, "x", Some(&decoded)).unwrap();
        assert_eq!(child.to_wire(NameEncoding::ModifiedUtf7), "caf&AOk-/x");
    }

    #[test]
    fn rename_same_name_on_raw_is_noop() {
        use nebula_imap::proto::types::NameEncoding;
        // A raw-wire name and a decoded one with the same text: the same
        // visible name, so a rename to it changes nothing.
        let raw = MailboxName::from_wire("A&B", NameEncoding::ModifiedUtf7).unwrap();
        let decoded = MailboxName::new("A&B").unwrap();
        assert_ne!(raw, decoded);
        assert_eq!(raw.as_str(), decoded.as_str());
    }

    #[test]
    fn inside_itself() {
        let name = |n| MailboxName::new(n).unwrap();
        assert!(is_below(&name("A/B"), &name("A"), Some('/')));
        assert!(!is_below(&name("AB"), &name("A"), Some('/')));
        assert!(!is_below(&name("A"), &name("A"), Some('/')));
    }
}
