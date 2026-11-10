// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Server capabilities (RFC 5804 §1.7): the data lines of the greeting and
//! of `CAPABILITY`.

use crate::{DecodeError, Item};

/// Most capability lines we accept in one response.
const MAX_LINES: usize = 256;

/// Most entries in one space-separated list (`SIEVE`, `SASL`, `NOTIFY`).
const MAX_LIST: usize = 512;

/// What a server announces. Names are compared case-insensitively;
/// mechanisms are kept upper-case, Sieve extensions as sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Capabilities {
    /// `IMPLEMENTATION`: server name and version (for logs only).
    pub implementation: Option<String>,
    /// `SASL`: the authentication mechanisms.
    pub sasl: Vec<String>,
    /// `SIEVE`: the Sieve extensions (`fileinto`, `vacation`…).
    pub sieve: Vec<String>,
    /// `STARTTLS` is offered.
    pub starttls: bool,
    /// `MAXREDIRECTS`: most `redirect` actions per script run.
    pub max_redirects: Option<u32>,
    /// `NOTIFY`: the `enotify` methods (`mailto`…).
    pub notify: Vec<String>,
    /// `LANGUAGE`.
    pub language: Option<String>,
    /// `OWNER`: the authenticated user.
    pub owner: Option<String>,
    /// `VERSION`: `1.0` for RFC 5804 servers (which have CHECKSCRIPT,
    /// RENAMESCRIPT and NOOP); absent in older ones.
    pub version: Option<String>,
    /// Anything else, with its value.
    pub other: Vec<(String, Option<String>)>,
}

impl Capabilities {
    /// Reads the data lines of a capability response.
    ///
    /// # Errors
    ///
    /// A line that isn't `string [SP string]`, a value that isn't UTF-8,
    /// a `MAXREDIRECTS` that isn't a number, too many lines or entries.
    pub fn parse(lines: &[Vec<Item>]) -> Result<Self, DecodeError> {
        if lines.len() > MAX_LINES {
            return Err(DecodeError::TooManyItems);
        }
        let mut capabilities = Self::default();
        for line in lines {
            let (name, value) = match line.as_slice() {
                [name] => (name, None),
                [name, value] => (name, Some(value)),
                _ => return Err(DecodeError::Malformed("capability line")),
            };
            let name = name.as_text().ok_or(DecodeError::Utf8("capability"))?;
            let value = value
                .map(|value| value.as_text().ok_or(DecodeError::Utf8("capability")))
                .transpose()?;
            capabilities.add(name, value)?;
        }
        Ok(capabilities)
    }

    fn add(&mut self, name: &str, value: Option<&str>) -> Result<(), DecodeError> {
        let list = |value: Option<&str>, upper: bool| -> Result<Vec<String>, DecodeError> {
            let items: Vec<String> = value
                .unwrap_or_default()
                .split_ascii_whitespace()
                .map(|item| {
                    if upper {
                        item.to_ascii_uppercase()
                    } else {
                        item.to_owned()
                    }
                })
                .collect();
            if items.len() > MAX_LIST {
                return Err(DecodeError::TooManyItems);
            }
            Ok(items)
        };
        match name.to_ascii_uppercase().as_str() {
            "IMPLEMENTATION" => self.implementation = value.map(str::to_owned),
            "SASL" => self.sasl = list(value, true)?,
            "SIEVE" => self.sieve = list(value, false)?,
            "STARTTLS" => self.starttls = true,
            "MAXREDIRECTS" => {
                let number = value
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .ok_or(DecodeError::Malformed("MAXREDIRECTS is not a number"))?;
                self.max_redirects = Some(number);
            }
            "NOTIFY" => self.notify = list(value, false)?,
            "LANGUAGE" => self.language = value.map(str::to_owned),
            "OWNER" => self.owner = value.map(str::to_owned),
            "VERSION" => self.version = value.map(str::to_owned),
            _ => {
                if self.other.len() >= MAX_LINES {
                    return Err(DecodeError::TooManyItems);
                }
                self.other.push((name.to_owned(), value.map(str::to_owned)));
            }
        }
        Ok(())
    }

    /// The server offers this SASL mechanism.
    #[must_use]
    pub fn has_sasl(&self, mechanism: &str) -> bool {
        self.sasl.iter().any(|m| m.eq_ignore_ascii_case(mechanism))
    }

    /// The server has this Sieve extension.
    #[must_use]
    pub fn has_extension(&self, extension: &str) -> bool {
        self.sieve.iter().any(|e| e.eq_ignore_ascii_case(extension))
    }

    /// An RFC 5804 server (`VERSION "1.0"`): it has CHECKSCRIPT,
    /// RENAMESCRIPT and NOOP.
    #[must_use]
    pub fn is_rfc5804(&self) -> bool {
        self.version.is_some()
    }

    /// The lines a server would send (fake servers, round-trip tests).
    #[must_use]
    pub fn to_lines(&self) -> Vec<Vec<Item>> {
        let text = |text: &str| Item::Quoted(text.as_bytes().to_vec());
        let mut lines = Vec::new();
        let mut put = |name: &str, value: Option<String>| {
            lines.push(
                std::iter::once(text(name))
                    .chain(value.as_deref().map(text))
                    .collect(),
            );
        };
        if let Some(value) = &self.implementation {
            put("IMPLEMENTATION", Some(value.clone()));
        }
        if !self.sasl.is_empty() {
            put("SASL", Some(self.sasl.join(" ")));
        }
        if !self.sieve.is_empty() {
            put("SIEVE", Some(self.sieve.join(" ")));
        }
        if self.starttls {
            put("STARTTLS", None);
        }
        if let Some(value) = self.max_redirects {
            put("MAXREDIRECTS", Some(value.to_string()));
        }
        if !self.notify.is_empty() {
            put("NOTIFY", Some(self.notify.join(" ")));
        }
        if let Some(value) = &self.language {
            put("LANGUAGE", Some(value.clone()));
        }
        if let Some(value) = &self.owner {
            put("OWNER", Some(value.clone()));
        }
        if let Some(value) = &self.version {
            put("VERSION", Some(value.clone()));
        }
        for (name, value) in &self.other {
            put(name, value.clone());
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Limits, ResponseDecoder, ServerLine};

    fn data(bytes: &[u8]) -> Vec<Vec<Item>> {
        let mut decoder = ResponseDecoder::new(Limits::default());
        decoder.feed(bytes);
        let mut lines = Vec::new();
        while let Some(line) = decoder.decode().unwrap() {
            if let ServerLine::Data(items) = line {
                lines.push(items);
            }
        }
        lines
    }

    #[test]
    fn dovecot_and_stalwart() {
        let dovecot = Capabilities::parse(&data(
            b"\"IMPLEMENTATION\" \"Dovecot Pigeonhole\"\r\n\
              \"SIEVE\" \"fileinto reject envelope vacation include\"\r\n\
              \"NOTIFY\" \"mailto\"\r\n\"SASL\" \"plain\"\r\n\"STARTTLS\"\r\n\
              \"VERSION\" \"1.0\"\r\nOK \"Dovecot ready.\"\r\n",
        ))
        .unwrap();
        assert_eq!(
            dovecot.implementation.as_deref(),
            Some("Dovecot Pigeonhole")
        );
        assert!(dovecot.has_extension("vacation") && dovecot.has_extension("Include"));
        assert!(!dovecot.has_extension("mailboxid"));
        assert_eq!(dovecot.sasl, ["PLAIN"]);
        assert!(dovecot.starttls && dovecot.is_rfc5804());
        assert_eq!(dovecot.notify, ["mailto"]);
        assert_eq!(dovecot.max_redirects, None);

        let stalwart = Capabilities::parse(&data(
            b"\"IMPLEMENTATION\" \"Stalwart ManageSieve\"\r\n\"VERSION\" \"1.0\"\r\n\
              \"STARTTLS\"\r\n\"SASL\" \"OAUTHBEARER XOAUTH2\"\r\n\
              \"SIEVE\" \"body mailboxid vacation\"\r\n\"MAXREDIRECTS\" \"1\"\r\n\
              \"X-UNKNOWN\"\r\nOK\r\n",
        ))
        .unwrap();
        assert_eq!(stalwart.max_redirects, Some(1));
        assert!(stalwart.has_sasl("oauthbearer") && !stalwart.has_sasl("PLAIN"));
        assert_eq!(stalwart.other, [("X-UNKNOWN".to_owned(), None)]);
        assert_eq!(Capabilities::parse(&stalwart.to_lines()).unwrap(), stalwart);
    }

    #[test]
    fn rejects_bad_capabilities() {
        for bad in [
            &b"\"MAXREDIRECTS\" \"many\"\r\n"[..],
            b"\"SIEVE\" \"a\" \"b\"\r\n",
            b"\"SIEVE\" \"\xff\"\r\n",
            b"\"SIEVE\" ACTIVE\r\n",
        ] {
            assert!(Capabilities::parse(&data(bad)).is_err());
        }
        let many = "x ".repeat(MAX_LIST + 1);
        let line = vec![
            Item::Quoted(b"SIEVE".to_vec()),
            Item::Quoted(many.into_bytes()),
        ];
        assert_eq!(Capabilities::parse(&[line]), Err(DecodeError::TooManyItems));
    }
}
