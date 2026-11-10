// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! VAPID (RFC 8292): every push carries
//! `Authorization: vapid t=<JWT>, k=<public key>`, an ES256 JWT for the
//! push service's origin (`aud`), valid for [`TOKEN_LIFETIME`] (`exp`,
//! at most 24 h), with the admin's contact (`sub`).

use std::fmt;
use std::str::FromStr;

use crate::WebPushError;
use crate::keys::{VapidKey, encode_base64url};

/// How long a token is valid (RFC 8292 §2: at most 24 h).
pub const TOKEN_LIFETIME_SECS: i64 = 12 * 3600;

/// The JWT header (fixed, written as RFC 8292 §2.4 shows it).
const HEADER: &str = r#"{"typ":"JWT","alg":"ES256"}"#;

/// The contact for push services (the JWT `sub`, RFC 8292 §2.1): a
/// `mailto:` address or an `https:` URL of the admin. Printable ASCII
/// without spaces, at most 255 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact(String);

impl Contact {
    /// The contact.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Contact {
    type Err = WebPushError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let printable = text.bytes().all(|b| b.is_ascii_graphic());
        let mailto = text
            .strip_prefix("mailto:")
            .is_some_and(|address| address.contains('@') && !address.starts_with('@'));
        let https = text
            .strip_prefix("https://")
            .is_some_and(|rest| !rest.is_empty());
        if text.len() > 255 || !printable || !(mailto || https) {
            return Err(WebPushError::InvalidContact);
        }
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Display for Contact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The `Authorization` header value for a push to `audience` (the push
/// service's origin, `https://host[:port]`), issued at `now` (Unix
/// seconds).
///
/// # Errors
/// [`WebPushError::Crypto`] if signing fails.
pub fn authorization(
    key: &VapidKey,
    audience: &str,
    contact: &Contact,
    now: i64,
) -> Result<String, WebPushError> {
    let claims = serde_json::json!({
        "aud": audience,
        "exp": now.saturating_add(TOKEN_LIFETIME_SECS),
        "sub": contact.as_str(),
    });
    let claims = serde_json::to_vec(&claims).map_err(|_| WebPushError::Crypto)?;
    let signing_input = format!(
        "{}.{}",
        encode_base64url(HEADER.as_bytes()),
        encode_base64url(&claims)
    );
    let signature = key.sign(signing_input.as_bytes())?;
    Ok(format!(
        "vapid t={signing_input}.{}, k={}",
        encode_base64url(&signature),
        key.application_server_key()
    ))
}

#[cfg(test)]
mod tests {
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;
    use crate::keys::decode_base64url;

    fn verify(public: &[u8], header: &str) -> serde_json::Value {
        let token = header
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(", k="))
            .map(|(token, _)| token)
            .unwrap();
        let (input, signature) = token.rsplit_once('.').unwrap();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public)
            .verify(input.as_bytes(), &decode_base64url(signature).unwrap())
            .expect("signature verifies");
        let (head, claims) = input.split_once('.').unwrap();
        assert_eq!(decode_base64url(head).unwrap(), HEADER.as_bytes());
        serde_json::from_slice(&decode_base64url(claims).unwrap()).unwrap()
    }

    /// RFC 8292 §2.4: the example token verifies with the example key,
    /// and our encoding of the same claims is the same signing input.
    #[test]
    fn rfc8292_example() {
        let token = concat!(
            "eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9.eyJhdWQiOiJodHRwczovL3",
            "B1c2guZXhhbXBsZS5uZXQiLCJleHAiOjE0NTM1MjM3NjgsInN1YiI6Im1ha",
            "Wx0bzpwdXNoQGV4YW1wbGUuY29tIn0.i3CYb7t4xfxCDquptFOepC9GAu_H",
            "LGkMlMuCGSK2rpiUfnK9ojFwDXb1JrErtmysazNjjvW2L9OkSSHzvoD1oA"
        );
        let k = "BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs";
        let claims = verify(
            &decode_base64url(k).unwrap(),
            &format!("vapid t={token}, k={k}"),
        );
        assert_eq!(claims["aud"], "https://push.example.net");
        assert_eq!(claims["exp"], 1_453_523_768_i64);
        let ours = serde_json::to_vec(&serde_json::json!({
            "aud": "https://push.example.net",
            "exp": 1_453_523_768_i64,
            "sub": "mailto:push@example.com",
        }))
        .unwrap();
        let input = format!(
            "{}.{}",
            encode_base64url(HEADER.as_bytes()),
            encode_base64url(&ours)
        );
        assert!(token.starts_with(&input), "{input}");
    }

    #[test]
    fn our_tokens() {
        let key = VapidKey::generate().unwrap();
        let contact: Contact = "mailto:admin@example.com".parse().unwrap();
        let header = authorization(&key, "https://fcm.googleapis.com", &contact, 1_000).unwrap();
        assert!(header.ends_with(&format!(", k={}", key.application_server_key())));
        let claims = verify(key.public_key(), &header);
        assert_eq!(
            claims,
            serde_json::json!({
                "aud": "https://fcm.googleapis.com",
                "exp": 1_000 + TOKEN_LIFETIME_SECS,
                "sub": "mailto:admin@example.com"
            })
        );
    }

    #[test]
    fn contacts() {
        assert!("mailto:admin@example.com".parse::<Contact>().is_ok());
        assert!("https://example.com/contact".parse::<Contact>().is_ok());
        for bad in [
            "",
            "admin@example.com",
            "mailto:",
            "mailto:@x",
            "http://example.com",
            "mailto:a@b\r\nX: y",
            "mailto:a b@c",
        ] {
            assert!(bad.parse::<Contact>().is_err(), "{bad:?}");
        }
    }
}
