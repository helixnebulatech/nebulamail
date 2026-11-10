// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errors of this crate.

/// A value does not follow the rules of its type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ValueError {
    /// The value must not be zero (UIDs, UIDVALIDITY, sequence numbers…).
    #[error("{0} must not be zero")]
    Zero(&'static str),
    /// The value is outside the allowed range.
    #[error("{0} is out of range")]
    OutOfRange(&'static str),
    /// The value is empty but must not be.
    #[error("{0} must not be empty")]
    Empty(&'static str),
    /// The value has characters or a shape that the type does not allow.
    #[error("invalid {0}")]
    Invalid(&'static str),
}

/// A command cannot be encoded safely.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EncodeError {
    /// NUL bytes cannot be sent in IMAP strings or literals (only in
    /// `literal8` of the BINARY extension, for APPEND).
    #[error("{0} contains a NUL byte")]
    Nul(&'static str),
    /// A list that must have at least one element is empty.
    #[error("{0} must not be empty")]
    Empty(&'static str),
    /// Search criteria are nested more deeply than we allow.
    #[error("search criteria are nested too deeply")]
    TooDeep,
    /// A flag that clients cannot set (`\Recent`).
    #[error("flag {0} cannot be stored")]
    FlagNotStorable(String),
    /// Arguments that break a rule of the command.
    #[error("invalid command: {0}")]
    Invalid(&'static str),
    /// The command needs a capability the caller said is not available.
    #[error("the server does not support {0}")]
    Unsupported(&'static str),
}

/// The server sent bytes that break the framing limits or rules.
///
/// The decoder no longer returns these: a response over a limit is dropped
/// and becomes an `Unknown` with reason `TooLarge`, and the connection goes
/// on. They are still used inside the scanner (and kept for compatibility).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// A line (without its literals) is longer than the limit.
    #[error("response line longer than {limit} bytes")]
    LineTooLong {
        /// The configured limit.
        limit: usize,
    },
    /// A literal announces more bytes than the limit.
    #[error("literal of {size} bytes exceeds the limit of {limit} bytes")]
    LiteralTooLarge {
        /// Size announced by the server.
        size: u64,
        /// The configured limit.
        limit: usize,
    },
    /// The response (lines + literals) is larger than the limit.
    #[error("response larger than {limit} bytes")]
    ResponseTooLarge {
        /// The configured limit.
        limit: usize,
    },
    /// A response has more literals than the limit.
    #[error("response with more than {limit} literals")]
    TooManyLiterals {
        /// The configured limit.
        limit: usize,
    },
}
