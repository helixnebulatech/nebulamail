// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Events for push (`EventSource` `StateChange`). Refused passwords go
//! through [`Mail::auth_failures`](crate::Mail::auth_failures) instead.

use nebula_store::{AccountId, DataType, EmailId, Recorded};

/// The JMAP state of some data types of an account moved forward. Read the
/// new states from the store (`Store::state`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChanged {
    /// The account.
    pub account: AccountId,
    /// What changed.
    pub data_types: Vec<DataType>,
    /// The new mail that moved `EmailDelivery` (found by a sync; not the
    /// bridge's own writes, not the first sync of a mailbox), for Web
    /// Push's `EmailPush`.
    pub delivered: Vec<EmailId>,
}

/// What the mail layer tells the rest of the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MailEvent {
    /// New states to push.
    StateChanged(StateChanged),
}

impl MailEvent {
    pub(crate) fn changed(account: AccountId, recorded: &Recorded) -> Option<Self> {
        if recorded.is_empty() {
            return None;
        }
        Some(Self::StateChanged(StateChanged {
            account,
            data_types: recorded.data_types().collect(),
            delivered: recorded.delivered().to_vec(),
        }))
    }
}
