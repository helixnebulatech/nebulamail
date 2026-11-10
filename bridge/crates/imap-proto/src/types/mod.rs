// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validated IMAP values: valid by construction ("parse, don't validate").

mod capability;
mod date;
mod flag;
mod mailbox;
mod number;
mod sequence;
mod special_use;
mod tag;
pub mod utf7;

pub use capability::{AuthMechanism, Capabilities, Capability};
pub use date::{ImapDate, InternalDate};
pub use flag::{Flag, Keyword};
pub use mailbox::{MailboxName, NameEncoding};
pub(crate) use number::parse_digits_u32;
pub use number::{ModSeq, SeqNum, Uid, UidValidity};
pub use sequence::{SeqBound, SeqItem, SeqSet, SequenceSet, SequenceValue, UidSet};
pub use special_use::{MailboxAttribute, SpecialUse};
pub use tag::{Tag, TagGenerator};

/// `ATOM-CHAR` of RFC 3501: any 7-bit char except controls, space and
/// `(){%*"\]`.
pub(crate) fn is_atom_char(byte: u8) -> bool {
    matches!(byte, 0x21..=0x7e)
        && !matches!(byte, b'(' | b')' | b'{' | b'%' | b'*' | b'"' | b'\\' | b']')
}

/// `ASTRING-CHAR`: an atom char or `]`.
pub(crate) fn is_astring_char(byte: u8) -> bool {
    is_atom_char(byte) || byte == b']'
}
