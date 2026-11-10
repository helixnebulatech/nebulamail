// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Commands on mailboxes: CREATE, DELETE, RENAME, (UN)SUBSCRIBE, STATUS,
//! GETQUOTAROOT.

use super::{command, write_mailbox};
use crate::EncodeError;
use crate::types::{MailboxName, SpecialUse};

/// `CREATE name [(USE (\Sent))]`. The special use needs `CREATE-SPECIAL-USE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Create<'a> {
    /// The new mailbox.
    pub mailbox: &'a MailboxName,
    /// Optional special use (RFC 6154 §3).
    pub special_use: Option<SpecialUse>,
}

command!(Create<'_>, "CREATE", |create, writer| {
    writer.raw(b"CREATE ");
    write_mailbox(writer, create.mailbox)?;
    if let Some(special_use) = create.special_use {
        writer.raw(b" (USE (");
        writer.raw(special_use.as_wire().as_bytes());
        writer.raw(b"))");
    }
    Ok(())
});

/// `DELETE name`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delete<'a> {
    /// The mailbox to delete.
    pub mailbox: &'a MailboxName,
}

command!(Delete<'_>, "DELETE", |delete, writer| {
    writer.raw(b"DELETE ");
    write_mailbox(writer, delete.mailbox)
});

/// `RENAME from to`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename<'a> {
    /// Current name.
    pub from: &'a MailboxName,
    /// New name.
    pub to: &'a MailboxName,
}

command!(Rename<'_>, "RENAME", |rename, writer| {
    writer.raw(b"RENAME ");
    write_mailbox(writer, rename.from)?;
    writer.space();
    write_mailbox(writer, rename.to)
});

/// `SUBSCRIBE name`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscribe<'a> {
    /// The mailbox.
    pub mailbox: &'a MailboxName,
}

command!(Subscribe<'_>, "SUBSCRIBE", |subscribe, writer| {
    writer.raw(b"SUBSCRIBE ");
    write_mailbox(writer, subscribe.mailbox)
});

/// `UNSUBSCRIBE name`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsubscribe<'a> {
    /// The mailbox.
    pub mailbox: &'a MailboxName,
}

command!(Unsubscribe<'_>, "UNSUBSCRIBE", |unsubscribe, writer| {
    writer.raw(b"UNSUBSCRIBE ");
    write_mailbox(writer, unsubscribe.mailbox)
});

/// A STATUS data item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StatusItem {
    /// `MESSAGES`
    Messages,
    /// `RECENT`
    Recent,
    /// `UIDNEXT`
    UidNext,
    /// `UIDVALIDITY`
    UidValidity,
    /// `UNSEEN`
    Unseen,
    /// `HIGHESTMODSEQ` (CONDSTORE).
    HighestModSeq,
}

impl StatusItem {
    /// The name on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "MESSAGES",
            Self::Recent => "RECENT",
            Self::UidNext => "UIDNEXT",
            Self::UidValidity => "UIDVALIDITY",
            Self::Unseen => "UNSEEN",
            Self::HighestModSeq => "HIGHESTMODSEQ",
        }
    }
}

/// Writes `(MESSAGES UIDNEXT ...)`.
pub(crate) fn write_status_items(
    writer: &mut super::Writer,
    items: &[StatusItem],
) -> Result<(), EncodeError> {
    if items.is_empty() {
        return Err(EncodeError::Empty("STATUS items"));
    }
    writer.raw(b"(");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            writer.space();
        }
        writer.raw(item.as_str().as_bytes());
    }
    writer.raw(b")");
    Ok(())
}

/// `STATUS name (items)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status<'a> {
    /// The mailbox.
    pub mailbox: &'a MailboxName,
    /// At least one item.
    pub items: &'a [StatusItem],
}

command!(Status<'_>, "STATUS", |status, writer| {
    writer.raw(b"STATUS ");
    write_mailbox(writer, status.mailbox)?;
    writer.space();
    write_status_items(writer, status.items)
});

/// `GETQUOTAROOT name` (RFC 9208).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetQuotaRoot<'a> {
    /// The mailbox.
    pub mailbox: &'a MailboxName,
}

command!(GetQuotaRoot<'_>, "GETQUOTAROOT", |quota, writer| {
    writer.raw(b"GETQUOTAROOT ");
    write_mailbox(writer, quota.mailbox)
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;

    #[test]
    fn encodes() {
        let sent = MailboxName::new("Enviados é").unwrap();
        let create = Create {
            mailbox: &sent,
            special_use: Some(SpecialUse::Sent),
        };
        assert_eq!(
            basic(&create),
            "A1 CREATE \"Enviados &AOk-\" (USE (\\Sent))\r\n"
        );
        let inbox = MailboxName::inbox();
        let status = Status {
            mailbox: &inbox,
            items: &[StatusItem::Messages, StatusItem::HighestModSeq],
        };
        assert_eq!(
            basic(&status),
            "A1 STATUS INBOX (MESSAGES HIGHESTMODSEQ)\r\n"
        );
        let rename = Rename {
            from: &inbox,
            to: &sent,
        };
        assert_eq!(basic(&rename), "A1 RENAME INBOX \"Enviados &AOk-\"\r\n");
        assert_eq!(
            basic(&GetQuotaRoot { mailbox: &inbox }),
            "A1 GETQUOTAROOT INBOX\r\n"
        );
    }
}
