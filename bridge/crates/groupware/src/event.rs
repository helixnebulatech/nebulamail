// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Events for push (`EventSource` `StateChange`).

use nebula_store::{AccountId, DataType, Recorded};

use crate::alerts::AlertDue;

/// The JMAP state of some data types of an account moved forward
/// (`Calendar`, `CalendarEvent`, `AddressBook`, `ContactCard`). Read the
/// new states from the store (`Store::state`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChanged {
    /// The account.
    pub account: AccountId,
    /// What changed.
    pub data_types: Vec<DataType>,
}

/// What the groupware layer tells the rest of the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GroupwareEvent {
    /// New states to push.
    StateChanged(StateChanged),
    /// The DAV server refused the login: calendars and contacts are off
    /// for this account until it logs in again. Mail is not affected.
    Unauthorized(AccountId),
    /// A calendar alert is due now: send it to the account's
    /// push connections that asked for `CalendarAlert`.
    CalendarAlert(AlertDue),
}

impl GroupwareEvent {
    pub(crate) fn changed(account: AccountId, recorded: &Recorded) -> Option<Self> {
        if recorded.is_empty() {
            return None;
        }
        Some(Self::StateChanged(StateChanged {
            account,
            data_types: recorded.data_types().collect(),
        }))
    }
}
