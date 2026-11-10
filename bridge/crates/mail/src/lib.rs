// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The mailbox over IMAP: everything between the IMAP
//! client and the database. The `jmap` crate reads the store and calls the
//! operations here; this crate keeps the store in step with the server.
//!
//! - [`Mail`]: the service. [`Mail::open`] logs an account in and starts
//!   its live sync; [`Mail::close`] stops it. [`Mail::subscribe`] gives
//!   [`MailEvent`]s (new JMAP states to push); [`Mail::auth_failures`] the
//!   accounts whose password was refused.
//! - [`MailAccount`]: one account. Sync ([`MailAccount::sync_all`],
//!   [`MailAccount::sync_mailbox`], [`MailAccount::check_mailboxes`]) and
//!   operations: keywords, mailboxes of an email (move/copy), destroy,
//!   append, mailbox create/rename/delete/subscribe, raw message, text
//!   search.
//! - Connections go through `net-guard` (port policy, every resolved address
//!   checked, pinned IP) and `nebula-imap` (TLS, limits). Passwords come in
//!   as `SecretString` (unsealed by the caller with `nebula-crypto`); this
//!   crate never reads the store's secrets.
//!
//! Layout as in Stalwart's `email` crate: `model/` is the pure IMAP→JMAP
//! translation (roles, hierarchy, keywords, headers, previews), `sync/` the
//! sync state machine, `op/` one file per operation, `core/` the account,
//! its connections and the live task.

#![forbid(unsafe_code)]

mod config;
mod core;
mod error;
mod event;
mod model;
mod op;
mod service;
mod sync;
#[cfg(test)]
mod test_support;

pub use config::{CertificateCheck, ImapServer, MailConfig, TlsMode};
pub use core::account::MailAccount;
pub use core::alerts::{ALERT_TTL, AccountAlert, MAX_ALERTS};
pub use core::memory::{Copies, Lease, MemoryBudget};
pub use error::{MailError, Unavailable};
pub use event::{MailEvent, StateChanged};
pub use model::header_form::{
    display_name as header_display_name, nfc, raw as header_raw, text as header_text,
};
pub use model::headers::{
    LIST_FIELDS, MAX_HEADER_BYTES, ParsedHeaders, header_block_is_complete, parse as parse_headers,
};
pub use model::layout::{MessageStructure, PartSection, streamable_parts};
pub use model::parsed::{ParsedMessage, media_type};
pub use model::preview::from_message as message_preview;
pub use op::append::{AppendItem, AppendOrigin, Appended};
pub use op::destroy::Discarded;
pub use op::folder::OnDestroy;
pub use op::keywords::KeywordChange;
pub use op::quota::{QuotaResource, QuotaUsage};
pub use op::search::{TextCondition, TextField};
pub use op::stream::{ChunkReader, PartDownload};
pub use service::{Mail, Start};
