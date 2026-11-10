// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `ENABLE` (RFC 5161).

use super::command;
use crate::EncodeError;

/// An extension that must be enabled before use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EnableExtension {
    /// `CONDSTORE` (RFC 7162).
    CondStore,
    /// `QRESYNC` (RFC 7162; implies CONDSTORE).
    QResync,
    /// `UTF8=ACCEPT` (RFC 6855).
    Utf8Accept,
    /// `OBJECTID+` (IETF draft; Stalwart).
    ObjectIdPlus,
}

impl EnableExtension {
    /// The name on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CondStore => "CONDSTORE",
            Self::QResync => "QRESYNC",
            Self::Utf8Accept => "UTF8=ACCEPT",
            Self::ObjectIdPlus => "OBJECTID+",
        }
    }
}

/// `ENABLE ext...`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enable {
    /// At least one extension.
    pub extensions: Vec<EnableExtension>,
}

command!(Enable, "ENABLE", |enable, writer| {
    if enable.extensions.is_empty() {
        return Err(EncodeError::Empty("ENABLE extensions"));
    }
    writer.raw(b"ENABLE");
    for extension in &enable.extensions {
        writer.space();
        writer.raw(extension.as_str().as_bytes());
    }
    Ok(())
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{EncodeOptions, Tag, encode, test_util::basic};

    #[test]
    fn encodes() {
        let enable = Enable {
            extensions: vec![EnableExtension::QResync, EnableExtension::Utf8Accept],
        };
        assert_eq!(basic(&enable), "A1 ENABLE QRESYNC UTF8=ACCEPT\r\n");
        let empty = Enable {
            extensions: Vec::new(),
        };
        assert!(encode(&empty, Tag::new("A1").unwrap(), EncodeOptions::basic()).is_err());
    }
}
