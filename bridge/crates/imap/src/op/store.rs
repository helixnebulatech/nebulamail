// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! UID STORE.

use nebula_imap_proto::command::{StoreOperation, StoreResponse, UidStore, UnchangedSince};
use nebula_imap_proto::response::{FetchData, ResponseCode, Untagged};
use nebula_imap_proto::types::{Capability, Flag, SeqBound, SeqItem, Uid, UidSet};

use crate::Error;
use crate::core::client::{Client, Selected};
use crate::core::connection::{StrayContinuation, expect_ok};

/// Results of UID STORE.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct StoreResult {
    /// FETCH responses with the new flags (and MODSEQ) of the UIDs asked
    /// for. Empty with `.SILENT`, except for changes made by others at the
    /// same time. FETCH responses for other messages go to the events.
    pub fetched: Vec<FetchData>,
    /// With UNCHANGEDSINCE: the UIDs that were not stored because they
    /// changed (`[MODIFIED ...]`).
    pub modified: Option<UidSet>,
}

/// Whether `uid` is in the set sent (`n:*` covers every UID from `n`).
fn asked(uids: &UidSet, uid: Uid) -> bool {
    uids.contains(uid)
        || uids.items().iter().any(|item| match item {
            SeqItem::Single(SeqBound::Star) | SeqItem::Range(SeqBound::Star, SeqBound::Star) => {
                true
            }
            SeqItem::Range(SeqBound::Value(low), SeqBound::Star)
            | SeqItem::Range(SeqBound::Star, SeqBound::Value(low)) => *low <= uid,
            _ => false,
        })
}

impl Client<Selected> {
    /// `UID STORE`: adds, removes or replaces flags. A long UID set is
    /// sent as several commands.
    ///
    /// # Errors
    ///
    /// NO/BAD, UNCHANGEDSINCE without CONDSTORE, or connection errors.
    pub async fn uid_store(
        &mut self,
        uids: &UidSet,
        operation: StoreOperation,
        flags: &[Flag],
        response: StoreResponse,
        unchanged_since: Option<UnchangedSince>,
    ) -> Result<StoreResult, Error> {
        if unchanged_since.is_some() && !self.session.has(&Capability::CondStore) {
            return Err(Error::Unsupported("CONDSTORE"));
        }
        let mut fetched = Vec::new();
        let mut modified: Vec<SeqItem<Uid>> = Vec::new();
        for uids in uids.split(self.connection.limits.max_uid_set_len) {
            let command = UidStore {
                uids: &uids,
                operation,
                flags,
                response,
                unchanged_since,
            };
            let tagged = self
                .run_with(&command, StrayContinuation::Reject, |data| match data {
                    Untagged::Fetch(fetch) if fetch.uid.is_some_and(|uid| asked(&uids, uid)) => {
                        fetched.push(*fetch);
                        None
                    }
                    other => Some(other),
                })
                .await?;
            let tagged = expect_ok("UID STORE", tagged)?;
            if let Some(ResponseCode::Modified(set)) = tagged.code {
                modified.extend_from_slice(set.items());
            }
        }
        let modified = UidSet::from_items(modified);
        Ok(StoreResult { fetched, modified })
    }
}
