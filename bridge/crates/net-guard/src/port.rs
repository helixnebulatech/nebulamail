// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! TCP ports and port policies.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use crate::{BlockReason, ParseError};

/// A TCP port, never 0.
///
/// ```
/// use net_guard::Port;
///
/// let port: Port = "993".parse().unwrap();
/// assert_eq!(port.get(), 993);
/// assert!(Port::try_from(0).is_err());
/// assert!("99999".parse::<Port>().is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Port(u16);

impl Port {
    /// IMAP (STARTTLS).
    pub const IMAP: Self = Self(143);
    /// IMAP over implicit TLS.
    pub const IMAPS: Self = Self(993);
    /// SMTP submission over implicit TLS.
    pub const SUBMISSIONS: Self = Self(465);
    /// SMTP submission (STARTTLS).
    pub const SUBMISSION: Self = Self(587);
    /// `ManageSieve`.
    pub const SIEVE: Self = Self(4190);
    /// HTTP.
    pub const HTTP: Self = Self(80);
    /// HTTPS.
    pub const HTTPS: Self = Self(443);

    /// The port number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for Port {
    type Error = ParseError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if value == 0 {
            Err(ParseError::InvalidPort)
        } else {
            Ok(Self(value))
        }
    }
}

impl FromStr for Port {
    type Err = ParseError;

    /// Decimal digits only (no sign, no spaces).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError::InvalidPort);
        }
        let number = s.parse::<u16>().map_err(|_| ParseError::InvalidPort)?;
        Self::try_from(number)
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The ports a destination may use: a closed list.
///
/// ```
/// use net_guard::{Port, PortPolicy};
///
/// assert!(PortPolicy::MAIL.check(Port::IMAPS).is_ok());
/// assert!(PortPolicy::MAIL.check(Port::HTTP).is_err());
/// assert!(PortPolicy::PROXY.check(Port::HTTP).is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortPolicy {
    ports: Cow<'static, [Port]>,
}

impl PortPolicy {
    /// The bridge: IMAP, SMTP submission, `ManageSieve` and HTTPS (§8.1 rule 2).
    pub const MAIL: Self = Self::fixed(&[
        Port::IMAP,
        Port::IMAPS,
        Port::SUBMISSIONS,
        Port::SUBMISSION,
        Port::SIEVE,
        Port::HTTPS,
    ]);
    /// Image proxy and other public fetches: 80 and 443.
    pub const PROXY: Self = Self::fixed(&[Port::HTTP, Port::HTTPS]);
    /// HTTPS only.
    pub const HTTPS: Self = Self::fixed(&[Port::HTTPS]);

    const fn fixed(ports: &'static [Port]) -> Self {
        Self {
            ports: Cow::Borrowed(ports),
        }
    }

    /// A custom list, for an explicit admin choice (a mail server on a
    /// non-standard port) or for tests. Prefer the constants.
    #[must_use]
    pub fn only(ports: Vec<Port>) -> Self {
        Self {
            ports: Cow::Owned(ports),
        }
    }

    /// Whether `port` is in the list.
    ///
    /// # Errors
    ///
    /// [`BlockReason::Port`] if it is not.
    pub fn check(&self, port: Port) -> Result<(), BlockReason> {
        if self.ports.contains(&port) {
            Ok(())
        } else {
            Err(BlockReason::Port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Port, PortPolicy};

    #[test]
    fn parses_only_plain_decimal() {
        assert_eq!("1".parse::<Port>().unwrap().get(), 1);
        assert_eq!("65535".parse::<Port>().unwrap().get(), 65535);
        for bad in ["", "0", "65536", "+993", "-1", " 993", "0x3e1", "9 9"] {
            assert!(bad.parse::<Port>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn mail_policy_is_the_closed_list() {
        for n in [143, 993, 465, 587, 4190, 443] {
            assert!(PortPolicy::MAIL.check(Port::try_from(n).unwrap()).is_ok());
        }
        for n in [22, 25, 80, 110, 1993, 6379, 8080] {
            assert!(PortPolicy::MAIL.check(Port::try_from(n).unwrap()).is_err());
        }
    }

    #[test]
    fn custom_policy() {
        let policy = PortPolicy::only(vec![Port::try_from(1993).unwrap()]);
        assert!(policy.check(Port::try_from(1993).unwrap()).is_ok());
        assert!(policy.check(Port::IMAPS).is_err());
    }
}
