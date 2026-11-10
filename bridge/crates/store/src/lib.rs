// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Bridge database: accounts, sealed secrets, OAuth codes and
//! tokens, the JMAP change log, and the mail metadata synced from IMAP
//! (mailboxes, emails, threads, keywords, where each copy is). SQLite through
//! diesel; Postgres later.
//!
//! The store keeps secrets and tokens as opaque bytes: sealing and hashing
//! happen in `nebula-crypto`, so a database leak alone reveals no password
//! and no usable token.
//!
//! ```no_run
//! # async fn example() -> Result<(), nebula_store::StoreError> {
//! use nebula_store::{NewAccount, Store, UnixTime};
//!
//! let store = Store::open(std::path::Path::new("bridge.db")).await?;
//! let account = store
//!     .get_or_create_account(
//!         NewAccount { login: "alice", email: "alice@example.com", wrapped_dek: b"wrapped key" },
//!         UnixTime::now()?,
//!     )
//!     .await?;
//! # Ok(()) }
//! ```

#![forbid(unsafe_code)]

mod account;
mod changes;
mod dav;
mod db;
mod devices;
mod error;
mod ids;
mod jmap;
mod mail;
mod oauth;
mod record;
mod schema;
mod sieve;
mod time;

pub use account::{Account, NewAccount, SecretField, WrappedKeyRow};
pub use changes::{Change, ChangeKind, ChangeRetention, Changes, ChangesSince, DataType, Seq};
pub use dav::{
    CalendarExtras, Collection, CollectionKind, CollectionProps, CollectionSharing, CollectionSync,
    Components, DavAccount, DavHref, ExternalUrl, FiredAlert, MAX_SHARES, Parsed, ResourceIndex,
    ResourceQuery, ResourceWrite, Rights, SealedLink, ShareMode, Sharee, StoredResource,
    VcardVersions,
};
pub use db::Store;
pub use devices::MAX_DEVICES_PER_ACCOUNT;
pub use error::StoreError;
pub use ids::{
    AccountId, CollectionId, EmailId, GrantId, IdentityId, MailboxId, PushSubscriptionId,
    ResourceId, SieveScriptId, SubmissionId, ThreadId, UploadId,
};
pub use jmap::{
    Cancel, ClaimedHeld, CreatedPushSubscription, Delivered, DeliveryReport, DueHeld, HeldLimits,
    Identity, IdentityUpdate, MAX_DEVICE_CLIENT_ID, MAX_IDENTITIES, MAX_IDENTITY_ADDRESSES,
    MAX_IDENTITY_NAME, MAX_SIGNATURE, MAX_UPLOADS_PER_ACCOUNT, NewHeld, NewIdentity,
    NewPushSubscription, NewSubmission, PushSubscriptionChange, PushSubscriptionRow,
    RecipientReport, RecipientStatus, Release, ReportAction, ReportKind, SetSignedOut, Submission,
    SystemSecret, UndoState, Upload, UploadInfo, UploadOutcome, report_address_key,
};
pub use mail::{
    Direction, Email, EmailFilter, EmailMeta, EmailQuery, EmailQueryResult, EmailSort, FlagUpdate,
    FlagsApplied, ListedMailbox, Location, MAX_FILTER_NODES, MAX_OTHER_THAN_MAILBOXES,
    MAX_THREAD_REFS, Mailbox, MailboxCounts, MailboxSyncState, NewMessage, QueryStart, Recorded,
    Role, SentAt, ServerId, ServerIds, SortKey, StoredUid,
};
pub use oauth::{
    Code, CodeOutcome, KEEP_USED_CODES, NewCode, NewGrant, NewTokens, REUSE_GRACE, RefreshOutcome,
    Rotated, TokenHash, TokenKind, TokenOwner,
};
pub use sieve::{ListedSieveScript, SieveScriptRow};
pub use time::UnixTime;
