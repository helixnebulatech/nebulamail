// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mailbox names.

use std::borrow::Cow;
use std::fmt;

use super::utf7;
use crate::ValueError;

/// How mailbox names travel on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NameEncoding {
    /// Modified UTF-7 (RFC 3501 §5.1.3): the default.
    ModifiedUtf7,
    /// Plain UTF-8: after `ENABLE UTF8=ACCEPT` (RFC 6855 §3), or with an
    /// `IMAP4rev2`-only server (RFC 9051 §5.1).
    Utf8,
}

/// Whether the name came from the server in a form we could not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Form {
    /// `name` is the real (decoded) name: encode it for the wire.
    Decoded,
    /// The server sent invalid modified UTF-7: `name` is the raw wire form,
    /// sent back unchanged so we can still open the mailbox.
    RawWire,
}

/// A mailbox name, stored as UTF-8 and encoded for the wire when sent.
///
/// `INBOX` is case-insensitive, so any spelling becomes `INBOX`. Names never
/// contain CR, LF or NUL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MailboxName {
    name: Box<str>,
    form: Form,
}

impl MailboxName {
    /// The inbox.
    #[must_use]
    pub fn inbox() -> Self {
        Self {
            name: "INBOX".into(),
            form: Form::Decoded,
        }
    }

    /// A name typed by a user or taken from our database.
    ///
    /// # Errors
    ///
    /// Empty names and names with CR, LF or NUL.
    pub fn new(name: &str) -> Result<Self, ValueError> {
        check(name)?;
        Ok(Self {
            name: normalize_inbox(name).into(),
            form: Form::Decoded,
        })
    }

    /// A name received from the server in `encoding`.
    ///
    /// Invalid modified UTF-7 is kept as is (and sent back as is), because
    /// some servers allow such names and the user must still reach them.
    ///
    /// # Errors
    ///
    /// Empty names and names with CR, LF or NUL.
    pub fn from_wire(wire: &str, encoding: NameEncoding) -> Result<Self, ValueError> {
        check(wire)?;
        let wire = normalize_inbox(wire);
        let (name, form) = match encoding {
            NameEncoding::Utf8 => (wire.to_owned(), Form::Decoded),
            NameEncoding::ModifiedUtf7 => match utf7::decode(wire) {
                Ok(name) => (name, Form::Decoded),
                Err(_) => (wire.to_owned(), Form::RawWire),
            },
        };
        // Decoding can produce controls such as CR (`&AA0-`): refuse them.
        check(&name)?;
        Ok(Self {
            name: name.into(),
            form,
        })
    }

    /// The name as UTF-8 text (or the raw wire form, see [`Self::from_wire`]).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// Whether this is the inbox.
    #[must_use]
    pub fn is_inbox(&self) -> bool {
        &*self.name == "INBOX"
    }

    /// Whether the server sent this name in invalid modified UTF-7, so
    /// [`Self::as_str`] (and `Display`) is its raw wire form and not the
    /// decoded name.
    #[must_use]
    pub fn is_raw_wire(&self) -> bool {
        self.form == Form::RawWire
    }

    /// The child `leaf` of this mailbox, keeping the form of the parent: a
    /// child of a raw-wire name is raw too (`A&B` + `x` → wire `A&B/x`),
    /// so it names a mailbox below the same parent on the server. Building
    /// it with `format!` and [`Self::new`] would re-encode the raw parent
    /// (`A&-B/x`) and point to another mailbox.
    ///
    /// # Errors
    ///
    /// An empty `leaf`, one with the delimiter, CR, LF or NUL.
    pub fn child(&self, delimiter: char, leaf: &str) -> Result<Self, ValueError> {
        check(leaf)?;
        if leaf.contains(delimiter) {
            return Err(ValueError::Invalid("mailbox name"));
        }
        let mut name = String::from(&*self.name);
        name.push(delimiter);
        match self.form {
            Form::Decoded => name.push_str(leaf),
            Form::RawWire => name.push_str(&utf7::encode(leaf)),
        }
        check(&name)?;
        Ok(Self {
            name: name.into(),
            form: self.form,
        })
    }

    /// The name as it must be sent to the server.
    #[must_use]
    pub fn to_wire(&self, encoding: NameEncoding) -> Cow<'_, str> {
        match (self.form, encoding) {
            (Form::RawWire, _) | (Form::Decoded, NameEncoding::Utf8) => Cow::Borrowed(&self.name),
            (Form::Decoded, NameEncoding::ModifiedUtf7) => Cow::Owned(utf7::encode(&self.name)),
        }
    }
}

impl fmt::Display for MailboxName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

fn check(name: &str) -> Result<(), ValueError> {
    if name.is_empty() {
        return Err(ValueError::Empty("mailbox name"));
    }
    if name.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0)) {
        return Err(ValueError::Invalid("mailbox name"));
    }
    Ok(())
}

fn normalize_inbox(name: &str) -> &str {
    if name.eq_ignore_ascii_case("INBOX") {
        "INBOX"
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_is_case_insensitive() {
        assert!(MailboxName::new("inbox").unwrap().is_inbox());
        assert!(
            MailboxName::from_wire("InBoX", NameEncoding::ModifiedUtf7)
                .unwrap()
                .is_inbox()
        );
        assert!(!MailboxName::new("INBOX/Sub").unwrap().is_inbox());
    }

    #[test]
    fn wire_forms() {
        let name = MailboxName::new("Enviados é").unwrap();
        assert_eq!(name.to_wire(NameEncoding::ModifiedUtf7), "Enviados &AOk-");
        assert_eq!(name.to_wire(NameEncoding::Utf8), "Enviados é");
        let back = MailboxName::from_wire("Enviados &AOk-", NameEncoding::ModifiedUtf7).unwrap();
        assert_eq!(back, name);
    }

    #[test]
    fn invalid_utf7_is_kept_raw() {
        let name = MailboxName::from_wire("A&B", NameEncoding::ModifiedUtf7).unwrap();
        assert_eq!(name.as_str(), "A&B");
        assert_eq!(name.to_wire(NameEncoding::ModifiedUtf7), "A&B");
    }

    #[test]
    fn child_of_raw_wire_keeps_raw() {
        let parent = MailboxName::from_wire("A&B", NameEncoding::ModifiedUtf7).unwrap();
        assert!(parent.is_raw_wire());
        let child = parent.child('/', "child").unwrap();
        assert!(child.is_raw_wire());
        assert_eq!(child.to_wire(NameEncoding::ModifiedUtf7), "A&B/child");
        let child = parent.child('/', "é").unwrap();
        assert_eq!(child.to_wire(NameEncoding::ModifiedUtf7), "A&B/&AOk-");
    }

    #[test]
    fn child_of_decoded() {
        let parent = MailboxName::new("café").unwrap();
        assert!(!parent.is_raw_wire());
        let child = parent.child('/', "x").unwrap();
        assert_eq!(child.as_str(), "café/x");
        assert_eq!(child.to_wire(NameEncoding::ModifiedUtf7), "caf&AOk-/x");
        assert!(parent.child('/', "a/b").is_err());
        assert!(parent.child('/', "").is_err());
        assert!(parent.child('/', "a\r\n").is_err());
    }

    #[test]
    fn rejects_controls() {
        assert!(MailboxName::new("").is_err());
        assert!(MailboxName::new("a\r\nb").is_err());
        assert!(MailboxName::new("a\0b").is_err());
        assert!(MailboxName::from_wire("&AA0-", NameEncoding::ModifiedUtf7).is_err());
    }
}
