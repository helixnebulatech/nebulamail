// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Web Push from the bridge (extension of 2026-10-28): what
//! Stalwart does for JMAP `PushSubscription` (RFC 8620 §7.2), so the web
//! app's notifications with every tab closed work through the bridge.
//!
//! - [`keys`]: the browser's keys ([`ClientKeys`]) and the bridge's VAPID
//!   key pair ([`VapidKey`]);
//! - [`ece`]: message encryption, RFC 8291 `aes128gcm`;
//! - [`vapid`]: the ES256 JWT of RFC 8292 with the admin's [`Contact`];
//! - [`endpoint`]: which push URLs are accepted ([`EndpointPolicy`], by
//!   default only the browsers' push services);
//! - [`client`]: one guarded POST ([`PushClient::send`]).
//!
//! Queues, retries and the JMAP side are in `nebula-jmap`.
//!
//! Crypto is aws-lc-rs (the provider rustls already uses): ECDH and
//! ECDSA on P-256, HKDF-SHA-256, AES-128-GCM.

#![forbid(unsafe_code)]

pub mod client;
pub mod ece;
pub mod endpoint;
mod http_date;
pub mod keys;
pub mod vapid;

pub use client::{Failure, Limits, Message, Outcome, PushClient, Urgency};
pub use endpoint::{EndpointError, EndpointPolicy, HostPattern, KNOWN_PUSH_SERVICES, PushEndpoint};
pub use keys::{ClientKeys, VapidKey};
pub use vapid::Contact;

/// Errors of this crate. None carries a key, a URL or a payload.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WebPushError {
    /// `p256dh` is not a P-256 point, `auth` not 16 bytes, or a private
    /// key is not valid.
    #[error("invalid push keys")]
    InvalidKeys,
    /// The VAPID contact is not a `mailto:` or `https:` URI.
    #[error("invalid VAPID contact")]
    InvalidContact,
    /// The payload does not fit in one push message.
    #[error("push payload too large")]
    TooLarge,
    /// A cryptographic primitive or the random generator failed.
    #[error("cryptographic failure")]
    Crypto,
    /// A message could not be decrypted (tests and tools).
    #[error("push message could not be decrypted")]
    Decrypt,
    /// The TLS setup failed.
    #[error(transparent)]
    Tls(#[from] nebula_tls::TlsError),
}
