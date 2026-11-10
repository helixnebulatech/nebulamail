// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Reads for the JMAP methods: the text of a stored resource,
//! opened with the account's data key (only in memory while the account
//! runs), and the user's calendar addresses.

use nebula_store::ResourceId;

use super::account::GroupwareAccount;
use crate::GroupwareError;
use crate::model::calendar::participant_addresses;

impl GroupwareAccount {
    /// The text of one of the account's resources (iCalendar or vCard) as
    /// bytes, if it exists and is still on the server; opened like
    /// [`Self::resource_text`] (bound to its collection and href).
    ///
    /// # Errors
    /// [`GroupwareError::Store`] on database errors;
    /// [`GroupwareError::Unreadable`] if the copy does not open.
    pub async fn resource_bytes(
        &self,
        resource: ResourceId,
    ) -> Result<Option<Vec<u8>>, GroupwareError> {
        Ok(self
            .resource_text(resource)
            .await?
            .map(|found| found.text.into_bytes()))
    }

    /// The user's `mailto:` addresses from `calendar-user-address-set`,
    /// once discovered (empty before).
    #[must_use]
    pub fn calendar_addresses(&self) -> Vec<String> {
        self.discovered()
            .map(|found| participant_addresses(found.user_addresses.iter().map(String::as_str)))
            .unwrap_or_default()
    }
}
