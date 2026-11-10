// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed IMAP client commands and responses, without I/O.
//!
//! - [`types`]: validated values (`Uid`, `UidValidity`, `ModSeq`, sequence
//!   sets, mailbox names with modified UTF-7, flags, capabilities…).
//! - [`command`]: commands as typed values that encode to bytes. The encoded
//!   form is a list of chunks, so the client knows where it must wait for a
//!   continuation (`+`) from the server. CR, LF and NUL can never end up in a
//!   command line outside a literal.
//! - [`response`]: a framing decoder with limits that finds one complete
//!   response (line + literals), parses it with the `imap-proto` crate and
//!   converts it to an owned, validated model.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// protocol crates use checked arithmetic only.
#![deny(clippy::arithmetic_side_effects)]

pub mod command;
mod error;
pub mod response;
pub mod types;

pub use error::{DecodeError, EncodeError, ValueError};
