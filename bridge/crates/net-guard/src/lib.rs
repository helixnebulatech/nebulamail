// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! SSRF protection, the Rust twin of `packages/net-guard` (TypeScript).
//!
//! The rules match the TypeScript `net-guard` (stricter in places), and
//! both sides run the vectors in `security/ssrf-vectors.json`.
//!
//! - [`check_ip`]: is an address an allowed destination ([`NetworkAccess`])?
//! - [`Host`] and [`Port`]: parsed from the configuration, WHATWG semantics.
//! - [`resolve`]: resolve once with a [`Resolver`] and check EVERY address.
//! - [`connect`]: all of the above, then a TCP connection to a validated IP
//!   (never re-resolved), with a timeout.
//! - [`parse_target_url`] / [`guard_url`]: the same checks for http(s) URLs.
//!
//! Logs (`tracing`) carry only the host and the reason, never full URLs.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use net_guard::{connect, NetworkAccess, Policy, Port, SystemResolver, Target};
//!
//! // From the configuration: `imap = "imap.example.com"`, `private_network = false`.
//! let target = Target::new("imap.example.com".parse()?, Port::IMAPS);
//! let policy = Policy::mail_server(NetworkAccess::PublicOnly);
//! let stream = connect(&target, &policy, &SystemResolver::new()?).await?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod connect;
mod host;
mod ip;
mod port;
mod reason;
mod resolve;
mod url_guard;

#[cfg(test)]
mod vectors;

pub use connect::{
    CONNECT_TIMEOUT, GuardError, GuardedStream, MAX_ATTEMPTS, Policy, Target, connect,
};
pub use host::{DomainName, Host};
pub use ip::{AllowedIp, BlockedIp, Embedding, IpRange, NetworkAccess, check_ip};
pub use port::{Port, PortPolicy};
pub use reason::BlockReason;
pub use resolve::{
    Addresses, DNS_TIMEOUT, DnsError, LookupFuture, Resolver, SystemResolver, resolve,
};
pub use url_guard::{
    ParsedUrl, Scheme, UrlPolicy, ValidatedUrl, guard_url, parse_target_url, resolve_url,
};

/// Configuration values that are not a valid host or port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Not a host name or IP address.
    #[error("invalid host name or IP address")]
    InvalidHost,
    /// Not a port number from 1 to 65535.
    #[error("invalid port")]
    InvalidPort,
}
