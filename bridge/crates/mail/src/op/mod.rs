// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Operations on an account (one file per operation). Each writes to IMAP
//! first, then stores what the server answered, with the JMAP change
//! records in the same transaction, and broadcasts the new states.

pub(crate) mod append;
pub(crate) mod destroy;
pub(crate) mod folder;
pub(crate) mod keywords;
#[cfg(test)]
mod live_tests;
pub(crate) mod mailboxes;
pub(crate) mod quota;
pub(crate) mod raw;
pub(crate) mod report;
pub(crate) mod search;
pub(crate) mod stream;
pub(crate) mod text;

use nebula_imap::proto::types::{MailboxName, UidSet};
use nebula_imap::{Client, SelectOptions, Selected};
use nebula_store::{EmailId, Location};

use crate::MailError;
use crate::core::account::MailAccount;
use crate::core::slot::{Reselect, Slot};

impl MailAccount {
    /// The copies of a visible email (at least one).
    pub(crate) async fn email_locations(&self, email: EmailId) -> Result<Vec<Location>, MailError> {
        let locations = self.store().locations(self.id, email).await?;
        if locations.is_empty() {
            return Err(MailError::NotFound("email"));
        }
        Ok(locations)
    }

    /// [`Self::email_locations`], also forgetting the cached message of an
    /// email that is gone (destroyed, or expunged by another client): the
    /// cache must never serve a message the store no longer shows.
    pub(crate) async fn visible_locations(
        &self,
        email: EmailId,
    ) -> Result<Vec<Location>, MailError> {
        let result = self.email_locations(email).await;
        if matches!(result, Err(MailError::NotFound(_))) {
            self.shared.cache.forget(self.id, email);
        }
        result
    }
}

/// Selects the mailbox of a location and checks that its UIDs are still
/// the ones we stored (same UIDVALIDITY); else the copy is out of date.
pub(crate) async fn select_location<'a>(
    account: &MailAccount,
    slot: &'a mut Slot,
    location: &Location,
) -> Result<&'a mut Client<Selected>, MailError> {
    let (client, _) = slot
        .select(
            account,
            &location.imap_name,
            SelectOptions::default(),
            Reselect::IfOther,
        )
        .await?;
    if location.uid_validity != Some(client.mailbox().uid_validity) {
        return Err(MailError::NotFound(
            "message (mailbox changed, resync pending)",
        ));
    }
    Ok(client)
}

/// Selects a mailbox by name (no UIDVALIDITY check: the caller compares).
pub(crate) async fn select_location_by_name<'a>(
    account: &MailAccount,
    slot: &'a mut Slot,
    name: &MailboxName,
) -> Result<&'a mut Client<Selected>, MailError> {
    let (client, _) = slot
        .select(account, name, SelectOptions::default(), Reselect::IfOther)
        .await?;
    Ok(client)
}

pub(crate) fn single(location: &Location) -> UidSet {
    UidSet::single(location.uid)
}
