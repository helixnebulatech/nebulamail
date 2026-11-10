// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The raw message (`BODY.PEEK[]`), for `Email/get` bodies and blob
//! downloads. Kept in a bounded in-memory cache; never written to disk.
//!
//! Outside this crate a message is only loaded with
//! [`MailAccount::load_message`], which counts it in a [`Lease`] (see
//! `core::memory`): nobody can forget the budget in a new place.

use std::sync::Arc;

use nebula_imap::proto::command::{FetchAttribute, FetchModifiers, Section};
use nebula_imap::proto::response::SectionKind;
use nebula_store::EmailId;

use super::{select_location, single};
use crate::MailError;
use crate::core::account::MailAccount;
use crate::core::memory::{Copies, Lease};
use crate::model::preview;

/// What loading a message takes: the account's turn or not, and the
/// bytes to reserve before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoadPlan {
    pub(crate) turn: bool,
    pub(crate) reserve: u64,
}

/// `cached`: the size of the cached copy, if there is one; `stored`: the
/// stored RFC822.SIZE (0 if unknown); `threshold`: the biggest message the
/// cache keeps.
pub(crate) fn load_plan(
    cached: Option<u64>,
    stored: u64,
    threshold: u64,
    copies: Copies,
) -> LoadPlan {
    match cached {
        // Shared with the cache (already bounded by it): only the copies.
        Some(len) => LoadPlan {
            turn: false,
            reserve: len.saturating_mul(copies.count().saturating_sub(1)),
        },
        None => LoadPlan {
            turn: stored == 0 || stored > threshold,
            reserve: stored.saturating_mul(copies.count()),
        },
    }
}

fn len_of(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

impl MailAccount {
    /// The whole message as stored on the server (`BODY.PEEK[]`: doesn't
    /// set `$seen`). Fills in the preview the first time if the server had
    /// none.
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an unknown email or a message the
    /// server no longer has, and connection errors.
    pub(crate) async fn raw_message(&self, email: EmailId) -> Result<Arc<[u8]>, MailError> {
        let cache = &self.shared.cache;
        // The store first: a cached message may have been deleted since.
        let location = self
            .visible_locations(email)
            .await?
            .into_iter()
            .next()
            .ok_or(MailError::NotFound("email"))?;
        if let Some(raw) = cache.get(self.id, email) {
            return Ok(raw);
        }
        let mut slot = self.slot().await;
        let client = select_location(self, &mut slot, &location).await?;
        let attributes = [FetchAttribute::BodyPeek {
            section: Section::Full,
            partial: None,
        }];
        let result = client
            .uid_fetch(&single(&location), &attributes, FetchModifiers::None)
            .await;
        let fetched = slot.check(result)?;
        drop(slot);
        let raw: Arc<[u8]> = fetched
            .messages
            .into_iter()
            .filter(|data| data.uid == Some(location.uid))
            .flat_map(|data| data.sections)
            .find(|section| section.section == SectionKind::Full)
            .and_then(|section| section.data)
            .map(Arc::from)
            .ok_or(MailError::NotFound("message"))?;
        cache.insert(self.id, email, Arc::clone(&raw));
        self.fill_preview(email, Arc::clone(&raw)).await?;
        Ok(raw)
    }

    /// The whole message (as [`Self::raw_message`]), counted in `lease`:
    /// the message `copies` times, or only the extra copy when it comes
    /// from the cache. A message the cache can't hold also takes the
    /// account's turn, kept in the lease: give it back with
    /// [`Lease::drop_turn`] (or [`Lease::keep_only`]) once the whole copy
    /// is no longer needed.
    ///
    /// # Errors
    /// [`MailError::Invalid`] for a lease of another account,
    /// [`Unavailable::Busy`](crate::Unavailable) if the memory or the turn
    /// isn't free in time (the lease is as it was), and the errors of
    /// [`Self::raw_message`].
    pub async fn load_message(
        &self,
        email: EmailId,
        lease: &mut Lease,
        copies: Copies,
    ) -> Result<Arc<[u8]>, MailError> {
        if !Arc::ptr_eq(lease.turns(), &self.whole_turn) {
            return Err(MailError::Invalid("lease of another account"));
        }
        if let Some(raw) = self.cached_message(email).await? {
            let plan = load_plan(Some(len_of(&raw)), 0, 0, copies);
            lease.reserve(plan.reserve).await?;
            return Ok(raw);
        }
        let stored = self
            .store()
            .emails(self.id, vec![email])
            .await?
            .first()
            .map_or(0, |stored| stored.meta.size);
        let plan = load_plan(None, stored, self.stream_threshold(), copies);
        if plan.turn {
            lease.take_turn().await?;
        }
        lease.reserve(plan.reserve).await?;
        let raw = match self.raw_message(email).await {
            Ok(raw) => raw,
            Err(error) => {
                lease.release(plan.reserve);
                return Err(error);
            }
        };
        // The stored size may not be the real one.
        let real = len_of(&raw).saturating_mul(copies.count());
        if let Some(more) = real.checked_sub(plan.reserve).filter(|more| *more > 0) {
            if let Err(error) = lease.reserve(more).await {
                lease.release(plan.reserve);
                return Err(error);
            }
        } else {
            lease.release(plan.reserve.saturating_sub(real));
        }
        Ok(raw)
    }

    async fn fill_preview(&self, email: EmailId, raw: Arc<[u8]>) -> Result<(), MailError> {
        let has_preview = self
            .store()
            .emails(self.id, vec![email])
            .await?
            .first()
            .is_some_and(|stored| stored.meta.preview.is_some());
        if has_preview {
            return Ok(());
        }
        // Parsing a big message is CPU work: off the async workers.
        let text = tokio::task::spawn_blocking(move || preview::from_message(&raw))
            .await
            .unwrap_or(None);
        if let Some(text) = text {
            self.store().set_preview(self.id, email, text).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use std::num::NonZeroU32;

    use nebula_imap::proto::types::Uid;

    use super::*;
    use crate::test_support::{NOW, closed_port, fixture};

    #[tokio::test(flavor = "multi_thread")]
    async fn the_cache_never_serves_a_deleted_email() {
        let f = fixture(closed_port()).await;
        let account = f.account();
        let email = f.add_message(5).await;
        let raw: Arc<[u8]> = Arc::from(&b"Subject: Hola\r\n\r\nHola mundo\r\n"[..]);
        f.mail
            .shared
            .cache
            .insert(f.account, email, Arc::clone(&raw));
        assert_eq!(account.raw_message(email).await.unwrap(), raw);
        assert_eq!(account.cached_message(email).await.unwrap(), Some(raw));

        // Another client expunged it: the sync removes the copy.
        f.store()
            .remove_messages(f.account, f.inbox, vec![Uid::new(5).unwrap()], NOW)
            .await
            .unwrap();
        assert!(matches!(
            account.raw_message(email).await,
            Err(MailError::NotFound(_))
        ));
        assert!(f.mail.shared.cache.get(f.account, email).is_none());
        f.mail
            .shared
            .cache
            .insert(f.account, email, Arc::from(&b"x"[..]));
        assert!(matches!(
            account.cached_message(email).await,
            Err(MailError::NotFound(_))
        ));
        assert!(f.mail.shared.cache.get(f.account, email).is_none());
        f.mail
            .shared
            .cache
            .insert(f.account, email, Arc::from(&b"x"[..]));
        let texts = account
            .body_texts(&[email], NonZeroU32::new(100).unwrap())
            .await
            .unwrap();
        assert!(texts.is_empty());
        assert!(f.mail.shared.cache.get(f.account, email).is_none());
    }

    #[test]
    fn load_plans() {
        let threshold = 8;
        // In the cache: no turn, only the extra copy.
        assert_eq!(
            load_plan(Some(5), 100, threshold, Copies::One),
            LoadPlan {
                turn: false,
                reserve: 0
            }
        );
        assert_eq!(
            load_plan(Some(5), 100, threshold, Copies::Two),
            LoadPlan {
                turn: false,
                reserve: 5
            }
        );
        // Small: no turn, all the copies.
        assert_eq!(
            load_plan(None, 8, threshold, Copies::Two),
            LoadPlan {
                turn: false,
                reserve: 16
            }
        );
        // Bigger than the cache, or of unknown size: the turn.
        assert_eq!(
            load_plan(None, 9, threshold, Copies::One),
            LoadPlan {
                turn: true,
                reserve: 9
            }
        );
        assert_eq!(
            load_plan(None, 0, threshold, Copies::Two),
            LoadPlan {
                turn: true,
                reserve: 0
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_load_takes_no_turn_nor_memory() {
        let f = fixture(closed_port()).await;
        let account = f.account();
        let email = f.add_message(5).await;
        // Small enough for the cache's shards; one permit (1 MiB) of memory.
        let raw: Arc<[u8]> = Arc::from(vec![b'x'; 300 * 1024]);
        f.mail
            .shared
            .cache
            .insert(f.account, email, Arc::clone(&raw));
        let memory = &f.mail.shared.memory;
        let free = memory.available();
        let mut lease = account.lease();
        let loaded = account
            .load_message(email, &mut lease, Copies::One)
            .await
            .unwrap();
        assert_eq!(loaded, raw);
        assert_eq!((lease.bytes(), lease.has_turn()), (0, false));
        assert_eq!(memory.available(), free);
        // Two copies: the cached one is shared, the other is counted.
        account
            .load_message(email, &mut lease, Copies::Two)
            .await
            .unwrap();
        assert_eq!(lease.bytes(), 300 * 1024);
        assert!(!lease.has_turn());
        assert_eq!(memory.available(), free - 1024 * 1024);
        drop(lease);
        assert_eq!(memory.available(), free);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lease_of_another_account_loads_nothing() {
        let f = fixture(closed_port()).await;
        let account = f.account();
        let email = f.add_message(5).await;
        f.mail
            .shared
            .cache
            .insert(f.account, email, Arc::from(&b"x"[..]));
        let mut foreign = f.account().lease();
        assert!(matches!(
            account.load_message(email, &mut foreign, Copies::One).await,
            Err(MailError::Invalid(_))
        ));
        let mut detached = f.mail.shared.memory.lease();
        assert!(matches!(
            account
                .load_message(email, &mut detached, Copies::One)
                .await,
            Err(MailError::Invalid(_))
        ));
    }
}
