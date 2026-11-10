// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! ManageSieve client (RFC 5804).
//!
//! - [`SieveServer`] + [`Connector`]: the configured server, reached through
//!   `net-guard` (port policy, every resolved address checked, connection
//!   pinned to the validated IP), as the IMAP client does.
//! - TLS is always on: implicit, or STARTTLS, which is then mandatory (no
//!   STARTTLS offered → no connection). Bytes that arrive together with the
//!   STARTTLS `OK` are refused (possible injection), and the capabilities
//!   seen before TLS are thrown away: the server re-sends them over TLS.
//!   So credentials only ever go out encrypted, and a man in the middle
//!   can't strip `PLAIN` or `STARTTLS` from what we believe.
//! - The connection is a typestate: [`Client<NotAuthenticated>`] →
//!   [`Client<Authenticated>`]; script commands exist only once logged in.
//!   Capabilities are read again after authentication.
//! - Limits on every line and literal, timeouts on every read and command
//!   ([`Limits`]). A `BYE` at any point ends the connection with its text.
//! - Logs carry command names only: never credentials, script names or
//!   script content.

#![forbid(unsafe_code)]

mod client;
mod connection;
mod error;
mod limits;
mod server;

pub use client::{Authenticated, CheckOutcome, Client, NotAuthenticated, OAuthMechanism};
pub use error::{Error, Refusal};
pub use limits::Limits;
pub use nebula_tls::{TlsConnector, TlsVerify};
pub use server::{CertificateCheck, Connector, Credentials, SieveServer, Tls};

/// The protocol types, re-exported.
pub use nebula_managesieve_proto as proto;

#[cfg(test)]
mod tests;
