// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `UID COPY`, `UID MOVE` (RFC 6851) and `UID EXPUNGE` (RFC 4315).

use super::{command, write_mailbox};
use crate::types::{MailboxName, UidSet};

/// `UID COPY set mailbox`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidCopy<'a> {
    /// Which messages.
    pub uids: &'a UidSet,
    /// Destination.
    pub mailbox: &'a MailboxName,
}

command!(UidCopy<'_>, "UID COPY", |copy, writer| {
    writer.raw(b"UID COPY ");
    writer.display(copy.uids);
    writer.space();
    write_mailbox(writer, copy.mailbox)
});

/// `UID MOVE set mailbox` (needs `MOVE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidMove<'a> {
    /// Which messages.
    pub uids: &'a UidSet,
    /// Destination.
    pub mailbox: &'a MailboxName,
}

command!(UidMove<'_>, "UID MOVE", |move_command, writer| {
    writer.raw(b"UID MOVE ");
    writer.display(move_command.uids);
    writer.space();
    write_mailbox(writer, move_command.mailbox)
});

/// `UID EXPUNGE set` (needs `UIDPLUS`): expunges only these `\Deleted`
/// messages, unlike plain EXPUNGE which removes every `\Deleted` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidExpunge<'a> {
    /// Which messages.
    pub uids: &'a UidSet,
}

command!(UidExpunge<'_>, "UID EXPUNGE", |expunge, writer| {
    writer.raw(b"UID EXPUNGE ");
    writer.display(expunge.uids);
    Ok(())
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;
    use crate::types::Uid;

    #[test]
    fn encodes() {
        let uids = UidSet::from_values([1, 2, 3, 9].map(|v| Uid::new(v).unwrap())).unwrap();
        let trash = MailboxName::new("Papelera").unwrap();
        assert_eq!(
            basic(&UidMove {
                uids: &uids,
                mailbox: &trash
            }),
            "A1 UID MOVE 1:3,9 Papelera\r\n"
        );
        assert_eq!(
            basic(&UidCopy {
                uids: &uids,
                mailbox: &trash
            }),
            "A1 UID COPY 1:3,9 Papelera\r\n"
        );
        assert_eq!(
            basic(&UidExpunge { uids: &uids }),
            "A1 UID EXPUNGE 1:3,9\r\n"
        );
    }
}
