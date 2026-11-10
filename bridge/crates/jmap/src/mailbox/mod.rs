// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Mailbox (RFC 8621 §2): the IMAP folders of the account.

pub(crate) mod changes;
pub(crate) mod get;
pub(crate) mod query;
pub(crate) mod set;

use nebula_jmap_proto::object::{MailboxRights, MailboxRole};
use nebula_store::{Mailbox, Role};

/// The JMAP role of a stored role.
pub(crate) fn jmap_role(role: Role) -> Option<MailboxRole> {
    Some(match role {
        Role::Inbox => MailboxRole::Inbox,
        Role::Archive => MailboxRole::Archive,
        Role::Drafts => MailboxRole::Drafts,
        Role::Sent => MailboxRole::Sent,
        Role::Trash => MailboxRole::Trash,
        Role::Junk => MailboxRole::Junk,
        Role::All => MailboxRole::All,
        Role::Flagged => MailboxRole::Flagged,
        Role::Important => MailboxRole::Important,
        _ => return None,
    })
}

/// What IMAP lets the user do: a `\Noselect` folder holds no messages, a
/// flat namespace (NIL delimiter) or a `\Noinferiors` folder has no
/// children (RFC 9051 §7.3.1), and INBOX can't be renamed or deleted.
/// Finer rights (ACL, RFC 4314) are not read.
pub(crate) fn rights(mailbox: &Mailbox) -> MailboxRights {
    let items = mailbox.selectable;
    let inbox = mailbox.imap_name.is_inbox();
    MailboxRights {
        may_read_items: items,
        may_add_items: items,
        may_remove_items: items,
        may_set_seen: items,
        may_set_keywords: items,
        may_create_child: mailbox.delimiter.is_some() && !mailbox.no_inferiors,
        may_rename: !inbox,
        may_delete: !inbox,
        may_submit: true,
    }
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::types::MailboxName;
    use nebula_store::{MailboxId, MailboxSyncState};

    use super::*;

    fn mailbox(name: &str, delimiter: Option<char>, no_inferiors: bool) -> Mailbox {
        Mailbox {
            id: MailboxId::from_raw(1).unwrap(),
            imap_name: MailboxName::new(name).unwrap(),
            name: name.to_owned(),
            delimiter,
            parent: None,
            role: None,
            subscribed: true,
            selectable: true,
            no_inferiors,
            sort_order: 0,
            sync: MailboxSyncState::default(),
        }
    }

    /// `mayCreateChild` (RFC 8621 §2) is false for `\Noinferiors` (RFC
    /// 9051 §7.3.1) and for a flat namespace.
    #[test]
    fn may_create_child() {
        assert!(rights(&mailbox("Work", Some('/'), false)).may_create_child);
        assert!(!rights(&mailbox("Leaf", Some('/'), true)).may_create_child);
        assert!(!rights(&mailbox("Flat", None, false)).may_create_child);
    }
}
