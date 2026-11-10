// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Limits and timeouts of a connection.

use std::time::Duration;

use nebula_imap_proto::response::FrameLimits;

/// Limits and timeouts. The defaults suit the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Sizes of incoming responses (16 MiB lines, 64 MiB literals…). A
    /// response over them is dropped, not fatal: see
    /// [`Error::ResponseTooLarge`](crate::Error::ResponseTooLarge).
    pub frame: FrameLimits,
    /// Time to get the greeting (and finish TLS).
    pub greeting_timeout: Duration,
    /// Longest silence from the server while a command runs.
    pub read_timeout: Duration,
    /// Longest total time for one command.
    pub command_timeout: Duration,
    /// When to restart IDLE. RFC 2177: servers may drop idle clients after
    /// 30 minutes, so clients re-IDLE before 29.
    pub idle_refresh: Duration,
    /// Most UID pairs we expand from a COPYUID response.
    pub max_copyuid_pairs: u64,
    /// Most UIDs one UID SEARCH may return, from `* SEARCH` or `* ESEARCH`
    /// (a hostile server could answer `ALL 1:4294967295` in a few bytes).
    pub max_search_results: u64,
    /// Longest UID set in one command line, in bytes. Longer sets are sent
    /// in several commands (RFC 7162 §4: lines under 8192 bytes).
    pub max_uid_set_len: usize,
    /// Most bytes of untagged responses one command may receive. Each
    /// response is bounded by [`FrameLimits`], this bounds their sum: a
    /// hostile server cannot fill memory with endless responses until the
    /// command timeout. `UID FETCH 1:* (UID FLAGS MODSEQ)` of a million
    /// messages is about 60 MiB. Going over it breaks the connection.
    pub max_command_bytes: usize,
    /// Most untagged responses one command may receive (also fatal).
    ///
    /// Memory: a response is small on the wire (`* 1 EXPUNGE` is 13 bytes)
    /// but an `Untagged` is 88 bytes, and a FETCH kept by a collector
    /// (`uid_fetch`, a QRESYNC SELECT) 464 bytes plus its data. Events are
    /// capped by [`Self::max_events`] and big FETCH answers are streamed by
    /// `uid_fetch_each`, so this bounds the collectors: with the default of
    /// 1,000,000, 88 MiB per connection for `Untagged`s (about 180 MiB per
    /// account, which has a command and an IDLE connection), 464 MiB in the
    /// worst collector. 2,000,000 would double that for mailboxes of over
    /// a million messages, which are rare: raise it for them.
    pub max_command_responses: usize,
    /// Most unsolicited responses kept for
    /// [`Client::take_events`](crate::Client::take_events). Past it they
    /// are dropped and the events say `overflowed`: the caller must then
    /// do a full sync. Not an error: a UID MOVE of 20,000 messages without
    /// QRESYNC sends 20,000 EXPUNGE.
    pub max_events: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            frame: FrameLimits::default(),
            greeting_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(60),
            command_timeout: Duration::from_secs(300),
            idle_refresh: Duration::from_mins(28),
            max_copyuid_pairs: 100_000,
            max_search_results: 1_000_000,
            max_uid_set_len: 4096,
            max_command_bytes: 256 * 1024 * 1024,
            max_command_responses: 1_000_000,
            max_events: 10_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use nebula_imap_proto::response::{FetchData, Untagged};

    /// The memory figures in the docs of [`super::Limits`].
    #[test]
    fn response_sizes_match_the_docs() {
        assert!(size_of::<Untagged>() <= 88, "{}", size_of::<Untagged>());
        assert!(size_of::<FetchData>() <= 464, "{}", size_of::<FetchData>());
    }
}
