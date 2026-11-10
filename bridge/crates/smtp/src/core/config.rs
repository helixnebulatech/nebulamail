// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! How to talk to one SMTP submission server.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::time::Duration;

use lettre::transport::smtp::extension::ClientId;
use rustls::pki_types::ServerName;
use secrecy::{ExposeSecret, SecretString};

use crate::error::SmtpError;

/// How TLS starts. There is no plaintext option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Security {
    /// TLS from the first byte (port 465, RFC 8314). Preferred.
    ImplicitTls,
    /// Plain greeting, then `STARTTLS`, which is required (port 587).
    StartTls,
}

/// How the server certificate is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CertificateCheck {
    /// The operating system's verifier and trust store
    /// (`rustls-platform-verifier`), with the [`TlsHost`] as the name.
    #[default]
    Platform,
    /// No check at all, for a development server with a self-signed
    /// certificate. Refused unless the connection's peer address is loopback
    /// ([`SmtpError::InsecureTlsNotLoopback`]), and logged as a warning.
    InsecureLoopbackOnly,
}

/// The server's host name, used for SNI and to check the certificate. It is
/// the configured name, never something from DNS.
#[derive(Clone, PartialEq, Eq)]
pub struct TlsHost {
    name: ServerName<'static>,
    text: String,
}

impl TlsHost {
    /// The name for rustls.
    pub(crate) fn server_name(&self) -> ServerName<'static> {
        self.name.clone()
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl FromStr for TlsHost {
    type Err = SmtpError;

    /// Accepts a DNS name or an IP address.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let name = ServerName::try_from(text.to_owned()).map_err(|_| SmtpError::InvalidHostName)?;
        Ok(Self {
            name,
            text: text.to_owned(),
        })
    }
}

impl fmt::Debug for TlsHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TlsHost").field(&self.text).finish()
    }
}

/// The name we give in `EHLO` (RFC 5321 §4.1.1.1): a domain, or an
/// address literal (§4.1.3) when the client has no primary host name
/// (§4.1.4: "an address literal SHOULD be substituted for the domain
/// name").
///
/// It defaults to the IPv4 loopback literal `[127.0.0.1]`: the bridge's
/// own host name or address would leak into every message's `Received`
/// header, and `localhost` is not a primary host name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloName {
    /// A domain name.
    Domain(String),
    /// An IPv4 address literal, `[192.0.2.1]`.
    Ipv4(Ipv4Addr),
    /// An IPv6 address literal, `[IPv6:2001:db8::1]`.
    Ipv6(Ipv6Addr),
}

impl HelloName {
    /// The name as lettre sends it (it adds the brackets and the `IPv6:`
    /// tag of RFC 5321 §4.1.3).
    pub(crate) fn client_id(&self) -> ClientId {
        match self {
            Self::Domain(name) => ClientId::Domain(name.clone()),
            Self::Ipv4(address) => ClientId::Ipv4(*address),
            Self::Ipv6(address) => ClientId::Ipv6(*address),
        }
    }
}

impl Default for HelloName {
    fn default() -> Self {
        Self::Ipv4(Ipv4Addr::LOCALHOST)
    }
}

impl fmt::Display for HelloName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.client_id(), f)
    }
}

/// `IPv4-address-literal = Snum 3("." Snum)`, `Snum = 1*3DIGIT` from 0 to
/// 255 (RFC 5321 §4.1.3; leading zeros are allowed there, unlike in Rust's
/// parser).
fn ipv4_literal(text: &str) -> Option<Ipv4Addr> {
    let mut octets = [0_u8; 4];
    let mut parts = text.split('.');
    for octet in &mut octets {
        let part = parts.next()?;
        if !(1..=3).contains(&part.len()) || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *octet = part.parse().ok()?;
    }
    parts.next().is_none().then(|| Ipv4Addr::from(octets))
}

impl FromStr for HelloName {
    type Err = SmtpError;

    /// Accepts a domain name (dot-separated labels of letters, digits and
    /// hyphens, at most 253 characters, RFC 5321 §4.1.2) or an IPv4 or
    /// IPv6 address literal (`[192.0.2.1]`, `[IPv6:2001:db8::1]`, §4.1.3;
    /// the tag is case-insensitive, RFC 5234 §2.3). General address
    /// literals (other tags) are refused: none is registered.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Some(literal) = text.strip_prefix('[') {
            let literal = literal
                .strip_suffix(']')
                .ok_or(SmtpError::InvalidHelloName)?;
            if let Some(address) = ipv4_literal(literal) {
                return Ok(Self::Ipv4(address));
            }
            let tagged = literal
                .get(..5)
                .filter(|tag| tag.eq_ignore_ascii_case("IPv6:"))
                .and_then(|_| literal.get(5..))
                .ok_or(SmtpError::InvalidHelloName)?;
            // No zone ids or brackets: Rust's parser takes only the plain
            // forms, which match `IPv6-addr`.
            return tagged
                .parse()
                .map(Self::Ipv6)
                .map_err(|_| SmtpError::InvalidHelloName);
        }
        let label_ok = |label: &str| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        };
        if text.is_empty() || text.len() > 253 || !text.split('.').all(label_ok) {
            return Err(SmtpError::InvalidHelloName);
        }
        Ok(Self::Domain(text.to_owned()))
    }
}

/// How long to wait for the server: the minimums of RFC 5321 §4.5.3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// Each command's reply: greeting, TLS, `EHLO`, `AUTH`, `MAIL FROM`
    /// and `RCPT TO` (the RFC's 5 minutes).
    pub command: Duration,
    /// `DATA`, until the server says to go on (354).
    pub data_start: Duration,
    /// While the message goes out: the longest time without a single byte
    /// leaving. A slow transfer that keeps moving is never cut.
    pub data_block: Duration,
    /// After the last byte, the server's final reply (it may scan the
    /// message for spam and viruses first).
    pub data_end: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            command: Duration::from_mins(5),
            data_start: Duration::from_mins(2),
            data_block: Duration::from_mins(3),
            data_end: Duration::from_mins(10),
        }
    }
}

/// One submission server, from the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SmtpServer {
    /// Host name for SNI and the certificate check.
    pub host: TlsHost,
    /// Implicit TLS or required STARTTLS.
    pub security: Security,
    /// How to check the certificate.
    pub certificates: CertificateCheck,
    /// Name to give in `EHLO`.
    pub hello: HelloName,
    /// Timeouts.
    pub timeouts: Timeouts,
}

impl SmtpServer {
    /// A server with the safe defaults: platform certificate checks,
    /// `EHLO [127.0.0.1]` and the default timeouts.
    #[must_use]
    pub fn new(host: TlsHost, security: Security) -> Self {
        Self {
            host,
            security,
            certificates: CertificateCheck::default(),
            hello: HelloName::default(),
            timeouts: Timeouts::default(),
        }
    }

    /// Sets how the certificate is checked.
    #[must_use]
    pub const fn with_certificates(mut self, certificates: CertificateCheck) -> Self {
        self.certificates = certificates;
        self
    }

    /// Sets the `EHLO` name.
    #[must_use]
    pub fn with_hello(mut self, hello: HelloName) -> Self {
        self.hello = hello;
        self
    }

    /// Sets the timeouts.
    #[must_use]
    pub const fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }
}

/// A user name and password for `AUTH PLAIN` or `AUTH LOGIN`.
///
/// `Debug` shows only the user name.
pub struct Credentials {
    username: String,
    password: SecretString,
}

impl Credentials {
    /// Creates credentials.
    ///
    /// # Errors
    ///
    /// [`SmtpError::InvalidCredentials`] if either is empty or contains NUL,
    /// CR or LF: PLAIN separates its fields with NUL (RFC 4616), so a NUL
    /// would let the user name pick another authorization identity.
    pub fn new(username: impl Into<String>, password: SecretString) -> Result<Self, SmtpError> {
        let username = username.into();
        let bad = |text: &str| text.is_empty() || text.contains(['\0', '\r', '\n']);
        if bad(&username) || bad(password.expose_secret()) {
            return Err(SmtpError::InvalidCredentials);
        }
        Ok(Self { username, password })
    }

    /// The user name.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// lettre's form. Our patched lettre (`vendor/lettre/PATCHES.md`) wipes
    /// its copies of the password on drop; still, build it right before
    /// `AUTH` and drop it right after.
    pub(crate) fn to_lettre(&self) -> lettre::transport::smtp::authentication::Credentials {
        lettre::transport::smtp::authentication::Credentials::new(
            self.username.clone(),
            self.password.expose_secret().to_owned(),
        )
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert!("smtp.example.com".parse::<TlsHost>().is_ok());
        assert!("127.0.0.1".parse::<TlsHost>().is_ok());
        assert!("::1".parse::<TlsHost>().is_ok());
        assert!("".parse::<TlsHost>().is_err());
        assert!("bad host".parse::<TlsHost>().is_err());
    }

    /// RFC 5321 §4.1.4 and §4.1.3: a domain or an address literal, and the
    /// default is the loopback literal, not `localhost`.
    #[test]
    fn hello_names() {
        assert_eq!(HelloName::default().to_string(), "[127.0.0.1]");
        assert_eq!(HelloName::default().client_id().to_string(), "[127.0.0.1]");
        assert!("mail.example.com".parse::<HelloName>().is_ok());
        let v4 = format!("[{}]", ["192", "0", "2", "1"].join("."));
        assert_eq!(v4.parse::<HelloName>().unwrap().to_string(), v4);
        // Snum allows leading zeros.
        let zeros = format!("[{}]", ["010", "0", "0", "001"].join("."));
        assert_eq!(
            zeros.parse::<HelloName>().unwrap(),
            HelloName::Ipv4(Ipv4Addr::new(10, 0, 0, 1))
        );
        for v6 in ["[IPv6:2001:db8::1]", "[ipv6:2001:db8::1]"] {
            assert_eq!(
                v6.parse::<HelloName>().unwrap().to_string(),
                "[IPv6:2001:db8::1]"
            );
        }
        let mapped = format!("[IPv6:::ffff:{}]", ["192", "0", "2", "1"].join("."));
        assert!(mapped.parse::<HelloName>().is_ok());
        let bad_v4 = |parts: &[&str]| format!("[{}]", parts.join("."));
        for bad in [
            String::new(),
            "a b".to_owned(),
            "-a.com".to_owned(),
            "a..com".to_owned(),
            "a.com\r\nMAIL".to_owned(),
            bad_v4(&["256", "0", "0", "1"]),
            bad_v4(&["1", "2", "3"]),
            bad_v4(&["1", "2", "3", "4", "5"]),
            bad_v4(&["0001", "2", "3", "4"]),
            format!("{}]", bad_v4(&["1", "2", "3", "4"])),
            "[2001:db8::1]".to_owned(),
            "[IPv6:2001:db8::1%eth0]".to_owned(),
            "[IPv6:]".to_owned(),
            "[x-tag:abc]".to_owned(),
            "[IPv6:2001:db8::1".to_owned(),
            "[]".to_owned(),
        ] {
            assert!(bad.parse::<HelloName>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn credentials() {
        let secret = |s: &str| SecretString::from(s);
        assert!(Credentials::new("alice", secret("pw")).is_ok());
        assert!(Credentials::new("", secret("pw")).is_err());
        assert!(Credentials::new("alice", secret("")).is_err());
        assert!(Credentials::new("ali\0ce", secret("pw")).is_err());
        assert!(Credentials::new("alice", secret("p\r\nw")).is_err());
        let debug = format!(
            "{:?}",
            Credentials::new("alice", secret("hunter2")).unwrap()
        );
        assert!(debug.contains("alice"));
        assert!(!debug.contains("hunter2"));
    }
}
