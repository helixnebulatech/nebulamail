// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP methods on top of the IMAP mailbox (`nebula-mail`)
//! and SMTP (`nebula-smtp`): [`JmapBackend`] implements
//! `nebula_http::Backend`.
//!
//! Layout as in Stalwart's `jmap` crate: one module per object with its
//! methods (`get`, `set`, `query`, `changes`…), `blob` for upload and
//! download, and `core` for the backend, the dispatcher and the ids.
//!
//! ```no_run
//! # async fn example(store: nebula_store::Store, mail: nebula_mail::Mail,
//! #     resolver: std::sync::Arc<dyn net_guard::Resolver>, smtp: nebula_jmap::SmtpSettings) {
//! use nebula_jmap::{JmapBackend, JmapConfig};
//!
//! let backend = JmapBackend::new(store, mail, resolver, JmapConfig::new(smtp));
//! let shutdown = tokio_util::sync::CancellationToken::new();
//! tokio::spawn({
//!     let backend = backend.clone();
//!     async move { backend.run(shutdown).await }
//! });
//! // nebula_http::Server::new(config, store, keyring, backend, shutdown)…
//! # }
//! ```

#![forbid(unsafe_code)]

mod blob;
mod calendars;
mod changes;
mod collation;
mod contacts;
mod core;
mod email;
mod error;
mod identity;
mod mailbox;
mod mdn;
mod paging;
mod principals;
mod push;
mod quota;
mod sharing;
mod sieve;
mod submission;
mod thread;

pub use crate::core::backend::JmapBackend;
pub use crate::core::config::{HoldSettings, JmapConfig, SentCopy, SmtpSettings};
pub use crate::push::WebPushSettings;
pub use crate::sieve::SieveSettings;
pub use error::JmapError;
