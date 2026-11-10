// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! IMAP client.
//!
//! - The caller opens the TCP connection (through `net-guard`, which pins it
//!   to a validated IP) and hands it to [`Client::connect`].
//! - TLS is rustls with aws-lc-rs and the platform verifier. STARTTLS is
//!   mandatory in [`Tls::StartTls`] mode; credentials are only ever sent over
//!   TLS, because only a TLS stream can reach the authentication step.
//! - The connection is a typestate: [`Client<NotAuthenticated>`] →
//!   [`Client<Authenticated>`] → [`Client<Selected>`]. Commands exist only in
//!   the states where they are valid.
//! - Every response goes through the limits of [`Limits`], and so does the
//!   total that one command receives (past it the connection breaks).
//!   Unsolicited data (EXISTS, EXPUNGE, VANISHED, FETCH…) is kept in an
//!   event buffer ([`Client::take_events`]) of at most
//!   [`Limits::max_events`]; past it the events are dropped and marked
//!   [`Events::overflowed`], so the caller does a full sync.
//! - A command whose future is dropped half way leaves the connection
//!   marked as broken: it is never reused with a command in progress.
//! - Logs carry command names and tags only, never credentials or message
//!   content.
//!
//! ```no_run
//! # async fn example(stream: tokio::net::TcpStream) -> Result<(), nebula_imap::Error> {
//! use secrecy::SecretString;
//! use nebula_imap::{Client, Connected, Credentials, Limits, Tls, TlsConnector, TlsVerify};
//! use nebula_imap::proto::types::MailboxName;
//!
//! let connector = TlsConnector::new(TlsVerify::Platform)?;
//! let server_name = "imap.example.com".try_into().map_err(|_| nebula_imap::Error::InvalidServerName)?;
//! let Connected::NotAuthenticated(client) =
//!     Client::connect(stream, server_name, Tls::Implicit, &connector, Limits::default()).await?
//! else {
//!     return Ok(());
//! };
//! let credentials = Credentials::new("alice", SecretString::from("secret"));
//! let client = client.login(&credentials).await?;
//! let (mut inbox, _changes) = client.select(&MailboxName::inbox(), Default::default()).await?;
//! inbox.noop().await?;
//! inbox.logout().await?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]

mod core;
mod error;
mod limits;
mod op;

pub use crate::core::alert::{AlertSink, MAX_ALERT_CHARS, ServerAlert};
pub use crate::core::client::{
    Authenticated, Client, Connected, Events, LoggedIn, MailboxState, ModSeqState,
    NotAuthenticated, ObjectIdSupport, PermanentFlags, Selected, SyncLevel,
};
pub use crate::core::tls::{Tls, TlsConnector, TlsVerify};
pub use error::{Error, Refusal, StateError};
pub use limits::Limits;
pub use op::append::{AppendManyResult, AppendResult};
pub use op::copy_move::{CopyUidMap, MoveResult};
pub use op::fetch::{FetchResult, FetchSummary};
pub use op::idle::{IdleEvent, IdleSession};
pub use op::list::ListedMailbox;
pub use op::login::{Credentials, OAuthMechanism};
pub use op::quota::QuotaInfo;
pub use op::select::{Resync, SelectOptions, SelectResult};
pub use op::store::StoreResult;

/// The protocol types (commands, responses, values), re-exported.
pub use nebula_imap_proto as proto;

#[cfg(test)]
mod tests;
