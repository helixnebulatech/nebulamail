// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The data lines of `LISTSCRIPTS` (RFC 5804 §2.7).

use crate::{DecodeError, Item, NameError, ScriptName};

/// One script of `LISTSCRIPTS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedScript {
    /// Its name.
    pub name: ScriptName,
    /// It is the active script.
    pub active: bool,
}

impl ListedScript {
    /// Reads one line: `sieve-name [SP "ACTIVE"]`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::Malformed`] for a line of another shape; a name the
    /// RFC forbids comes back as `Ok(Err(…))` so the caller may skip it.
    pub fn parse(line: &[Item]) -> Result<Result<Self, NameError>, DecodeError> {
        let (name, active) = match line {
            [name] => (name, false),
            [name, Item::Atom(flag)] if flag.is("ACTIVE") => (name, true),
            _ => return Err(DecodeError::Malformed("LISTSCRIPTS line")),
        };
        let bytes = name
            .as_bytes()
            .ok_or(DecodeError::Malformed("LISTSCRIPTS line"))?;
        Ok(ScriptName::from_bytes(bytes).map(|name| Self { name, active }))
    }

    /// The line a server would send.
    #[must_use]
    pub fn to_line(&self) -> Vec<Item> {
        let mut line = vec![Item::Quoted(self.name.as_str().as_bytes().to_vec())];
        if self.active {
            line.extend(crate::Atom::new("ACTIVE").map(Item::Atom));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Atom;

    #[test]
    fn lines() {
        let quoted = |s: &[u8]| Item::Quoted(s.to_vec());
        let active = Item::Atom(Atom::new("active").unwrap());
        let one = ListedScript::parse(&[quoted(b"a b"), active.clone()])
            .unwrap()
            .unwrap();
        assert_eq!((one.name.as_str(), one.active), ("a b", true));
        let two = ListedScript::parse(&[Item::Literal(b"x".to_vec())])
            .unwrap()
            .unwrap();
        assert!(!two.active);
        assert_eq!(ListedScript::parse(&two.to_line()).unwrap().unwrap(), two);
        assert_eq!(
            ListedScript::parse(&[quoted(b"bad\x01")]).unwrap(),
            Err(NameError::ControlCharacter)
        );
        for bad in [
            vec![],
            vec![active.clone()],
            vec![quoted(b"a"), quoted(b"ACTIVE")],
            vec![quoted(b"a"), active.clone(), active],
        ] {
            assert!(ListedScript::parse(&bad).is_err());
        }
    }
}
