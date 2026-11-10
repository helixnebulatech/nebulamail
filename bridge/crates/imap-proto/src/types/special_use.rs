// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mailbox attributes and special uses (RFC 3501, RFC 5258, RFC 6154).

/// What a mailbox is for (RFC 6154, plus `\Important` of RFC 8457).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SpecialUse {
    /// `\All`: every message (virtual).
    All,
    /// `\Archive`
    Archive,
    /// `\Drafts`
    Drafts,
    /// `\Flagged`: flagged messages (virtual).
    Flagged,
    /// `\Important` (RFC 8457).
    Important,
    /// `\Junk`
    Junk,
    /// `\Sent`
    Sent,
    /// `\Trash`
    Trash,
}

impl SpecialUse {
    /// The attribute as sent on the wire (`\Sent`…).
    #[must_use]
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::All => "\\All",
            Self::Archive => "\\Archive",
            Self::Drafts => "\\Drafts",
            Self::Flagged => "\\Flagged",
            Self::Important => "\\Important",
            Self::Junk => "\\Junk",
            Self::Sent => "\\Sent",
            Self::Trash => "\\Trash",
        }
    }

    /// Parses `\Sent` and friends (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let name = text.strip_prefix('\\')?;
        let special = match name.to_ascii_lowercase().as_str() {
            "all" => Self::All,
            "archive" => Self::Archive,
            "drafts" => Self::Drafts,
            "flagged" => Self::Flagged,
            "important" => Self::Important,
            "junk" => Self::Junk,
            "sent" => Self::Sent,
            "trash" => Self::Trash,
            _ => return None,
        };
        Some(special)
    }
}

/// An attribute of a LIST response.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MailboxAttribute {
    /// `\Noinferiors`: cannot have children.
    NoInferiors,
    /// `\Noselect`: cannot be selected.
    NoSelect,
    /// `\NonExistent` (RFC 5258).
    NonExistent,
    /// `\Marked`
    Marked,
    /// `\Unmarked`
    Unmarked,
    /// `\HasChildren` (RFC 3348).
    HasChildren,
    /// `\HasNoChildren` (RFC 3348).
    HasNoChildren,
    /// `\Subscribed` (RFC 5258).
    Subscribed,
    /// `\Remote` (RFC 5258).
    Remote,
    /// A special use such as `\Sent`.
    SpecialUse(SpecialUse),
    /// Anything else, as sent.
    Other(String),
}

impl MailboxAttribute {
    /// Parses an attribute (case-insensitive).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if let Some(special) = SpecialUse::parse(text) {
            return Self::SpecialUse(special);
        }
        match text.to_ascii_lowercase().as_str() {
            "\\noinferiors" => Self::NoInferiors,
            "\\noselect" => Self::NoSelect,
            "\\nonexistent" => Self::NonExistent,
            "\\marked" => Self::Marked,
            "\\unmarked" => Self::Unmarked,
            "\\haschildren" => Self::HasChildren,
            "\\hasnochildren" => Self::HasNoChildren,
            "\\subscribed" => Self::Subscribed,
            "\\remote" => Self::Remote,
            _ => Self::Other(text.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_attributes() {
        assert_eq!(
            MailboxAttribute::parse("\\sent"),
            MailboxAttribute::SpecialUse(SpecialUse::Sent)
        );
        assert_eq!(
            MailboxAttribute::parse("\\HasChildren"),
            MailboxAttribute::HasChildren
        );
        assert_eq!(
            MailboxAttribute::parse("\\X"),
            MailboxAttribute::Other("\\X".to_owned())
        );
        assert_eq!(SpecialUse::Trash.as_wire(), "\\Trash");
        assert_eq!(SpecialUse::parse("Trash"), None);
    }
}
