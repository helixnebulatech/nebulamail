// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! LOGIN and AUTHENTICATE.
//!
//! Credentials are `secrecy` values. They are copied only into buffers that
//! are wiped on drop, and `Debug` never prints them.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use secrecy::{ExposeSecret, SecretString};
use zeroize::Zeroizing;

use super::writer::SecretBuf;
use super::{RawLine, Writer, command};
use crate::EncodeError;

/// `LOGIN user password`. Only send it over TLS.
pub struct Login<'a> {
    /// The user name.
    pub username: &'a str,
    /// The password.
    pub password: &'a SecretString,
}

impl std::fmt::Debug for Login<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Login { .. }")
    }
}

command!(Login<'_>, "LOGIN", |login, writer| {
    writer.raw(b"LOGIN ");
    writer.astring(login.username.as_bytes(), "user name")?;
    writer.space();
    writer.astring(login.password.expose_secret().as_bytes(), "password")
});

/// A SASL mechanism with its credentials.
pub enum SaslMechanism<'a> {
    /// `PLAIN` (RFC 4616), without an authorization identity.
    Plain {
        /// The user name.
        username: &'a str,
        /// The password.
        password: &'a SecretString,
    },
    /// `XOAUTH2` (Google, Microsoft).
    XOAuth2 {
        /// The user name (email address).
        username: &'a str,
        /// The OAuth access token.
        token: &'a SecretString,
    },
    /// `OAUTHBEARER` (RFC 7628).
    OAuthBearer {
        /// The user name (email address).
        username: &'a str,
        /// The OAuth access token.
        token: &'a SecretString,
    },
}

impl std::fmt::Debug for SaslMechanism<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl SaslMechanism<'_> {
    fn name(&self) -> &'static str {
        match self {
            Self::Plain { .. } => "PLAIN",
            Self::XOAuth2 { .. } => "XOAUTH2",
            Self::OAuthBearer { .. } => "OAUTHBEARER",
        }
    }

    /// The SASL message, before base64. Its exact length is computed
    /// first so the secret is copied into one allocation that never grows.
    fn payload(&self) -> Result<SecretBuf, EncodeError> {
        let out = match self {
            Self::Plain { username, password } => {
                let password = password.expose_secret();
                // NUL separates the fields, so it cannot appear inside them.
                if username.contains('\0') {
                    return Err(EncodeError::Nul("user name"));
                }
                if password.contains('\0') {
                    return Err(EncodeError::Nul("password"));
                }
                let len = 2usize
                    .saturating_add(username.len())
                    .saturating_add(password.len());
                let mut out = SecretBuf::with_capacity(len);
                out.push(0);
                out.extend(username.as_bytes());
                out.push(0);
                out.extend(password.as_bytes());
                out
            }
            Self::XOAuth2 { username, token } => {
                check_oauth(username, token)?;
                let token = token.expose_secret();
                let parts: [&[u8]; 5] = [
                    b"user=",
                    username.as_bytes(),
                    b"\x01auth=Bearer ",
                    token.as_bytes(),
                    b"\x01\x01",
                ];
                let len = parts.iter().map(|part| part.len()).sum();
                let mut out = SecretBuf::with_capacity(len);
                for part in parts {
                    out.extend(part);
                }
                out
            }
            Self::OAuthBearer { username, token } => {
                check_oauth(username, token)?;
                let token = token.expose_secret();
                // RFC 5801 saslname: ',' and '=' are escaped (3 bytes each).
                let name_len: usize = username
                    .bytes()
                    .map(|byte| if matches!(byte, b',' | b'=') { 3 } else { 1 })
                    .sum();
                let prefix: &[u8] = b"n,a=";
                let middle: &[u8] = b",\x01auth=Bearer ";
                let suffix: &[u8] = b"\x01\x01";
                let len = [
                    prefix.len(),
                    name_len,
                    middle.len(),
                    token.len(),
                    suffix.len(),
                ]
                .iter()
                .sum();
                let mut out = SecretBuf::with_capacity(len);
                out.extend(prefix);
                for byte in username.bytes() {
                    match byte {
                        b',' => out.extend(b"=2C"),
                        b'=' => out.extend(b"=3D"),
                        _ => out.push(byte),
                    }
                }
                out.extend(middle);
                out.extend(token.as_bytes());
                out.extend(suffix);
                out
            }
        };
        Ok(out)
    }
}

/// OAuth fields are separated by 0x01 and must be printable.
fn check_oauth(username: &str, token: &SecretString) -> Result<(), EncodeError> {
    let printable = |text: &str| text.bytes().all(|b| (0x20..=0x7e).contains(&b));
    if !printable(username) || username.contains('\0') {
        return Err(EncodeError::Nul("user name"));
    }
    if !printable(token.expose_secret()) {
        return Err(EncodeError::Nul("token"));
    }
    Ok(())
}

/// Where the first SASL message goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InitialResponse {
    /// In the AUTHENTICATE line (`SASL-IR`, RFC 4959): one round trip less.
    Inline,
    /// After the server's empty `+` challenge.
    AfterContinuation,
}

/// `AUTHENTICATE <mechanism> [initial response]`.
#[derive(Debug)]
pub struct Authenticate<'a> {
    /// Mechanism and credentials.
    pub mechanism: SaslMechanism<'a>,
    /// Use `SASL-IR` only if the server advertises it.
    pub initial_response: InitialResponse,
}

command!(Authenticate<'_>, "AUTHENTICATE", |auth, writer| {
    writer.raw(b"AUTHENTICATE ");
    writer.raw(auth.mechanism.name().as_bytes());
    let payload = auth.mechanism.payload()?;
    let encoded = base64_secret(payload.as_slice());
    match auth.initial_response {
        InitialResponse::Inline => {
            writer.space();
            write_sasl(writer, &encoded);
        }
        InitialResponse::AfterContinuation => {
            writer.await_continuation();
            write_sasl(writer, &encoded);
        }
    }
    Ok(())
});

/// Base64 straight into one wiped-on-drop buffer of the exact size
/// (`encode_string` goes through an unwiped stack buffer of its own).
fn base64_secret(payload: &[u8]) -> Zeroizing<Vec<u8>> {
    let len = base64::encoded_len(payload.len(), true).unwrap_or(0);
    let mut encoded = Zeroizing::new(vec![0u8; len]);
    // The buffer has exactly the needed size, so this cannot fail; if it
    // did, the result would be empty and the server would refuse it.
    let written = STANDARD
        .encode_slice(payload, &mut encoded[..])
        .unwrap_or(0);
    encoded.truncate(written);
    encoded
}

fn write_sasl(writer: &mut Writer, encoded: &[u8]) {
    // RFC 4959: an empty initial response is sent as "=".
    if encoded.is_empty() {
        writer.raw(b"=");
    } else {
        writer.raw(encoded);
    }
}

/// The line that cancels an authentication exchange (`*`), for example to
/// answer the error challenge that XOAUTH2 sends before its `NO`.
#[must_use]
pub fn sasl_cancel() -> RawLine {
    RawLine::new(Zeroizing::new(b"*\r\n".to_vec()))
}

/// The answer to an OAUTHBEARER error challenge: a single `%x01`, in base64
/// (`AQ==`), so the server finishes the exchange with its `NO` (RFC 7628
/// §3.2.3). Cancelling with `*` is also allowed there, but IMAP answers it
/// with `BAD` (RFC 3501 §6.2.2), which hides that the token was refused.
#[must_use]
pub fn oauthbearer_error_ack() -> RawLine {
    RawLine::new(Zeroizing::new(b"AQ==\r\n".to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;

    #[test]
    fn login_quotes_and_hides() {
        let password = SecretString::from("p \"w\"");
        let login = Login {
            username: "alice",
            password: &password,
        };
        assert_eq!(basic(&login), "A1 LOGIN alice \"p \\\"w\\\"\"\r\n");
        assert_eq!(format!("{login:?}"), "Login { .. }");
        let password = SecretString::from("a\r\nb");
        let login = Login {
            username: "alice",
            password: &password,
        };
        assert_eq!(basic(&login), "A1 LOGIN alice {4}\r\n<WAIT>a\r\nb\r\n");
    }

    #[test]
    fn payloads_are_allocated_once() {
        let secret = SecretString::from("tok,=en");
        for mechanism in [
            SaslMechanism::Plain {
                username: "a,=b",
                password: &secret,
            },
            SaslMechanism::XOAuth2 {
                username: "a,=b",
                token: &secret,
            },
            SaslMechanism::OAuthBearer {
                username: "a,=b",
                token: &secret,
            },
        ] {
            let payload = mechanism.payload().unwrap();
            assert_eq!(payload.capacity(), payload.len(), "{mechanism:?}");
        }
        assert_eq!(
            &base64_secret(b"\0alice\0secret")[..],
            b"AGFsaWNlAHNlY3JldA=="
        );
        assert!(base64_secret(b"").is_empty());
    }

    #[test]
    fn plain_with_and_without_ir() {
        let password = SecretString::from("secret");
        let mechanism = SaslMechanism::Plain {
            username: "alice",
            password: &password,
        };
        let auth = Authenticate {
            mechanism,
            initial_response: InitialResponse::Inline,
        };
        // base64("\0alice\0secret")
        assert_eq!(
            basic(&auth),
            "A1 AUTHENTICATE PLAIN AGFsaWNlAHNlY3JldA==\r\n"
        );
        let mechanism = SaslMechanism::Plain {
            username: "alice",
            password: &password,
        };
        let auth = Authenticate {
            mechanism,
            initial_response: InitialResponse::AfterContinuation,
        };
        assert_eq!(
            basic(&auth),
            "A1 AUTHENTICATE PLAIN\r\n<WAIT>AGFsaWNlAHNlY3JldA==\r\n"
        );
    }

    #[test]
    fn oauth_payloads() {
        let token = SecretString::from("tok");
        let mechanism = SaslMechanism::XOAuth2 {
            username: "a@b",
            token: &token,
        };
        let payload = mechanism.payload().unwrap();
        assert_eq!(payload.as_slice(), b"user=a@b\x01auth=Bearer tok\x01\x01");
        let mechanism = SaslMechanism::OAuthBearer {
            username: "a,b=c",
            token: &token,
        };
        let payload = mechanism.payload().unwrap();
        assert_eq!(
            payload.as_slice(),
            b"n,a=a=2Cb=3Dc,\x01auth=Bearer tok\x01\x01"
        );
        let bad = SecretString::from("t\x01k");
        let mechanism = SaslMechanism::XOAuth2 {
            username: "a",
            token: &bad,
        };
        assert!(mechanism.payload().is_err());
    }

    #[test]
    fn plain_rejects_nul() {
        let password = SecretString::from("a\0b");
        let mechanism = SaslMechanism::Plain {
            username: "alice",
            password: &password,
        };
        assert_eq!(
            mechanism.payload().err(),
            Some(EncodeError::Nul("password"))
        );
    }
}
