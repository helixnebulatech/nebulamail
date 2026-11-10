// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed ManageSieve (RFC 5804) commands and responses, without I/O.
//!
//! - [`ScriptName`]: a validated script name (RFC 5804 §1.6: UTF-8, no
//!   control characters, no line or paragraph separators).
//! - [`Item`] and the line scanner: atoms, quoted strings (`"…"` with `\"`
//!   and `\\` only), literals (`{n}` from the server, `{n+}` from the
//!   client) and parenthesized lists, with strict limits ([`Limits`]): a
//!   hostile server can never make the decoder buffer more than one line
//!   plus one literal of the configured size.
//! - [`ResponseDecoder`]: incremental, cancel-safe; yields one
//!   [`ServerLine`] at a time: a data line (capabilities, a listed script,
//!   a script's content, a SASL challenge) or a completion (`OK`, `NO`,
//!   `BYE` with its [`ResponseCode`] and text).
//! - [`Capabilities`]: what a server announces (`IMPLEMENTATION`, `SASL`,
//!   `SIEVE`, `STARTTLS`, `MAXREDIRECTS`, `NOTIFY`, `VERSION`…).
//! - [`Command`]: every client command, encoded with literals where a
//!   quoted string can't hold the value; SASL data is zeroized on drop.
//!   [`CommandDecoder`] reads commands back (fake servers in tests, fuzzing).
//!
//! Nothing here panics on any input (fuzzed).

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// protocol crates use checked arithmetic only.
#![deny(clippy::arithmetic_side_effects)]

mod capability;
mod command;
mod error;
mod item;
mod limits;
mod listing;
mod name;
mod response;

pub use capability::Capabilities;
pub use command::{Command, CommandDecoder, Mechanism, SaslData};
pub use error::{DecodeError, NameError};
pub use item::{Atom, Item};
pub use limits::Limits;
pub use listing::ListedScript;
pub use name::ScriptName;
pub use response::{Completion, ResponseCode, ResponseDecoder, ServerLine, Status, one_line};

#[cfg(test)]
mod proptests;
