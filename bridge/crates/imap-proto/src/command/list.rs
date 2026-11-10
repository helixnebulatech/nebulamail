// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `LIST`, plain (RFC 3501) or extended (RFC 5258, 5819, 6154).

use super::command;
use super::mailbox::{StatusItem, write_status_items};
use crate::types::NameEncoding;
use crate::types::utf7;
use crate::{EncodeError, ValueError};

/// A LIST pattern such as `*` or `Work/%`, in UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPattern(Box<str>);

impl ListPattern {
    /// Every mailbox.
    #[must_use]
    pub fn all() -> Self {
        Self("*".into())
    }

    /// Validates a pattern (no CR, LF or NUL).
    ///
    /// # Errors
    ///
    /// Empty patterns or patterns with control line characters.
    pub fn new(pattern: &str) -> Result<Self, ValueError> {
        if pattern.is_empty() {
            return Err(ValueError::Empty("LIST pattern"));
        }
        if pattern.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
            return Err(ValueError::Invalid("LIST pattern"));
        }
        Ok(Self(pattern.into()))
    }

    fn to_wire(&self, names: NameEncoding) -> String {
        match names {
            NameEncoding::Utf8 => self.0.to_string(),
            // `*` and `%` are printable ASCII, so they stay as they are.
            NameEncoding::ModifiedUtf7 => utf7::encode(&self.0),
        }
    }
}

/// A `RETURN` option of extended LIST.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ListReturn {
    /// `SPECIAL-USE` (RFC 6154).
    SpecialUse,
    /// `SUBSCRIBED` (RFC 5258).
    Subscribed,
    /// `CHILDREN` (RFC 5258).
    Children,
    /// `STATUS (...)` (RFC 5819, `LIST-STATUS`).
    Status(Vec<StatusItem>),
}

/// `LIST "" pattern [RETURN (...)]`. The reference is always empty: patterns
/// are relative to the root.
///
/// Only send `RETURN` options the server supports (`LIST-EXTENDED`,
/// `SPECIAL-USE`, `LIST-STATUS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List<'a> {
    /// What to list.
    pub pattern: &'a ListPattern,
    /// Extra data to return; empty for plain LIST.
    pub returns: &'a [ListReturn],
}

command!(List<'_>, "LIST", |list, writer| {
    writer.raw(b"LIST \"\" ");
    let wire = list.pattern.to_wire(writer.options().names);
    writer.string(wire.as_bytes(), "LIST pattern")?;
    if !list.returns.is_empty() {
        writer.raw(b" RETURN (");
        for (index, option) in list.returns.iter().enumerate() {
            if index > 0 {
                writer.space();
            }
            write_return(writer, option)?;
        }
        writer.raw(b")");
    }
    Ok(())
});

/// `LSUB "" pattern`: the subscribed mailboxes (RFC 3501; for servers
/// without LIST-EXTENDED, which can't answer `RETURN (SUBSCRIBED)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lsub<'a> {
    /// What to list.
    pub pattern: &'a ListPattern,
}

command!(Lsub<'_>, "LSUB", |lsub, writer| {
    writer.raw(b"LSUB \"\" ");
    let wire = lsub.pattern.to_wire(writer.options().names);
    writer.string(wire.as_bytes(), "LSUB pattern")
});

fn write_return(writer: &mut super::Writer, option: &ListReturn) -> Result<(), EncodeError> {
    match option {
        ListReturn::SpecialUse => writer.raw(b"SPECIAL-USE"),
        ListReturn::Subscribed => writer.raw(b"SUBSCRIBED"),
        ListReturn::Children => writer.raw(b"CHILDREN"),
        ListReturn::Status(items) => {
            writer.raw(b"STATUS ");
            write_status_items(writer, items)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;

    #[test]
    fn encodes() {
        let all = ListPattern::all();
        assert_eq!(
            basic(&List {
                pattern: &all,
                returns: &[]
            }),
            "A1 LIST \"\" \"*\"\r\n"
        );
        let returns = [
            ListReturn::SpecialUse,
            ListReturn::Status(vec![StatusItem::Messages, StatusItem::Unseen]),
        ];
        assert_eq!(
            basic(&List {
                pattern: &all,
                returns: &returns
            }),
            "A1 LIST \"\" \"*\" RETURN (SPECIAL-USE STATUS (MESSAGES UNSEEN))\r\n"
        );
        assert!(ListPattern::new("a\nb").is_err());
        assert_eq!(basic(&Lsub { pattern: &all }), "A1 LSUB \"\" \"*\"\r\n");
    }
}
