// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! IMAP commands as typed values (one file per command family).
//!
//! ```
//! use nebula_imap_proto::command::{self, EncodeOptions, Noop};
//! use nebula_imap_proto::types::TagGenerator;
//!
//! let mut tags = TagGenerator::new();
//! let encoded = command::encode(&Noop, tags.next_tag(), EncodeOptions::basic()).unwrap();
//! assert_eq!(encoded.chunks()[0].bytes(), b"A0001 NOOP\r\n");
//! ```

mod append;
mod auth;
mod copy_move;
mod enable;
mod fetch;
mod id;
mod list;
mod mailbox;
mod notify;
mod search;
mod select;
mod simple;
mod store;
mod writer;

pub use append::{Append, AppendData, AppendMessage, MultiAppend};
pub use auth::{
    Authenticate, InitialResponse, Login, SaslMechanism, oauthbearer_error_ack, sasl_cancel,
};
pub use copy_move::{UidCopy, UidExpunge, UidMove};
pub use enable::{Enable, EnableExtension};
pub use fetch::{FetchAttribute, FetchModifiers, HeaderName, PartPath, Partial, Section, UidFetch};
pub use id::Id;
pub use list::{List, ListPattern, ListReturn, Lsub};
pub use mailbox::{
    Create, Delete, GetQuotaRoot, Rename, Status, StatusItem, Subscribe, Unsubscribe,
};
pub use notify::{NotifyEvent, NotifyFilter, NotifyGroup, NotifyNone, NotifySet};
pub use search::{SearchKey, SearchReturn, UidSearch};
pub use select::{Access, QResyncParams, Select, SelectSync};
pub use simple::{Capability, Close, Idle, Logout, Namespace, Noop, StartTls, Unselect, idle_done};
pub use store::{StoreOperation, StoreResponse, UidStore, UnchangedSince};
pub use writer::{Chunk, ChunkEnd, EncodeOptions, EncodedCommand, LiteralMode, RawLine, Writer};

use crate::EncodeError;
use crate::types::{MailboxName, Tag};

mod sealed {
    pub trait Sealed {}
}

/// A command that can be encoded. Implemented by the types of this module.
pub trait Command: sealed::Sealed {
    /// The command name, safe to log (`LOGIN`, `UID FETCH`…).
    fn name(&self) -> &'static str;

    /// Writes the command after its tag.
    ///
    /// # Errors
    ///
    /// Arguments that cannot be sent safely.
    fn write(&self, writer: &mut Writer) -> Result<(), EncodeError>;
}

/// Encodes `command` with `tag`.
///
/// # Errors
///
/// Arguments that cannot be sent safely (NUL bytes, empty lists…).
pub fn encode(
    command: &impl Command,
    tag: Tag,
    options: EncodeOptions,
) -> Result<EncodedCommand, EncodeError> {
    let mut writer = Writer::new(options);
    writer.display(&tag);
    writer.space();
    command.write(&mut writer)?;
    Ok(writer.finish(tag, command.name()))
}

/// Writes a mailbox name in the connection's encoding.
fn write_mailbox(writer: &mut Writer, mailbox: &MailboxName) -> Result<(), EncodeError> {
    let wire = mailbox.to_wire(writer.options().names);
    writer.astring(wire.as_bytes(), "mailbox name")
}

/// Implements [`Command`] and the seal for a type.
macro_rules! command {
    ($type:ty, $name:literal, |$self:ident, $writer:ident| $body:block) => {
        impl crate::command::sealed::Sealed for $type {}
        impl crate::command::Command for $type {
            fn name(&self) -> &'static str {
                $name
            }
            fn write(
                &self,
                $writer: &mut crate::command::Writer,
            ) -> Result<(), crate::EncodeError> {
                let $self = self;
                $body
            }
        }
    };
}
pub(crate) use command;

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    /// Encodes and joins the chunks, marking waits with `<WAIT>`.
    pub(crate) fn wire(command: &impl Command, options: EncodeOptions) -> String {
        let encoded = encode(command, Tag::new("A1").unwrap(), options).unwrap();
        let mut out = Vec::new();
        for chunk in encoded.chunks() {
            out.extend_from_slice(chunk.bytes());
            if chunk.end() == ChunkEnd::AwaitContinuation {
                out.extend_from_slice(b"<WAIT>");
            }
        }
        String::from_utf8(out).unwrap()
    }

    pub(crate) fn basic(command: &impl Command) -> String {
        wire(command, EncodeOptions::basic())
    }
}
