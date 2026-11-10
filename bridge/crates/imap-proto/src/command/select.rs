// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `SELECT` and `EXAMINE`, with CONDSTORE and QRESYNC (RFC 7162).

use super::write_mailbox;
use crate::types::{MailboxName, ModSeq, UidSet, UidValidity};

/// Read-write (`SELECT`) or read-only (`EXAMINE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Access {
    /// `SELECT`
    ReadWrite,
    /// `EXAMINE`
    ReadOnly,
}

/// What we already know about the mailbox, for a quick resync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QResyncParams {
    /// The UIDVALIDITY we saw last time.
    pub uid_validity: UidValidity,
    /// The HIGHESTMODSEQ we saw last time.
    pub mod_seq: ModSeq,
    /// The UIDs we know, so the server only reports those that vanished.
    pub known_uids: Option<UidSet>,
}

/// Synchronisation extensions for SELECT.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectSync {
    /// Plain SELECT.
    Plain,
    /// `(CONDSTORE)`: ask for HIGHESTMODSEQ.
    CondStore,
    /// `(QRESYNC (...))`: needs `ENABLE QRESYNC` first.
    QResync(QResyncParams),
}

/// `SELECT name [(CONDSTORE | QRESYNC (...))]`, or `EXAMINE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Select<'a> {
    /// The mailbox.
    pub mailbox: &'a MailboxName,
    /// SELECT or EXAMINE.
    pub access: Access,
    /// CONDSTORE / QRESYNC parameters.
    pub sync: SelectSync,
}

impl super::sealed::Sealed for Select<'_> {}
impl super::Command for Select<'_> {
    fn name(&self) -> &'static str {
        match self.access {
            Access::ReadWrite => "SELECT",
            Access::ReadOnly => "EXAMINE",
        }
    }

    fn write(&self, writer: &mut super::Writer) -> Result<(), crate::EncodeError> {
        writer.raw(self.name().as_bytes());
        writer.space();
        write_mailbox(writer, self.mailbox)?;
        match &self.sync {
            SelectSync::Plain => {}
            SelectSync::CondStore => writer.raw(b" (CONDSTORE)"),
            SelectSync::QResync(params) => {
                writer.raw(b" (QRESYNC (");
                writer.number(params.uid_validity.get());
                writer.space();
                writer.number(params.mod_seq.get());
                if let Some(known) = &params.known_uids {
                    writer.space();
                    writer.display(known);
                }
                writer.raw(b"))");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;
    use crate::types::Uid;

    #[test]
    fn encodes() {
        let inbox = MailboxName::inbox();
        let select = Select {
            mailbox: &inbox,
            access: Access::ReadWrite,
            sync: SelectSync::CondStore,
        };
        assert_eq!(basic(&select), "A1 SELECT INBOX (CONDSTORE)\r\n");
        let params = QResyncParams {
            uid_validity: UidValidity::new(67_890_007).unwrap(),
            mod_seq: ModSeq::new(90_060_115_194_045_000).unwrap(),
            known_uids: Some(UidSet::range(Uid::new(41).unwrap(), Uid::new(211).unwrap())),
        };
        let select = Select {
            mailbox: &inbox,
            access: Access::ReadOnly,
            sync: SelectSync::QResync(params),
        };
        assert_eq!(
            basic(&select),
            "A1 EXAMINE INBOX (QRESYNC (67890007 90060115194045000 41:211))\r\n"
        );
    }
}
