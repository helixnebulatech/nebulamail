// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Destroying an email (`Email/set` destroy). Moving to Trash is the
//! client's job (a `mailboxIds` change); destroy removes every copy for
//! good: `\Deleted` + `UID EXPUNGE` in each mailbox.
//!
//! Also taking out an email just appended when a later step failed
//! ([`MailAccount::discard_appended`]), only if it is surely ours alone.

use std::collections::BTreeSet;

use nebula_store::{EmailId, MailboxId, Recorded};

use super::append::{AppendOrigin, Appended};
use crate::MailError;
use crate::core::account::MailAccount;

/// What [`MailAccount::discard_appended`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discarded {
    /// Every copy was removed.
    Removed,
    /// Left as it is: it may be (or be joined to) an email the user
    /// already had, and removing it would lose that one.
    Kept,
}

/// Whether an appended email may be removed whole: the APPEND made it
/// (not a server's deduplicated answer, not joined to an existing email)
/// and every copy it has now is one we were placing, one per mailbox (a
/// sync may have joined an older copy to it since).
fn may_discard(
    origin: AppendOrigin,
    copies: &[MailboxId],
    placed_in: &BTreeSet<MailboxId>,
) -> bool {
    let distinct: BTreeSet<MailboxId> = copies.iter().copied().collect();
    origin == AppendOrigin::New && distinct.len() == copies.len() && distinct.is_subset(placed_in)
}

impl MailAccount {
    /// Removes every copy of an email from the server.
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an unknown email,
    /// [`MailError::Unsupported`] without UIDPLUS, and connection errors.
    /// Copies removed before an error stay removed.
    pub async fn destroy(&self, email: EmailId) -> Result<Recorded, MailError> {
        let mut recorded = Recorded::default();
        let locations = self.email_locations(email).await?;
        // Forgotten before the expunges too: one failing half-way must not
        // leave the message served from the cache.
        self.shared.cache.forget(self.id, email);
        for location in locations {
            recorded.merge(self.expunge_copy(&location).await?);
        }
        self.shared.cache.forget(self.id, email);
        Ok(recorded)
    }

    /// Takes out an email just appended (and partly put in `placed_in`)
    /// after a later step failed, so a create is whole or not at all. Only
    /// if it is surely new and has no copy elsewhere; otherwise it is
    /// [`Discarded::Kept`]: the user's existing email is worth more than a
    /// half-done create.
    ///
    /// # Errors
    /// As [`Self::destroy`].
    pub async fn discard_appended(
        &self,
        appended: Appended,
        placed_in: &BTreeSet<MailboxId>,
    ) -> Result<Discarded, MailError> {
        let locations = self.email_locations(appended.email).await?;
        let copies: Vec<MailboxId> = locations.iter().map(|l| l.mailbox).collect();
        if !may_discard(appended.origin, &copies, placed_in) {
            return Ok(Discarded::Kept);
        }
        for location in &locations {
            self.expunge_copy(location).await?;
        }
        self.shared.cache.forget(self.id, appended.email);
        Ok(Discarded::Removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[i64]) -> Vec<MailboxId> {
        list.iter()
            .map(|n| MailboxId::from_raw(*n).unwrap())
            .collect()
    }

    fn set(list: &[i64]) -> BTreeSet<MailboxId> {
        ids(list).into_iter().collect()
    }

    #[test]
    fn only_new_emails_with_our_copies_are_discarded() {
        let placed = set(&[1, 2, 3]);
        // New, appended to 1 and copied to 2 before the failure.
        assert!(may_discard(AppendOrigin::New, &ids(&[1, 2]), &placed));
        assert!(may_discard(AppendOrigin::New, &ids(&[1]), &placed));
        // A deduplicating server answered with an existing email.
        assert!(!may_discard(
            AppendOrigin::MaybeExisting,
            &ids(&[1]),
            &placed
        ));
        // A copy somewhere we weren't placing it: the user's.
        assert!(!may_discard(AppendOrigin::New, &ids(&[1, 9]), &placed));
        // Two copies in one mailbox: an older one was joined to it.
        assert!(!may_discard(AppendOrigin::New, &ids(&[1, 1]), &placed));
    }
}
