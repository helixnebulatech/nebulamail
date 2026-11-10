// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Mail data synced from IMAP: mailboxes, emails, threads,
//! their copies on the server and their keywords. Every write that changes
//! something visible over JMAP records it in the change log in the same
//! transaction.

mod changeset;
pub(crate) mod convert;
mod mailbox;
mod message;
mod query;
mod read;
mod thread;
mod types;

pub use message::FlagsApplied;
pub use query::{
    Direction, EmailFilter, EmailQuery, EmailQueryResult, EmailSort, MAX_FILTER_NODES,
    MAX_OTHER_THAN_MAILBOXES, QueryStart, SortKey,
};
pub use thread::MAX_THREAD_REFS;
pub use types::{
    Email, EmailMeta, FlagUpdate, ListedMailbox, Location, Mailbox, MailboxCounts,
    MailboxSyncState, NewMessage, Recorded, Role, SentAt, ServerId, ServerIds, StoredUid,
};

#[cfg(test)]
mod tests;
