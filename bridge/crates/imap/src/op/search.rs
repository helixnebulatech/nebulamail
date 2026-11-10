// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! UID SEARCH.

use std::num::NonZeroU32;

use nebula_imap_proto::command::{SearchKey, SearchReturn, UidSearch};
use nebula_imap_proto::response::{ESearchAll, ESearchResult, SearchResult, UnknownKind, Untagged};
use nebula_imap_proto::types::{Capabilities, Capability};

use crate::Error;
use crate::core::client::{Client, Selected};
use crate::core::connection::{StrayContinuation, expect_ok};

/// Adds the UIDs of an ESEARCH answer, refusing more than `max` in total.
fn add_esearch(result: &mut SearchResult, found: ESearchResult, max: u64) -> Result<(), Error> {
    // We only send UID SEARCH: sequence numbers would be misread.
    if !found.uid {
        return Err(Error::Protocol("ESEARCH without UID"));
    }
    result.highest_mod_seq = result.highest_mod_seq.max(found.mod_seq);
    let all = match found.all {
        None => return Ok(()),
        Some(ESearchAll::Uids(set)) => set,
        Some(_) => return Err(Error::Protocol("ESEARCH without UID")),
    };
    let count = all.count().ok_or(Error::Protocol("ESEARCH ALL with *"))?;
    let total = u64::try_from(result.numbers.len())
        .unwrap_or(u64::MAX)
        .saturating_add(count);
    if total > max {
        return Err(Error::Protocol("SEARCH result too large"));
    }
    let values = all
        .iter_values()
        .ok_or(Error::Protocol("ESEARCH ALL with *"))?;
    result
        .numbers
        .extend(values.filter_map(|uid| NonZeroU32::new(uid.get())));
    Ok(())
}

/// Adds the UIDs of a plain `* SEARCH` line (servers may split the result
/// over several), refusing more than `max` in total.
fn add_plain(result: &mut SearchResult, found: SearchResult, max: u64) -> Result<(), Error> {
    let total = result.numbers.len().saturating_add(found.numbers.len());
    if u64::try_from(total).unwrap_or(u64::MAX) > max {
        return Err(Error::Protocol("SEARCH result too large"));
    }
    result.numbers.extend(found.numbers);
    result.highest_mod_seq = result.highest_mod_seq.max(found.highest_mod_seq);
    Ok(())
}

/// Whether to ask `RETURN (ALL)`: with ESEARCH (RFC 4731), which a server
/// that only speaks `IMAP4rev2` has without listing it (RFC 9051 Appendix E;
/// see [`Capabilities::has`]).
fn wants_esearch(capabilities: &Capabilities) -> bool {
    capabilities.has(&Capability::ESearch)
}

/// What one UID SEARCH received.
struct Collector {
    result: SearchResult,
    max: u64,
    /// A SEARCH or ESEARCH response arrived (RFC 3501 §7.2.5 and RFC 4731
    /// send one even when nothing matches).
    answered: bool,
    /// A SEARCH or ESEARCH response we could not read: the result would
    /// be partial.
    unreadable: bool,
    /// The first error, returned after the tagged response.
    failure: Option<Error>,
}

impl Collector {
    fn take(&mut self, data: Untagged) -> Option<Untagged> {
        let added = match data {
            Untagged::Search(found) => {
                self.answered = true;
                self.failure
                    .is_none()
                    .then(|| add_plain(&mut self.result, found, self.max))
            }
            Untagged::ESearch(found) => {
                self.answered = true;
                self.failure
                    .is_none()
                    .then(|| add_esearch(&mut self.result, found, self.max))
            }
            Untagged::Unknown(unknown)
                if matches!(unknown.about, UnknownKind::Search | UnknownKind::ESearch) =>
            {
                self.unreadable = true;
                // Still logged (by size) as an unknown response.
                return Some(Untagged::Unknown(unknown));
            }
            other => return Some(other),
        };
        if let Some(Err(error)) = added {
            self.failure = Some(error);
        }
        None
    }

    fn finish(self) -> Result<SearchResult, Error> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.unreadable {
            return Err(Error::Protocol("unreadable SEARCH response"));
        }
        if !self.answered {
            return Err(Error::Protocol("UID SEARCH without results response"));
        }
        Ok(self.result)
    }
}

impl Client<Selected> {
    /// `UID SEARCH`: the UIDs of the matching messages (and their highest
    /// MODSEQ when the criteria use MODSEQ).
    ///
    /// With ESEARCH (RFC 4731), or `IMAP4rev2` only, it asks `RETURN (ALL)`:
    /// the server answers with compact ranges (`1:5000`) instead of every
    /// UID, which matters for big mailboxes. The result is the same either
    /// way.
    ///
    /// Never an empty result by mistake: an OK without any SEARCH or
    /// ESEARCH response, or with one we could not read, is an error.
    ///
    /// # Errors
    ///
    /// NO/BAD (for example `BADCHARSET`), more than
    /// [`Limits::max_search_results`](crate::Limits) UIDs in SEARCH or
    /// ESEARCH answers, a missing or unreadable answer, or connection
    /// errors.
    pub async fn uid_search(&mut self, criteria: &SearchKey) -> Result<SearchResult, Error> {
        let returns = if wants_esearch(&self.session.capabilities) {
            SearchReturn::All
        } else {
            SearchReturn::Plain
        };
        let command = UidSearch { criteria, returns };
        let mut collector = Collector {
            result: SearchResult {
                numbers: Vec::new(),
                highest_mod_seq: None,
            },
            max: self.connection.limits.max_search_results,
            answered: false,
            unreadable: false,
            failure: None,
        };
        let tagged = self
            .run_with(&command, StrayContinuation::Reject, |data| {
                collector.take(data)
            })
            .await?;
        expect_ok("UID SEARCH", tagged)?;
        collector.finish()
    }
}
