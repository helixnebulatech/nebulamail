// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `UID STORE` with `UNCHANGEDSINCE` (RFC 7162).

use super::{Writer, command};
use crate::types::{Flag, ModSeq, UidSet};
use crate::{EncodeError, ValueError};

/// How the flags change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StoreOperation {
    /// `+FLAGS`: add these flags.
    Add,
    /// `-FLAGS`: remove these flags.
    Remove,
    /// `FLAGS`: replace all flags.
    Replace,
}

/// Whether the server answers with the new flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StoreResponse {
    /// `.SILENT`: no FETCH responses (other clients' changes still come).
    Silent,
    /// FETCH responses with the new flags (and MODSEQ with CONDSTORE).
    Fetch,
}

/// The `UNCHANGEDSINCE` value: a mod-sequence or 0 (RFC 7162
/// `mod-sequence-valzer`; 0 means "only if never changed").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnchangedSince(u64);

impl UnchangedSince {
    /// Validates the value (0 ..= 2^63 - 1).
    ///
    /// # Errors
    ///
    /// Values above 2^63 - 1.
    pub fn new(value: u64) -> Result<Self, ValueError> {
        if value > ModSeq::MAX_VALUE {
            return Err(ValueError::OutOfRange("UNCHANGEDSINCE"));
        }
        Ok(Self(value))
    }

    /// The value.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<ModSeq> for UnchangedSince {
    fn from(value: ModSeq) -> Self {
        Self(value.get())
    }
}

/// `UID STORE set [(UNCHANGEDSINCE n)] (+|-)FLAGS[.SILENT] (flags)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UidStore<'a> {
    /// Which messages.
    pub uids: &'a UidSet,
    /// Add, remove or replace.
    pub operation: StoreOperation,
    /// The flags (may be empty only for `Replace`).
    pub flags: &'a [Flag],
    /// Silent or not.
    pub response: StoreResponse,
    /// Conditional store (CONDSTORE).
    pub unchanged_since: Option<UnchangedSince>,
}

command!(UidStore<'_>, "UID STORE", |store, writer| {
    if store.flags.is_empty() && store.operation != StoreOperation::Replace {
        return Err(EncodeError::Empty("STORE flags"));
    }
    writer.raw(b"UID STORE ");
    writer.display(store.uids);
    if let Some(unchanged_since) = store.unchanged_since {
        writer.raw(b" (UNCHANGEDSINCE ");
        writer.number(unchanged_since.get());
        writer.raw(b")");
    }
    writer.raw(match store.operation {
        StoreOperation::Add => b" +FLAGS",
        StoreOperation::Remove => b" -FLAGS",
        StoreOperation::Replace => b" FLAGS",
    });
    if store.response == StoreResponse::Silent {
        writer.raw(b".SILENT");
    }
    writer.space();
    write_flag_list(writer, store.flags)
});

/// Writes `(\Seen $Junk)`. `\Recent` is refused: clients cannot set it.
pub(crate) fn write_flag_list(writer: &mut Writer, flags: &[Flag]) -> Result<(), EncodeError> {
    writer.raw(b"(");
    for (index, flag) in flags.iter().enumerate() {
        if !flag.is_storable() {
            return Err(EncodeError::FlagNotStorable(flag.as_wire()));
        }
        if index > 0 {
            writer.space();
        }
        // System flags and validated keywords are atoms.
        writer.display(flag);
    }
    writer.raw(b")");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{EncodeOptions, Tag, encode, test_util::basic};
    use crate::types::Uid;

    #[test]
    fn encodes() {
        let uids = UidSet::single(Uid::new(7).unwrap());
        let store = UidStore {
            uids: &uids,
            operation: StoreOperation::Add,
            flags: &[Flag::Seen, Flag::parse("$Junk").unwrap()],
            response: StoreResponse::Silent,
            unchanged_since: Some(UnchangedSince::new(12).unwrap()),
        };
        assert_eq!(
            basic(&store),
            "A1 UID STORE 7 (UNCHANGEDSINCE 12) +FLAGS.SILENT (\\Seen $Junk)\r\n"
        );
        let store = UidStore {
            flags: &[],
            operation: StoreOperation::Replace,
            unchanged_since: None,
            response: StoreResponse::Fetch,
            ..store
        };
        assert_eq!(basic(&store), "A1 UID STORE 7 FLAGS ()\r\n");
    }

    #[test]
    fn refuses_recent() {
        let uids = UidSet::single(Uid::new(7).unwrap());
        let store = UidStore {
            uids: &uids,
            operation: StoreOperation::Add,
            flags: &[Flag::Recent],
            response: StoreResponse::Silent,
            unchanged_since: None,
        };
        let result = encode(&store, Tag::new("A1").unwrap(), EncodeOptions::basic());
        assert!(matches!(result, Err(EncodeError::FlagNotStorable(_))));
    }
}
