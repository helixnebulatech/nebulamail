// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! UID FETCH.

use nebula_imap_proto::command::{FetchAttribute, FetchModifiers, UidFetch};
use nebula_imap_proto::response::{FetchData, UnknownKind, Untagged, VanishedKind};
use nebula_imap_proto::types::{Capability, UidSet};

use crate::Error;
use crate::core::client::{Client, ObjectIdSupport, Selected, SyncLevel};
use crate::core::connection::{StrayContinuation, expect_ok};

/// Results of a batched UID FETCH.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FetchResult {
    /// One entry per FETCH response, in arrival order.
    pub messages: Vec<FetchData>,
    /// UIDs reported expunged by `(CHANGEDSINCE n VANISHED)`.
    pub vanished: Vec<UidSet>,
    /// FETCH responses we could not read (they are missing from
    /// `messages`). If not zero, a message absent from `messages` may
    /// still exist: do not take the absence as an expunge.
    pub unreadable: usize,
    /// Responses dropped because they were larger than the limits (a
    /// BODYSTRUCTURE with thousands of parts…): their messages are missing
    /// from `messages`, and maybe some VANISHED from `vanished`.
    pub skipped: usize,
}

/// What a [`Client::uid_fetch_each`] gives back, besides the messages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FetchSummary {
    /// UIDs reported expunged by `(CHANGEDSINCE n VANISHED)`.
    pub vanished: Vec<UidSet>,
    /// FETCH responses we could not read (see [`FetchResult::unreadable`]).
    pub unreadable: usize,
    /// Responses dropped for their size, as in [`FetchResult::skipped`].
    pub skipped: usize,
}

impl Client<Selected> {
    /// `UID FETCH`, collecting all results. For big result sets prefer
    /// [`Self::uid_fetch_each`], which does not keep them in memory.
    ///
    /// # Errors
    ///
    /// NO/BAD from the server, missing CONDSTORE/QRESYNC for the modifiers,
    /// or connection errors. Each response is bounded by the frame limits.
    pub async fn uid_fetch(
        &mut self,
        uids: &UidSet,
        attributes: &[FetchAttribute],
        modifiers: FetchModifiers,
    ) -> Result<FetchResult, Error> {
        let mut messages = Vec::new();
        let FetchSummary {
            vanished,
            unreadable,
            skipped,
        } = self
            .uid_fetch_each(uids, attributes, modifiers, |data| messages.push(data))
            .await?;
        Ok(FetchResult {
            messages,
            vanished,
            unreadable,
            skipped,
        })
    }

    /// `UID FETCH`, calling `on_message` for each FETCH response as it
    /// arrives. Returns the UIDs reported by `VANISHED (EARLIER)` and how
    /// many FETCH responses could not be read or were dropped for their
    /// size: unlike other commands, that is not an error here (the
    /// connection is still usable and the caller can ask again for the
    /// missing messages with fewer items).
    ///
    /// FETCH responses for other messages (flag changes made by other
    /// clients) also arrive here: check [`FetchData::uid`].
    ///
    /// A UID set longer than [`Limits::max_uid_set_len`](crate::Limits) is
    /// sent as several commands, one after the other.
    ///
    /// # Errors
    ///
    /// As [`Self::uid_fetch`].
    pub async fn uid_fetch_each(
        &mut self,
        uids: &UidSet,
        attributes: &[FetchAttribute],
        modifiers: FetchModifiers,
        mut on_message: impl FnMut(FetchData) + Send,
    ) -> Result<FetchSummary, Error> {
        self.check_fetch(attributes, modifiers)?;
        let mut vanished = Vec::new();
        let mut unreadable: usize = 0;
        let mut skipped_total = 0_usize;
        for uids in uids.split(self.connection.limits.max_uid_set_len) {
            let command = UidFetch {
                uids: &uids,
                attributes,
                modifiers,
            };
            let (tagged, skipped) = self
                .run_counting_skipped(&command, StrayContinuation::Reject, |data| match data {
                    Untagged::Fetch(fetch) => {
                        on_message(*fetch);
                        None
                    }
                    Untagged::Vanished(gone) if gone.kind == VanishedKind::Earlier => {
                        vanished.push(gone.uids);
                        None
                    }
                    Untagged::Unknown(unknown) if unknown.about == UnknownKind::Fetch => {
                        unreadable = unreadable.saturating_add(1);
                        // Still logged (by size) as an unknown response.
                        Some(Untagged::Unknown(unknown))
                    }
                    other => Some(other),
                })
                .await?;
            expect_ok("UID FETCH", tagged)?;
            skipped_total = skipped_total.saturating_add(skipped);
        }
        Ok(FetchSummary {
            vanished,
            unreadable,
            skipped: skipped_total,
        })
    }

    fn check_fetch(
        &self,
        attributes: &[FetchAttribute],
        modifiers: FetchModifiers,
    ) -> Result<(), Error> {
        let session = &self.session;
        match modifiers {
            FetchModifiers::ChangedSince(_) if session.sync < SyncLevel::CondStore => {
                return Err(Error::Unsupported("CONDSTORE"));
            }
            FetchModifiers::ChangedSinceWithVanished(_) if session.sync < SyncLevel::QResync => {
                return Err(Error::Unsupported("QRESYNC"));
            }
            _ => {}
        }
        for attribute in attributes {
            if *attribute == FetchAttribute::ObjectIds
                && session.object_ids != ObjectIdSupport::ObjectIdPlus
            {
                return Err(Error::Unsupported("OBJECTID+"));
            }
            let needed = match attribute {
                FetchAttribute::ModSeq => Some((Capability::CondStore, "CONDSTORE")),
                FetchAttribute::BinaryPeek { .. } | FetchAttribute::BinarySize { .. } => {
                    Some((Capability::Binary, "BINARY"))
                }
                FetchAttribute::EmailId | FetchAttribute::ThreadId => {
                    Some((Capability::ObjectId, "OBJECTID"))
                }
                FetchAttribute::Preview => Some((Capability::Preview, "PREVIEW")),
                FetchAttribute::GmailMsgId
                | FetchAttribute::GmailThreadId
                | FetchAttribute::GmailLabels => Some((Capability::GmailExt1, "X-GM-EXT-1")),
                _ => None,
            };
            if let Some((capability, name)) = needed
                && !session.has(&capability)
            {
                return Err(Error::Unsupported(name));
            }
        }
        Ok(())
    }
}
