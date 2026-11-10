// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Size limits of the decoders.

/// How much a peer may send. The defaults suit the bridge: scripts
/// are a few KiB, servers allow 100 KiB to 1 MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Bytes of one line outside its literals (with the CRLF).
    pub max_line: usize,
    /// Bytes of one literal.
    pub max_literal: usize,
    /// Bytes of all literals of one line together.
    pub max_line_literals: usize,
    /// Items (atoms, strings, lists) in one line, nested ones included.
    pub max_items: usize,
    /// Nesting of parenthesized lists.
    pub max_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_line: 64 * 1024,
            max_literal: 4 * 1024 * 1024,
            max_line_literals: 4 * 1024 * 1024,
            max_items: 1024,
            max_depth: 4,
        }
    }
}
