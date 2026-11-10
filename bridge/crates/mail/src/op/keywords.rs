// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Setting and clearing keywords (`Email/set` with `keywords`).

use std::collections::BTreeSet;

use nebula_imap::proto::command::{StoreOperation, StoreResponse};
use nebula_imap::proto::types::Flag;
use nebula_jmap_proto::Keyword;
use nebula_store::{EmailId, FlagUpdate, Location, Recorded, UnixTime};

use super::{select_location, single};
use crate::MailError;
use crate::core::account::MailAccount;
use crate::model::keywords::{first_unstorable, from_flags, removal_flags, to_flag};

/// How the keywords of an email change.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeywordChange {
    /// Exactly these keywords (`keywords: {...}`).
    Set(BTreeSet<Keyword>),
    /// Add and remove some (`keywords/$seen: true` patches).
    Patch {
        /// Keywords to add.
        add: BTreeSet<Keyword>,
        /// Keywords to remove.
        remove: BTreeSet<Keyword>,
    },
}

fn flags(keywords: &BTreeSet<Keyword>) -> Result<Vec<Flag>, MailError> {
    keywords
        .iter()
        .map(|keyword| to_flag(keyword).ok_or(MailError::Invalid("keyword")))
        .collect()
}

impl MailAccount {
    /// Checks that every copy can keep the flags of `add` before writing to
    /// any, so a mailbox that can't keep a keyword doesn't leave the others
    /// changed. Only PERMANENTFLAGS changing in between can still do that.
    async fn check_storable(&self, locations: &[Location], add: &[Flag]) -> Result<(), MailError> {
        let mut slot = self.slot().await;
        for location in locations {
            let client = select_location(self, &mut slot, location).await?;
            let state = client.mailbox();
            if let Some(flag) = first_unstorable(&[&state.permanent_flags], add) {
                tracing::debug!(%flag, "keyword not storable in one of the mailboxes");
                return Err(MailError::Unsupported(
                    "keyword not storable in this mailbox",
                ));
            }
        }
        Ok(())
    }

    /// Changes the keywords of an email on every copy (UID STORE), then
    /// stores the flags the server reports.
    ///
    /// A keyword a mailbox can't store (PERMANENTFLAGS without it and
    /// without `\*`) fails the whole change before anything is written,
    /// checking every copy first: JMAP must not report a keyword that isn't
    /// kept. Clearing `$junk`/`$notjunk` also clears `Junk`/`NonJunk`.
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an unknown email,
    /// [`MailError::Unsupported`] for a keyword that can't be stored, and
    /// connection errors.
    pub async fn update_keywords(
        &self,
        email: EmailId,
        change: KeywordChange,
    ) -> Result<Recorded, MailError> {
        let current = self
            .store()
            .emails(self.id, vec![email])
            .await?
            .pop()
            .ok_or(MailError::NotFound("email"))?
            .keywords;
        let target = match change {
            KeywordChange::Set(set) => set,
            KeywordChange::Patch { add, remove } => current
                .union(&add)
                .filter(|keyword| !remove.contains(*keyword))
                .cloned()
                .collect(),
        };
        let add = flags(&target.difference(&current).cloned().collect())?;
        let mut remove = Vec::new();
        for keyword in current.difference(&target) {
            remove.extend(removal_flags(keyword).ok_or(MailError::Invalid("keyword"))?);
        }
        if add.is_empty() && remove.is_empty() {
            return Ok(Recorded::default());
        }

        let locations = self.email_locations(email).await?;
        if locations.len() > 1 && !add.is_empty() {
            self.check_storable(&locations, &add).await?;
        }

        let mut recorded = Recorded::default();
        for location in locations {
            let mut slot = self.slot().await;
            let client = select_location(self, &mut slot, &location).await?;
            let state = client.mailbox();
            if let Some(flag) = first_unstorable(&[&state.permanent_flags], &add) {
                tracing::debug!(%flag, "keyword not storable in this mailbox");
                return Err(MailError::Unsupported(
                    "keyword not storable in this mailbox",
                ));
            }
            let mut reported = None;
            for (operation, list) in [
                (StoreOperation::Add, &add),
                (StoreOperation::Remove, &remove),
            ] {
                if list.is_empty() {
                    continue;
                }
                let client = slot
                    .selected()
                    .ok_or(MailError::NotFound("selected mailbox"))?;
                let result = client
                    .uid_store(
                        &single(&location),
                        operation,
                        list,
                        StoreResponse::Fetch,
                        None,
                    )
                    .await;
                let stored = slot.check(result)?;
                if let Some(data) = stored
                    .fetched
                    .into_iter()
                    .rev()
                    .find(|data| data.uid == Some(location.uid) && data.flags.is_some())
                {
                    reported = Some(data);
                }
            }
            // The server's word if it sent the new flags, else ours. The
            // connection stays ours until the store has them (as in
            // `move_copy`), so a sync can't apply flags it read before ours.
            let update = match reported {
                Some(data) => FlagUpdate {
                    uid: location.uid,
                    mod_seq: data.mod_seq,
                    keywords: from_flags(data.flags.as_deref().unwrap_or(&[])).keywords,
                },
                None => FlagUpdate {
                    uid: location.uid,
                    mod_seq: None,
                    keywords: target.clone(),
                },
            };
            let applied = self
                .store()
                .update_flags(self.id, location.mailbox, vec![update], UnixTime::now()?)
                .await?;
            drop(slot);
            self.emit(&applied.recorded);
            recorded.merge(applied.recorded);
        }
        Ok(recorded)
    }
}
