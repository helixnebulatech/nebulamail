// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `ID` (RFC 2971).

use super::command;

/// `ID NIL` or `ID ("name" "value" ...)`.
///
/// For privacy, prefer `ID NIL` or just a product name: some servers log it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Id {
    /// Field/value pairs; `None` sends `NIL`.
    pub fields: Option<Vec<(String, String)>>,
}

command!(Id, "ID", |id, writer| {
    writer.raw(b"ID ");
    match &id.fields {
        Some(fields) if !fields.is_empty() => {
            writer.raw(b"(");
            for (index, (name, value)) in fields.iter().enumerate() {
                if index > 0 {
                    writer.space();
                }
                writer.string(name.as_bytes(), "ID field")?;
                writer.space();
                writer.string(value.as_bytes(), "ID value")?;
            }
            writer.raw(b")");
        }
        _ => writer.raw(b"NIL"),
    }
    Ok(())
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::test_util::basic;

    #[test]
    fn encodes() {
        assert_eq!(basic(&Id::default()), "A1 ID NIL\r\n");
        let id = Id {
            fields: Some(vec![("name".into(), "Nebula Mail".into())]),
        };
        assert_eq!(basic(&id), "A1 ID (\"name\" \"Nebula Mail\")\r\n");
    }
}
