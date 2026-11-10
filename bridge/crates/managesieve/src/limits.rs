// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Limits and timeouts of a connection.

use std::time::Duration;

use nebula_managesieve_proto::Limits as FrameLimits;

/// Limits and timeouts. The defaults suit the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Sizes of incoming lines and literals (a literal holds a whole
    /// script: 4 MiB by default).
    pub frame: FrameLimits,
    /// Time to connect's greeting (and TLS).
    pub greeting_timeout: Duration,
    /// Longest silence from the server while a command runs.
    pub read_timeout: Duration,
    /// Longest total time for one command.
    pub command_timeout: Duration,
    /// Most data lines in one response (scripts in `LISTSCRIPTS`).
    pub max_data_lines: usize,
    /// Bytes one response may take besides its literals (1 MiB by
    /// default: `LISTSCRIPTS` with a thousand names). A response may take
    /// [`Limits::max_reply_bytes`] in total.
    pub max_reply_overhead: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            frame: FrameLimits::default(),
            greeting_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(60),
            command_timeout: Duration::from_secs(120),
            max_data_lines: 1000,
            max_reply_overhead: 1024 * 1024,
        }
    }
}

/// Before STARTTLS: one line, one literal, the literals of a line.
const BEFORE_TLS_LINE: usize = 8 * 1024;
/// Before STARTTLS: one response in total (with `BEFORE_TLS_LINE` of
/// literals).
const BEFORE_TLS_REPLY: usize = 64 * 1024;

impl Limits {
    /// Most bytes received for one response: its literals
    /// ([`FrameLimits::max_line_literals`]) plus
    /// [`Limits::max_reply_overhead`]. Derived from the literal size, so a
    /// larger script size raises it too.
    #[must_use]
    pub fn max_reply_bytes(&self) -> usize {
        self.frame
            .max_line_literals
            .saturating_add(self.max_reply_overhead)
    }

    /// For the plaintext part of STARTTLS (greeting and the `STARTTLS`
    /// reply), which anyone in the middle can write: the same timeouts,
    /// small sizes. A capability list needs a few hundred bytes.
    #[must_use]
    pub fn before_tls(self) -> Self {
        Self {
            frame: FrameLimits {
                max_line: BEFORE_TLS_LINE,
                max_literal: BEFORE_TLS_LINE,
                max_line_literals: BEFORE_TLS_LINE,
                max_items: 64,
                ..self.frame
            },
            max_data_lines: 256,
            max_reply_overhead: BEFORE_TLS_REPLY.saturating_sub(BEFORE_TLS_LINE),
            ..self
        }
    }
}
