// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Errors.

/// A line could not be read. After any of these the stream is out of sync:
/// the connection must be dropped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// A line (outside literals) is longer than [`crate::Limits::max_line`].
    #[error("line too long")]
    LineTooLong,
    /// A literal is longer than [`crate::Limits::max_literal`], or the
    /// literals of a line together are longer than allowed.
    #[error("literal too long")]
    LiteralTooLong,
    /// Too many items or too deep lists.
    #[error("too many items in a line")]
    TooManyItems,
    /// The bytes are not ManageSieve: what the decoder expected.
    #[error("malformed line: {0}")]
    Malformed(&'static str),
    /// A value that must be UTF-8 (capability, script name) is not.
    #[error("invalid UTF-8 in {0}")]
    Utf8(&'static str),
}

/// A script name that RFC 5804 §1.6 does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NameError {
    /// Empty.
    #[error("the script name is empty")]
    Empty,
    /// Longer than [`crate::ScriptName::MAX_LEN`] bytes.
    #[error("the script name is too long")]
    TooLong,
    /// Not UTF-8.
    #[error("the script name is not UTF-8")]
    NotUtf8,
    /// A control character (U+0000–U+001F, U+007F–U+009F) or a line or
    /// paragraph separator (U+2028, U+2029).
    #[error("the script name has a control character")]
    ControlCharacter,
}
