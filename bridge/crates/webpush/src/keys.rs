// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The keys of Web Push: the browser's ([`ClientKeys`], RFC 8291 §2:
//! `p256dh` and `auth`) and the bridge's VAPID key pair ([`VapidKey`],
//! RFC 8292 §3.2).

use std::fmt;

use aws_lc_rs::agreement::{ECDH_P256, ParsedPublicKey, UnparsedPublicKey};
use aws_lc_rs::encoding::AsBigEndian as _;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT};
use zeroize::Zeroizing;

use crate::WebPushError;

/// Length of an uncompressed P-256 point (`0x04 || x || y`).
pub const PUBLIC_KEY_LEN: usize = 65;
/// Length of the `auth` secret (RFC 8291 §3.2).
pub const AUTH_LEN: usize = 16;
/// Length of a P-256 private key (a big-endian scalar).
pub const PRIVATE_KEY_LEN: usize = 32;

/// Decodes base64url with or without padding (browsers and libraries
/// differ).
pub(crate) fn decode_base64url(text: &str) -> Option<Vec<u8>> {
    // 1 KiB of base64 is far more than any key here.
    if text.len() > 1024 {
        return None;
    }
    URL_SAFE_NO_PAD_INDIFFERENT.decode(text.as_bytes()).ok()
}

/// base64url without padding.
pub(crate) fn encode_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Whether `bytes` is an uncompressed point on P-256.
fn is_p256_point(bytes: &[u8]) -> bool {
    bytes.len() == PUBLIC_KEY_LEN
        && bytes.first() == Some(&0x04)
        && ParsedPublicKey::try_from(&UnparsedPublicKey::new(&ECDH_P256, bytes)).is_ok()
}

/// The keys of one browser subscription (`PushSubscription.getKey`):
/// its ECDH public key and the authentication secret. Both are checked
/// when parsed: a point on P-256, 16 bytes of secret.
///
/// `Debug` shows nothing: with them anyone can write messages the
/// browser accepts as ours (if they also had the VAPID key).
#[derive(Clone, PartialEq, Eq)]
pub struct ClientKeys {
    p256dh: [u8; PUBLIC_KEY_LEN],
    auth: Zeroizing<[u8; AUTH_LEN]>,
}

impl fmt::Debug for ClientKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClientKeys(..)")
    }
}

impl ClientKeys {
    /// Length of [`Self::to_bytes`].
    pub const STORED_LEN: usize = PUBLIC_KEY_LEN + AUTH_LEN;

    /// The keys from their base64url forms, as JMAP `keys` carries them
    /// (RFC 8620 §7.2).
    ///
    /// # Errors
    /// [`WebPushError::InvalidKeys`] if either does not decode, `p256dh`
    /// is not an uncompressed point on P-256 or `auth` is not 16 bytes.
    pub fn parse(p256dh: &str, auth: &str) -> Result<Self, WebPushError> {
        let p256dh = decode_base64url(p256dh).ok_or(WebPushError::InvalidKeys)?;
        let auth = Zeroizing::new(decode_base64url(auth).ok_or(WebPushError::InvalidKeys)?);
        Self::from_parts(&p256dh, &auth)
    }

    fn from_parts(p256dh: &[u8], auth: &[u8]) -> Result<Self, WebPushError> {
        if !is_p256_point(p256dh) {
            return Err(WebPushError::InvalidKeys);
        }
        let p256dh: [u8; PUBLIC_KEY_LEN] =
            p256dh.try_into().map_err(|_| WebPushError::InvalidKeys)?;
        let auth: [u8; AUTH_LEN] = auth.try_into().map_err(|_| WebPushError::InvalidKeys)?;
        Ok(Self {
            p256dh,
            auth: Zeroizing::new(auth),
        })
    }

    /// The keys as stored (sealed): `p256dh || auth`.
    #[must_use]
    pub fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(Self::STORED_LEN));
        out.extend_from_slice(&self.p256dh);
        out.extend_from_slice(self.auth.as_ref());
        out
    }

    /// The keys from [`Self::to_bytes`].
    ///
    /// # Errors
    /// [`WebPushError::InvalidKeys`] for any other length or a bad point.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WebPushError> {
        if bytes.len() != Self::STORED_LEN {
            return Err(WebPushError::InvalidKeys);
        }
        let (p256dh, auth) = bytes.split_at(PUBLIC_KEY_LEN);
        Self::from_parts(p256dh, auth)
    }

    /// The browser's ECDH public key.
    #[must_use]
    pub fn p256dh(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.p256dh
    }

    /// The authentication secret.
    #[must_use]
    pub fn auth(&self) -> &[u8; AUTH_LEN] {
        &self.auth
    }
}

/// The bridge's VAPID key pair (ECDSA P-256): it signs the JWT of every
/// push (RFC 8292), and its public key is the `applicationServerKey`
/// browsers subscribe with, so a push service only accepts messages
/// signed by it.
///
/// `Debug` shows only the public key.
pub struct VapidKey {
    pair: EcdsaKeyPair,
    public: [u8; PUBLIC_KEY_LEN],
}

impl fmt::Debug for VapidKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VapidKey")
            .field("public", &self.application_server_key())
            .finish_non_exhaustive()
    }
}

impl VapidKey {
    /// A new random key pair.
    ///
    /// # Errors
    /// [`WebPushError::Crypto`] if the generator fails.
    pub fn generate() -> Result<Self, WebPushError> {
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
            .map_err(|_| WebPushError::Crypto)?;
        Self::from_pair(pair)
    }

    fn from_pair(pair: EcdsaKeyPair) -> Result<Self, WebPushError> {
        let public: [u8; PUBLIC_KEY_LEN] = pair
            .public_key()
            .as_ref()
            .try_into()
            .map_err(|_| WebPushError::Crypto)?;
        Ok(Self { pair, public })
    }

    /// The key pair from its private key (32 bytes, big-endian), as
    /// stored by [`Self::private_bytes`].
    ///
    /// # Errors
    /// [`WebPushError::InvalidKeys`] if it is not a P-256 private key.
    pub fn from_private_bytes(private: &[u8]) -> Result<Self, WebPushError> {
        if private.len() != PRIVATE_KEY_LEN {
            return Err(WebPushError::InvalidKeys);
        }
        let ecdh = aws_lc_rs::agreement::PrivateKey::from_private_key(&ECDH_P256, private)
            .map_err(|_| WebPushError::InvalidKeys)?;
        let public = ecdh
            .compute_public_key()
            .map_err(|_| WebPushError::InvalidKeys)?;
        let pair = EcdsaKeyPair::from_private_key_and_public_key(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            private,
            public.as_ref(),
        )
        .map_err(|_| WebPushError::InvalidKeys)?;
        Self::from_pair(pair)
    }

    /// The private key, to store sealed. Wiped when dropped.
    ///
    /// # Errors
    /// [`WebPushError::Crypto`] if it can't be exported.
    pub fn private_bytes(&self) -> Result<Zeroizing<Vec<u8>>, WebPushError> {
        let bin = self
            .pair
            .private_key()
            .as_be_bytes()
            .map_err(|_| WebPushError::Crypto)?;
        Ok(Zeroizing::new(bin.as_ref().to_vec()))
    }

    /// The public key (uncompressed point).
    #[must_use]
    pub fn public_key(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.public
    }

    /// The public key as base64url: the session's `applicationServerKey`
    /// and the `k` of the `Authorization` header.
    #[must_use]
    pub fn application_server_key(&self) -> String {
        encode_base64url(&self.public)
    }

    /// An ES256 signature (`r || s`, 64 bytes) of `message`.
    pub(crate) fn sign(&self, message: &[u8]) -> Result<Vec<u8>, WebPushError> {
        self.pair
            .sign(&SystemRandom::new(), message)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|_| WebPushError::Crypto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8291 §5: the receiver's keys.
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

    #[test]
    fn client_keys() {
        let keys = ClientKeys::parse(UA_PUBLIC, AUTH).unwrap();
        assert_eq!(keys.auth().len(), 16);
        assert_eq!(ClientKeys::from_bytes(&keys.to_bytes()).unwrap(), keys);
        // Padding is accepted too.
        assert!(ClientKeys::parse(UA_PUBLIC, "BTBZMqHH6r4Tts7J_aSIgg==").is_ok());
        assert_eq!(format!("{keys:?}"), "ClientKeys(..)");
        // Not a point on the curve (last byte changed), compressed, short.
        let mut bad = decode_base64url(UA_PUBLIC).unwrap();
        if let Some(last) = bad.last_mut() {
            *last ^= 1;
        }
        assert!(ClientKeys::parse(&encode_base64url(&bad), AUTH).is_err());
        assert!(ClientKeys::parse(&encode_base64url(&[2; 33]), AUTH).is_err());
        assert!(ClientKeys::parse(UA_PUBLIC, "AAAA").is_err());
        assert!(ClientKeys::parse(UA_PUBLIC, "not base64!").is_err());
        assert!(ClientKeys::from_bytes(&[4; 80]).is_err());
    }

    #[test]
    fn vapid_keys_round_trip() {
        let key = VapidKey::generate().unwrap();
        let private = key.private_bytes().unwrap();
        assert_eq!(private.len(), 32);
        let again = VapidKey::from_private_bytes(&private).unwrap();
        assert_eq!(again.public_key(), key.public_key());
        assert_eq!(key.application_server_key().len(), 87);
        assert!(VapidKey::from_private_bytes(&[0; 32]).is_err());
        assert!(VapidKey::from_private_bytes(&[1; 31]).is_err());
        assert!(!format!("{key:?}").contains(&format!("{:?}", private.as_slice())));
    }

    /// RFC 8291 §5: the sender's private key gives its public key.
    #[test]
    fn rfc_sender_key() {
        let private = decode_base64url("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw").unwrap();
        let key = VapidKey::from_private_bytes(&private).unwrap();
        assert_eq!(
            key.application_server_key(),
            "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8"
        );
    }
}
