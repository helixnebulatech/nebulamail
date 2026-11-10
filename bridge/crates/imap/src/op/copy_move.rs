// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! UID MOVE (with fallback), UID COPY and UID EXPUNGE.

use nebula_imap_proto::command::{StoreOperation, StoreResponse, UidCopy, UidExpunge, UidMove};
use nebula_imap_proto::response::{ResponseCode, Untagged};
use nebula_imap_proto::types::{Capability, Flag, MailboxName, Uid, UidSet, UidValidity};

use crate::Error;
use crate::core::client::{Client, Selected};
use crate::core::connection::{StrayContinuation, expect_ok};

/// Where each copied message ended up (UIDPLUS `COPYUID`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyUidMap {
    /// UIDVALIDITY of the destination mailbox.
    pub uid_validity: UidValidity,
    /// (source UID, destination UID) pairs.
    pub pairs: Vec<(Uid, Uid)>,
}

impl CopyUidMap {
    /// Pairs the two sets in order (RFC 4315 §3). `None` if they do not have
    /// the same size or are bigger than `max_pairs`.
    fn new(
        uid_validity: UidValidity,
        source: &UidSet,
        destination: &UidSet,
        max_pairs: u64,
    ) -> Option<Self> {
        let count = source.count()?;
        if count != destination.count()? || count > max_pairs {
            return None;
        }
        let pairs = source
            .iter_values()?
            .zip(destination.iter_values()?)
            .collect();
        Some(Self {
            uid_validity,
            pairs,
        })
    }
}

/// Result of a move or copy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct MoveResult {
    /// The UID mapping, when the server has UIDPLUS.
    pub copy_uid: Option<CopyUidMap>,
}

impl MoveResult {
    /// Joins the results of a set sent in several commands: the mapping is
    /// kept only if every part has one, with the same UIDVALIDITY.
    fn merge(parts: Vec<Self>) -> Self {
        let mut merged: Option<CopyUidMap> = None;
        for (index, part) in parts.into_iter().enumerate() {
            let Some(map) = part.copy_uid else {
                return Self::default();
            };
            match merged.as_mut() {
                None if index == 0 => merged = Some(map),
                Some(all) if all.uid_validity == map.uid_validity => {
                    all.pairs.extend(map.pairs);
                }
                _ => return Self::default(),
            }
        }
        Self { copy_uid: merged }
    }
}

impl Client<Selected> {
    /// Moves messages to another mailbox. Uses `UID MOVE` when available,
    /// else `UID COPY` + `UID STORE +FLAGS.SILENT (\Deleted)` + `UID
    /// EXPUNGE`. Without MOVE or UIDPLUS it refuses: a plain EXPUNGE would
    /// also remove other messages marked `\Deleted`.
    ///
    /// # Errors
    ///
    /// NO/BAD (`TRYCREATE` if the target does not exist), missing
    /// capabilities, or connection errors. If the fallback fails after the
    /// copy, the copies stay in the target mailbox.
    pub async fn uid_move(
        &mut self,
        uids: &UidSet,
        mailbox: &MailboxName,
    ) -> Result<MoveResult, Error> {
        if self.session.has(&Capability::Move) {
            let mut parts = Vec::new();
            for uids in uids.split(self.connection.limits.max_uid_set_len) {
                let command = UidMove {
                    uids: &uids,
                    mailbox,
                };
                parts.push(self.copy_like(&command, "UID MOVE").await?);
            }
            return Ok(MoveResult::merge(parts));
        }
        if !self.session.has(&Capability::UidPlus) {
            return Err(Error::Unsupported("MOVE or UIDPLUS"));
        }
        let result = self.uid_copy(uids, mailbox).await?;
        self.uid_store(
            uids,
            StoreOperation::Add,
            &[Flag::Deleted],
            StoreResponse::Silent,
            None,
        )
        .await?;
        self.uid_expunge(uids).await?;
        Ok(result)
    }

    /// `UID COPY`.
    ///
    /// # Errors
    ///
    /// NO/BAD or connection errors.
    pub async fn uid_copy(
        &mut self,
        uids: &UidSet,
        mailbox: &MailboxName,
    ) -> Result<MoveResult, Error> {
        let mut parts = Vec::new();
        for uids in uids.split(self.connection.limits.max_uid_set_len) {
            let command = UidCopy {
                uids: &uids,
                mailbox,
            };
            parts.push(self.copy_like(&command, "UID COPY").await?);
        }
        Ok(MoveResult::merge(parts))
    }

    /// `UID EXPUNGE` (needs UIDPLUS): expunges only these messages, if they
    /// are marked `\Deleted`.
    ///
    /// # Errors
    ///
    /// Missing UIDPLUS, NO/BAD or connection errors.
    pub async fn uid_expunge(&mut self, uids: &UidSet) -> Result<(), Error> {
        if !self.session.has(&Capability::UidPlus) {
            return Err(Error::Unsupported("UIDPLUS"));
        }
        for uids in uids.split(self.connection.limits.max_uid_set_len) {
            self.run_simple(&UidExpunge { uids: &uids }).await?;
        }
        Ok(())
    }

    async fn copy_like(
        &mut self,
        command: &impl nebula_imap_proto::command::Command,
        name: &'static str,
    ) -> Result<MoveResult, Error> {
        let mut code = None;
        let tagged = self
            .run_with(command, StrayContinuation::Reject, |data| match data {
                // RFC 6851: MOVE sends COPYUID in an untagged OK.
                Untagged::Status(status)
                    if matches!(status.code, Some(ResponseCode::CopyUid { .. })) =>
                {
                    code = status.code;
                    None
                }
                other => Some(other),
            })
            .await?;
        let tagged = expect_ok(name, tagged)?;
        let code = match tagged.code {
            Some(found @ ResponseCode::CopyUid { .. }) => Some(found),
            _ => code,
        };
        let max = self.connection.limits.max_copyuid_pairs;
        let copy_uid = match code {
            Some(ResponseCode::CopyUid {
                uid_validity,
                source,
                destination,
            }) => {
                let map = CopyUidMap::new(uid_validity, &source, &destination, max);
                if map.is_none() {
                    tracing::warn!(
                        command = name,
                        "COPYUID ignored: sets of different size or too large"
                    );
                }
                map
            }
            _ => None,
        };
        Ok(MoveResult { copy_uid })
    }
}
