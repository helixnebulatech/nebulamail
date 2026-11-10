// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Delivery status notification parameters (DSN, RFC 3461) and the
//! server's `EHLO` extensions.
//!
//! Values are kept decoded. `utf-8` `ORCPT` addresses are written here in
//! the `utf-8-addr-xtext` form (RFC 6533 §3), which is plain ASCII; lettre
//! applies the xtext encoding (RFC 3461 §4) to everything else when it
//! writes the command. Every value is parsed into a closed type, so nothing
//! the client sends reaches the SMTP command line as free text.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::str::FromStr;

/// Why a DSN parameter was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DsnError {
    /// `RET` is not `FULL` or `HDRS`.
    #[error("RET must be FULL or HDRS")]
    Return,
    /// `ENVID` is empty, too long or not printable ASCII.
    #[error("invalid ENVID")]
    EnvelopeId,
    /// `NOTIFY` is not `NEVER` or a list of `SUCCESS`, `FAILURE`, `DELAY`.
    #[error("invalid NOTIFY")]
    Notify,
    /// `ORCPT` is not `addr-type;address`.
    #[error("invalid ORCPT")]
    OriginalRecipient,
}

/// `RET`: what a failure report includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DsnReturn {
    /// `FULL`: the whole message.
    Full,
    /// `HDRS`: only the headers.
    Headers,
}

impl FromStr for DsnReturn {
    type Err = DsnError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("FULL") {
            Ok(Self::Full)
        } else if s.eq_ignore_ascii_case("HDRS") {
            Ok(Self::Headers)
        } else {
            Err(DsnError::Return)
        }
    }
}

impl DsnReturn {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "FULL",
            Self::Headers => "HDRS",
        }
    }
}

/// `ENVID`: an id the reports quote back. Printable ASCII, 1 to 100
/// characters (RFC 3461 §4.4).
#[derive(Clone, PartialEq, Eq)]
pub struct EnvelopeId(String);

impl FromStr for EnvelopeId {
    type Err = DsnError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let printable = s.bytes().all(|b| b.is_ascii_graphic());
        if s.is_empty() || s.len() > 100 || !printable {
            return Err(DsnError::EnvelopeId);
        }
        Ok(Self(s.to_owned()))
    }
}

impl EnvelopeId {
    /// Random bytes in an id the bridge makes itself: 160 bits.
    const RANDOM_BYTES: usize = 20;

    /// A new id for a message whose client gave none, so that reports are
    /// matched by something a recipient never sees and can't forge (unlike
    /// the `Message-ID`): 20 bytes from the operating system's CSPRNG in
    /// lower-case hex. 40 characters of `0-9a-f`, which xtext leaves as
    /// they are (RFC 3461 §4).
    ///
    /// # Errors
    ///
    /// [`RandomError`] if the system's random number generator fails.
    pub fn random() -> Result<Self, RandomError> {
        let mut bytes = [0_u8; Self::RANDOM_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| RandomError)?;
        Ok(Self::from_random_bytes(bytes))
    }

    fn from_random_bytes(bytes: [u8; Self::RANDOM_BYTES]) -> Self {
        let mut id = String::with_capacity(2 * Self::RANDOM_BYTES);
        for byte in bytes {
            // Writing to a String can't fail.
            let _ = write!(id, "{byte:02x}");
        }
        Self(id)
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The system's random number generator failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the system random number generator failed")]
pub struct RandomError;

impl fmt::Debug for EnvelopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("EnvelopeId").field(&self.0).finish()
    }
}

/// `NOTIFY`: when to report on a recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DsnNotify {
    /// `NEVER`.
    Never,
    /// Some of `SUCCESS`, `FAILURE`, `DELAY` (at least one).
    On {
        /// `SUCCESS`.
        success: bool,
        /// `FAILURE`.
        failure: bool,
        /// `DELAY`.
        delay: bool,
    },
}

impl FromStr for DsnNotify {
    type Err = DsnError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("NEVER") {
            return Ok(Self::Never);
        }
        let (mut success, mut failure, mut delay) = (false, false, false);
        for item in s.split(',') {
            let flag = match item.to_ascii_uppercase().as_str() {
                "SUCCESS" => &mut success,
                "FAILURE" => &mut failure,
                "DELAY" => &mut delay,
                _ => return Err(DsnError::Notify),
            };
            if *flag {
                return Err(DsnError::Notify);
            }
            *flag = true;
        }
        Ok(Self::On {
            success,
            failure,
            delay,
        })
    }
}

impl DsnNotify {
    pub(crate) fn to_value(self) -> String {
        match self {
            Self::Never => "NEVER".to_owned(),
            Self::On {
                success,
                failure,
                delay,
            } => [(success, "SUCCESS"), (failure, "FAILURE"), (delay, "DELAY")]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

/// `ORCPT`: the original recipient, `addr-type;address`
/// (e.g. `rfc822;bob@example.org`).
#[derive(Clone, PartialEq, Eq)]
pub struct OriginalRecipient {
    addr_type: String,
    address: String,
}

impl FromStr for OriginalRecipient {
    type Err = DsnError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr_type, address) = s.split_once(';').ok_or(DsnError::OriginalRecipient)?;
        let atom = |t: &str| {
            !t.is_empty()
                && t.len() <= 32
                && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        // `utf-8` addresses (RFC 6533) may have non-ASCII; others are ASCII.
        let utf8 = addr_type.eq_ignore_ascii_case("utf-8");
        let good_char = |c: char| {
            if utf8 {
                !c.is_control() && !c.is_whitespace()
            } else {
                c.is_ascii_graphic()
            }
        };
        if !atom(addr_type) || address.is_empty() || address.len() > Self::MAX_LEN {
            return Err(DsnError::OriginalRecipient);
        }
        if !address.chars().all(good_char) {
            return Err(DsnError::OriginalRecipient);
        }
        let orcpt = Self {
            addr_type: addr_type.to_owned(),
            address: address.to_owned(),
        };
        // The limit holds for what goes on the wire too: encoding can make
        // a character up to 10 bytes long (`\x{10FFFF}`), and a server
        // would refuse the whole `RCPT` line.
        if orcpt.wire_len() > Self::MAX_LEN {
            return Err(DsnError::OriginalRecipient);
        }
        Ok(orcpt)
    }
}

impl OriginalRecipient {
    /// The longest address, decoded and encoded.
    const MAX_LEN: usize = 500;

    fn is_utf8(&self) -> bool {
        self.addr_type.eq_ignore_ascii_case("utf-8")
    }

    /// `addr-type;address`, with a `utf-8` address in `utf-8-addr-xtext`
    /// (RFC 6533 §3): every character outside QCHAR (non-ASCII, `+`, `=`,
    /// `\`) as `\x{HEX}`, upper-case hex without leading zeros and at least
    /// two digits. The result is ASCII with nothing xtext changes, so
    /// lettre's xtext leaves it as it is: valid with and without SMTPUTF8.
    pub(crate) fn to_value(&self) -> String {
        if !self.is_utf8() {
            return format!("{};{}", self.addr_type, self.address);
        }
        let mut value = String::with_capacity(self.addr_type.len() + 1 + self.address.len());
        value.push_str(&self.addr_type);
        value.push(';');
        for c in self.address.chars() {
            if is_qchar(c) {
                value.push(c);
            } else {
                // Writing to a String can't fail.
                let _ = write!(value, "\\x{{{:02X}}}", u32::from(c));
            }
        }
        value
    }

    /// The length of the address as sent: after `utf-8-addr-xtext` for
    /// `utf-8`, after lettre's xtext (`+` and `=` take three bytes; the
    /// parser lets nothing else through that xtext changes) for the rest.
    fn wire_len(&self) -> usize {
        if self.is_utf8() {
            return self
                .to_value()
                .len()
                .saturating_sub(self.addr_type.len() + 1);
        }
        self.address
            .bytes()
            .map(|b| if matches!(b, b'+' | b'=') { 3 } else { 1 })
            .sum()
    }
}

/// QCHAR of RFC 6533 §3: `%x21-2A / %x2C-3C / %x3E-5B / %x5D-7E`, that is
/// printable ASCII except `+`, `=` and `\`.
fn is_qchar(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '+' | '=' | '\\')
}

impl fmt::Debug for OriginalRecipient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OriginalRecipient")
            .field(&self.to_value())
            .finish()
    }
}

/// DSN parameters of `MAIL FROM`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailDsn {
    /// `RET`.
    pub ret: Option<DsnReturn>,
    /// `ENVID`.
    pub envid: Option<EnvelopeId>,
}

impl MailDsn {
    /// Whether there are none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.ret.is_none() && self.envid.is_none()
    }

    /// The ENVID to send: the client's if it gave one; if not, `own`, but
    /// only if `extensions` (the `EHLO` of **this** connection, never an
    /// older one) has `DSN`: a server without DSN would refuse `ENVID=`,
    /// and the message must still go out.
    pub(crate) fn envid_to_send(
        &self,
        extensions: &ServerExtensions,
        own: Option<EnvelopeId>,
    ) -> Option<EnvelopeId> {
        self.envid
            .clone()
            .or_else(|| own.filter(|_| extensions.has("DSN")))
    }
}

/// DSN parameters of one `RCPT TO`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RcptDsn {
    /// `NOTIFY`.
    pub notify: Option<DsnNotify>,
    /// `ORCPT`.
    pub orcpt: Option<OriginalRecipient>,
}

impl RcptDsn {
    /// Whether there are none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.notify.is_none() && self.orcpt.is_none()
    }
}

/// The extensions the server announced in its `EHLO` reply (after TLS):
/// keyword (upper case) → its arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerExtensions(BTreeMap<String, Vec<String>>);

impl ServerExtensions {
    /// Parses the lines of an `EHLO` reply (the first one is the greeting).
    #[must_use]
    pub fn from_ehlo_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut map = BTreeMap::new();
        for line in lines.into_iter().skip(1) {
            let mut words = line.split_ascii_whitespace();
            let Some(keyword) = words.next() else {
                continue;
            };
            let valid = keyword.len() <= 64
                && keyword
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-');
            if !valid || map.len() >= 64 {
                continue;
            }
            let arguments = words.take(16).map(str::to_owned).collect();
            map.insert(keyword.to_ascii_uppercase(), arguments);
        }
        Self(map)
    }

    /// Whether `keyword` (any case) was announced.
    #[must_use]
    pub fn has(&self, keyword: &str) -> bool {
        self.0.contains_key(&keyword.to_ascii_uppercase())
    }

    /// The arguments of `keyword`, if announced.
    #[must_use]
    pub fn arguments(&self, keyword: &str) -> Option<&[String]> {
        self.0.get(&keyword.to_ascii_uppercase()).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dsn_values() {
        assert_eq!("full".parse::<DsnReturn>(), Ok(DsnReturn::Full));
        assert_eq!("HDRS".parse::<DsnReturn>(), Ok(DsnReturn::Headers));
        assert!("ALL".parse::<DsnReturn>().is_err());

        assert_eq!("NEVER".parse::<DsnNotify>(), Ok(DsnNotify::Never));
        let notify: DsnNotify = "failure,Delay".parse().unwrap();
        assert_eq!(notify.to_value(), "FAILURE,DELAY");
        for bad in [
            "",
            "NEVER,SUCCESS",
            "SUCCESS,SUCCESS",
            "SUCCESS, FAILURE",
            "ALWAYS",
        ] {
            assert!(bad.parse::<DsnNotify>().is_err(), "{bad:?}");
        }

        assert!("abc-123".parse::<EnvelopeId>().is_ok());
        for bad in ["", "a b", "a\r\nb", "é", &"x".repeat(101)] {
            assert!(bad.parse::<EnvelopeId>().is_err(), "{bad:?}");
        }

        let orcpt: OriginalRecipient = "rfc822;bob@example.org".parse().unwrap();
        assert_eq!(orcpt.to_value(), "rfc822;bob@example.org");
        assert!("utf-8;bøb@example.org".parse::<OriginalRecipient>().is_ok());
        for bad in [
            "bob@example.org",
            ";bob@example.org",
            "rfc822;",
            "rfc822;bøb@example.org",
            "rfc822;bob@example.org MAIL",
            "rfc 822;bob@example.org",
        ] {
            assert!(bad.parse::<OriginalRecipient>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn orcpt_length_counts_the_encoding() {
        // Too long once encoded: 200 × `ø` is 400 bytes, but 1200 as
        // `\x{F8}`.
        let long = format!("utf-8;{}@x", "ø".repeat(200));
        assert_eq!(
            long.parse::<OriginalRecipient>(),
            Err(DsnError::OriginalRecipient)
        );
        // xtext makes `+` three bytes long.
        let plus = format!("rfc822;{}@x", "+".repeat(200));
        assert_eq!(
            plus.parse::<OriginalRecipient>(),
            Err(DsnError::OriginalRecipient)
        );
        assert!(
            format!("rfc822;{}@x", "+".repeat(160))
                .parse::<OriginalRecipient>()
                .is_ok()
        );
        assert!(
            format!("utf-8;{}@x", "ø".repeat(80))
                .parse::<OriginalRecipient>()
                .is_ok()
        );
    }

    #[test]
    fn utf8_orcpt_is_written_as_utf8_addr_xtext() {
        let orcpt: OriginalRecipient = "utf-8;bø+x=y\\b@ex.example".parse().unwrap();
        assert_eq!(
            orcpt.to_value(),
            "utf-8;b\\x{F8}\\x{2B}x\\x{3D}y\\x{5C}b@ex.example"
        );
        let heart: OriginalRecipient = "UTF-8;❤@x.example".parse().unwrap();
        assert_eq!(heart.to_value(), "UTF-8;\\x{2764}@x.example");
        let astral: OriginalRecipient = "utf-8;\u{10FFFF}@x".parse().unwrap();
        assert_eq!(astral.to_value(), "utf-8;\\x{10FFFF}@x");
        // Other types are left to lettre's xtext.
        let rfc822: OriginalRecipient = "rfc822;a+b@x".parse().unwrap();
        assert_eq!(rfc822.to_value(), "rfc822;a+b@x");
    }

    proptest::proptest! {
        #[test]
        fn utf8_orcpt_values_are_left_alone_by_xtext(
            address in proptest::collection::vec(proptest::prelude::any::<char>(), 1..60),
        ) {
            let address: String = address.into_iter().collect();
            if let Ok(orcpt) = format!("utf-8;{address}").parse::<OriginalRecipient>() {
                let value = orcpt.to_value();
                // ASCII without `+`, `=` or bytes below `!`: what lettre's
                // `XText` writes unchanged.
                proptest::prop_assert!(
                    value.bytes().all(|b| b.is_ascii_graphic() && b != b'+' && b != b'='),
                    "{value:?}"
                );
                proptest::prop_assert!(value.len() <= "utf-8;".len() + OriginalRecipient::MAX_LEN);
            }
        }
    }

    #[test]
    fn random_envelope_ids() {
        let ids: Vec<EnvelopeId> = (0..64).map(|_| EnvelopeId::random().unwrap()).collect();
        for id in &ids {
            assert_eq!(id.as_str().len(), 40);
            assert!(
                id.as_str()
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "{id:?}"
            );
            // A valid ENVID, which xtext leaves as it is.
            assert_eq!(id.as_str().parse::<EnvelopeId>().as_ref(), Ok(id));
        }
        let distinct: std::collections::BTreeSet<&str> =
            ids.iter().map(EnvelopeId::as_str).collect();
        assert_eq!(distinct.len(), ids.len());
        assert_eq!(
            EnvelopeId::from_random_bytes([0x0f; 20]).as_str(),
            "0f".repeat(20)
        );
        assert_eq!(
            EnvelopeId::from_random_bytes([0xa5; 20]).as_str(),
            "a5".repeat(20)
        );
    }

    #[test]
    fn own_envelope_id_only_with_dsn_in_this_ehlo() {
        let with_dsn = ServerExtensions::from_ehlo_lines(["hi", "DSN", "AUTH PLAIN"]);
        let without = ServerExtensions::from_ehlo_lines(["hi", "AUTH PLAIN"]);
        let own: EnvelopeId = "own".parse().unwrap();
        let client: EnvelopeId = "client".parse().unwrap();
        let none = MailDsn::default();
        let given = MailDsn {
            ret: None,
            envid: Some(client.clone()),
        };
        // Ours only if this connection's EHLO has DSN.
        assert_eq!(
            none.envid_to_send(&with_dsn, Some(own.clone())),
            Some(own.clone())
        );
        assert_eq!(none.envid_to_send(&without, Some(own.clone())), None);
        assert_eq!(
            none.envid_to_send(&ServerExtensions::default(), Some(own.clone())),
            None
        );
        assert_eq!(none.envid_to_send(&with_dsn, None), None);
        // The client's always wins (without DSN, `send` refuses it).
        assert_eq!(
            given.envid_to_send(&with_dsn, Some(own.clone())),
            Some(client.clone())
        );
        assert_eq!(given.envid_to_send(&without, Some(own)), Some(client));
    }

    #[test]
    fn ehlo_extensions() {
        let lines = [
            "mail.example.com Hello",
            "PIPELINING",
            "size 10240000",
            "DSN",
            "AUTH PLAIN LOGIN",
            "b@d keyword",
            "",
        ];
        let extensions = ServerExtensions::from_ehlo_lines(lines);
        assert!(extensions.has("dsn"));
        assert!(!extensions.has("MAIL.EXAMPLE.COM"));
        assert_eq!(
            extensions.arguments("SIZE"),
            Some(["10240000".to_owned()].as_slice())
        );
        assert!(!extensions.has("BAD"));
    }
}
