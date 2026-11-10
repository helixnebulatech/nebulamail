// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Message encryption for Web Push (RFC 8291) with the `aes128gcm`
//! content coding (RFC 8188): what the push service relays, only the
//! browser can read.
//!
//! One record (push messages are at most 4096 bytes on the wire), a new
//! ECDH key and salt for every message, and padding to a multiple of
//! [`PAD_BLOCK`] so the size says little about the content.

use aws_lc_rs::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::agreement::{self, ECDH_P256, PrivateKey, UnparsedPublicKey};
use aws_lc_rs::hkdf::{HKDF_SHA256, KeyType, Salt};
use zeroize::Zeroizing;

use crate::WebPushError;
use crate::keys::{AUTH_LEN, ClientKeys, PUBLIC_KEY_LEN};

/// Most bytes of a push message body (RFC 8291 §4: push services must
/// accept 4096).
pub const MAX_BODY: usize = 4096;
/// Length of the `aes128gcm` header with a P-256 key id: salt (16),
/// record size (4), key id length (1), key id (65).
pub const HEADER_LEN: usize = 16 + 4 + 1 + PUBLIC_KEY_LEN;
const TAG_LEN: usize = 16;
/// Most plaintext bytes in one message: the body minus the header, the
/// tag and the padding delimiter.
pub const MAX_PLAINTEXT: usize = MAX_BODY - HEADER_LEN - TAG_LEN - 1;
/// The plaintext (with its delimiter) is padded to a multiple of this.
pub const PAD_BLOCK: usize = 128;
/// The record size written in the header.
const RECORD_SIZE: u32 = 4096;
const SALT_LEN: usize = 16;

/// An HKDF output length.
struct Len(usize);

impl KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn hkdf<const N: usize>(
    salt: &[u8],
    ikm: &[u8],
    info: &[u8],
) -> Result<Zeroizing<[u8; N]>, WebPushError> {
    let mut out = Zeroizing::new([0_u8; N]);
    Salt::new(HKDF_SHA256, salt)
        .extract(ikm)
        .expand(&[info], Len(N))
        .and_then(|okm| okm.fill(out.as_mut()))
        .map_err(|_| WebPushError::Crypto)?;
    Ok(out)
}

/// The content key and nonce (RFC 8291 §3.3–3.4, RFC 8188 §2.2–2.3).
fn derive(
    ecdh_secret: &[u8],
    auth: &[u8; AUTH_LEN],
    ua_public: &[u8; PUBLIC_KEY_LEN],
    as_public: &[u8],
    salt: &[u8],
) -> Result<(Zeroizing<[u8; 16]>, [u8; 12]), WebPushError> {
    let mut key_info = Vec::with_capacity(14 + 2 * PUBLIC_KEY_LEN);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let ikm = hkdf::<32>(auth, ecdh_secret, &key_info)?;
    let cek = hkdf::<16>(salt, ikm.as_ref(), b"Content-Encoding: aes128gcm\0")?;
    let nonce = hkdf::<12>(salt, ikm.as_ref(), b"Content-Encoding: nonce\0")?;
    Ok((cek, *nonce))
}

fn ecdh(private: &PrivateKey, peer: &[u8]) -> Result<Zeroizing<Vec<u8>>, WebPushError> {
    agreement::agree(
        private,
        UnparsedPublicKey::new(&ECDH_P256, peer),
        WebPushError::Crypto,
        |secret| Ok(Zeroizing::new(secret.to_vec())),
    )
}

fn aead_key(cek: &[u8; 16]) -> Result<LessSafeKey, WebPushError> {
    UnboundKey::new(&AES_128_GCM, cek)
        .map(LessSafeKey::new)
        .map_err(|_| WebPushError::Crypto)
}

/// Zero bytes of padding for `len` bytes of plaintext: up to the next
/// multiple of [`PAD_BLOCK`] (delimiter included), never past
/// [`MAX_PLAINTEXT`].
fn padding_for(len: usize) -> usize {
    let with_delimiter = len.saturating_add(1);
    let rounded = with_delimiter
        .div_ceil(PAD_BLOCK)
        .saturating_mul(PAD_BLOCK)
        .min(MAX_PLAINTEXT.saturating_add(1));
    rounded.saturating_sub(with_delimiter)
}

/// Encrypts `plaintext` for the browser with `keys`: a new ECDH key pair
/// and salt every time, padded.
///
/// # Errors
/// [`WebPushError::TooLarge`] above [`MAX_PLAINTEXT`] bytes;
/// [`WebPushError::Crypto`] if the generator or a primitive fails.
pub fn encrypt(plaintext: &[u8], keys: &ClientKeys) -> Result<Vec<u8>, WebPushError> {
    let sender = PrivateKey::generate(&ECDH_P256).map_err(|_| WebPushError::Crypto)?;
    let mut salt = [0_u8; SALT_LEN];
    aws_lc_rs::rand::fill(&mut salt).map_err(|_| WebPushError::Crypto)?;
    encrypt_with(
        plaintext,
        keys,
        &sender,
        &salt,
        padding_for(plaintext.len()),
    )
}

/// [`encrypt`] with a given sender key, salt and padding (the RFC test
/// vector has no padding).
pub(crate) fn encrypt_with(
    plaintext: &[u8],
    keys: &ClientKeys,
    sender: &PrivateKey,
    salt: &[u8; SALT_LEN],
    padding: usize,
) -> Result<Vec<u8>, WebPushError> {
    let record_len = plaintext
        .len()
        .checked_add(1)
        .and_then(|n| n.checked_add(padding))
        .ok_or(WebPushError::TooLarge)?;
    if plaintext.len() > MAX_PLAINTEXT || record_len > MAX_PLAINTEXT.saturating_add(1) {
        return Err(WebPushError::TooLarge);
    }
    let as_public = sender
        .compute_public_key()
        .map_err(|_| WebPushError::Crypto)?;
    let as_public = as_public.as_ref();
    let secret = ecdh(sender, keys.p256dh())?;
    let (cek, nonce) = derive(&secret, keys.auth(), keys.p256dh(), as_public, salt)?;

    let mut record = Vec::with_capacity(record_len.saturating_add(TAG_LEN));
    record.extend_from_slice(plaintext);
    // The last (and only) record ends with 0x02 (RFC 8188 §2).
    record.push(2);
    record.resize(record_len, 0);
    aead_key(&cek)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut record,
        )
        .map_err(|_| WebPushError::Crypto)?;

    let key_id_len = u8::try_from(as_public.len()).map_err(|_| WebPushError::Crypto)?;
    let mut body = Vec::with_capacity(HEADER_LEN.saturating_add(record.len()));
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(key_id_len);
    body.extend_from_slice(as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// Decrypts a one-record `aes128gcm` push message as the browser would,
/// with its private key and `auth` secret. For tests and tools (the
/// bridge only sends); the input is treated as hostile: every length is
/// checked.
///
/// # Errors
/// [`WebPushError::Decrypt`] for anything malformed or that does not
/// authenticate.
pub fn decrypt(
    body: &[u8],
    receiver_private: &[u8],
    auth: &[u8; AUTH_LEN],
) -> Result<Vec<u8>, WebPushError> {
    let bad = || WebPushError::Decrypt;
    if body.len() > MAX_BODY.saturating_mul(16) {
        return Err(bad());
    }
    let (salt, rest) = body.split_at_checked(SALT_LEN).ok_or_else(bad)?;
    let (rs, rest) = rest.split_at_checked(4).ok_or_else(bad)?;
    let rs = u32::from_be_bytes(rs.try_into().map_err(|_| bad())?);
    let (id_len, rest) = rest.split_first().ok_or_else(bad)?;
    let (as_public, record) = rest
        .split_at_checked(usize::from(*id_len))
        .ok_or_else(bad)?;
    let rs = usize::try_from(rs).map_err(|_| bad())?;
    if as_public.len() != PUBLIC_KEY_LEN || record.len() <= TAG_LEN || record.len() > rs {
        return Err(bad());
    }
    let receiver = PrivateKey::from_private_key(&ECDH_P256, receiver_private).map_err(|_| bad())?;
    let ua_public = receiver.compute_public_key().map_err(|_| bad())?;
    let ua_public: [u8; PUBLIC_KEY_LEN] = ua_public.as_ref().try_into().map_err(|_| bad())?;
    let secret = ecdh(&receiver, as_public).map_err(|_| bad())?;
    let (cek, nonce) = derive(&secret, auth, &ua_public, as_public, salt)?;
    let mut buffer = record.to_vec();
    let plain = aead_key(&cek)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut buffer,
        )
        .map_err(|_| bad())?;
    // Strip the zero padding; the delimiter of a last record is 0x02.
    let end = plain.iter().rposition(|byte| *byte != 0).ok_or_else(bad)?;
    let (content, delimiter) = plain.split_at(end);
    if delimiter.first() != Some(&2) {
        return Err(bad());
    }
    Ok(content.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{decode_base64url, encode_base64url};

    const PLAINTEXT: &[u8] = b"When I grow up, I want to be a watermelon";
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    /// RFC 8291 §5, the body of the example request.
    const BODY: &str = concat!(
        "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml",
        "mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT",
        "pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN"
    );

    fn auth() -> [u8; 16] {
        decode_base64url(AUTH).unwrap().try_into().unwrap()
    }

    /// RFC 8291 §5 and Appendix A: the same keys and salt give the same
    /// bytes, and the example decrypts.
    #[test]
    fn rfc8291_vector() {
        let keys = ClientKeys::parse(UA_PUBLIC, AUTH).unwrap();
        let sender =
            PrivateKey::from_private_key(&ECDH_P256, &decode_base64url(AS_PRIVATE).unwrap())
                .unwrap();
        let salt: [u8; 16] = decode_base64url(SALT).unwrap().try_into().unwrap();
        let body = encrypt_with(PLAINTEXT, &keys, &sender, &salt, 0).unwrap();
        assert_eq!(encode_base64url(&body), BODY);
        // 144 bytes (the RFC says `Content-Length: 145`, an erratum).
        assert_eq!(body.len(), 144);
        let ua_private = decode_base64url(UA_PRIVATE).unwrap();
        let decoded = decode_base64url(BODY).unwrap();
        assert_eq!(decrypt(&decoded, &ua_private, &auth()).unwrap(), PLAINTEXT);
    }

    #[test]
    fn random_keys_padding_and_tampering() {
        let keys = ClientKeys::parse(UA_PUBLIC, AUTH).unwrap();
        let ua_private = decode_base64url(UA_PRIVATE).unwrap();
        let one = encrypt(PLAINTEXT, &keys).unwrap();
        let two = encrypt(PLAINTEXT, &keys).unwrap();
        assert_ne!(one, two, "new key and salt every time");
        // Padded to 128 bytes + tag.
        assert_eq!(one.len(), HEADER_LEN + 128 + 16);
        assert_eq!(decrypt(&one, &ua_private, &auth()).unwrap(), PLAINTEXT);
        for index in [0, 20, 30, HEADER_LEN, one.len() - 1] {
            let mut bad = one.clone();
            if let Some(byte) = bad.get_mut(index) {
                *byte ^= 0x40;
            }
            assert!(decrypt(&bad, &ua_private, &auth()).is_err(), "byte {index}");
        }
        assert!(decrypt(&one[..50], &ua_private, &auth()).is_err());
        assert!(decrypt(&[], &ua_private, &auth()).is_err());
        let mut other_auth = auth();
        other_auth[0] ^= 1;
        assert!(decrypt(&one, &ua_private, &other_auth).is_err());
        // Limits.
        let big = vec![b'x'; MAX_PLAINTEXT];
        let sealed = encrypt(&big, &keys).unwrap();
        assert_eq!(sealed.len(), MAX_BODY);
        assert_eq!(decrypt(&sealed, &ua_private, &auth()).unwrap(), big);
        assert!(matches!(
            encrypt(&vec![b'x'; MAX_PLAINTEXT + 1], &keys),
            Err(WebPushError::TooLarge)
        ));
        assert_eq!(padding_for(0), 127);
        assert_eq!(padding_for(127), 0);
        assert_eq!(padding_for(128), 127);
        assert_eq!(padding_for(MAX_PLAINTEXT), 0);
    }
}
