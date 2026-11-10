// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Email property names (for `properties`).

use std::fmt;
use std::str::FromStr;

use super::header::HeaderProperty;
use crate::macros::{serde_via_str, string_enum};

string_enum! {
    /// The fixed Email properties (RFC 8621 §4.1).
    pub enum EmailField ("Email property") {
        /// `id`.
        Id = "id",
        /// `blobId`.
        BlobId = "blobId",
        /// `threadId`.
        ThreadId = "threadId",
        /// `mailboxIds`.
        MailboxIds = "mailboxIds",
        /// `keywords`.
        Keywords = "keywords",
        /// `size`.
        Size = "size",
        /// `receivedAt`.
        ReceivedAt = "receivedAt",
        /// `messageId`.
        MessageId = "messageId",
        /// `inReplyTo`.
        InReplyTo = "inReplyTo",
        /// `references`.
        References = "references",
        /// `sender`.
        Sender = "sender",
        /// `from`.
        From = "from",
        /// `to`.
        To = "to",
        /// `cc`.
        Cc = "cc",
        /// `bcc`.
        Bcc = "bcc",
        /// `replyTo`.
        ReplyTo = "replyTo",
        /// `subject`.
        Subject = "subject",
        /// `sentAt`.
        SentAt = "sentAt",
        /// `hasAttachment`.
        HasAttachment = "hasAttachment",
        /// `preview`.
        Preview = "preview",
        /// `bodyStructure`.
        BodyStructure = "bodyStructure",
        /// `bodyValues`.
        BodyValues = "bodyValues",
        /// `textBody`.
        TextBody = "textBody",
        /// `htmlBody`.
        HtmlBody = "htmlBody",
        /// `attachments`.
        Attachments = "attachments",
        /// `headers`.
        Headers = "headers",
    }
}

/// A property of `Email/get` `properties`: a fixed one or `header:…`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum EmailProperty {
    /// A fixed property.
    Field(EmailField),
    /// A header field property.
    Header(HeaderProperty),
}

impl EmailProperty {
    /// `properties` when the client doesn't send it (RFC 8621 §4.2).
    pub const DEFAULT: &'static [EmailField] = &[
        EmailField::Id,
        EmailField::BlobId,
        EmailField::ThreadId,
        EmailField::MailboxIds,
        EmailField::Keywords,
        EmailField::Size,
        EmailField::ReceivedAt,
        EmailField::MessageId,
        EmailField::InReplyTo,
        EmailField::References,
        EmailField::Sender,
        EmailField::From,
        EmailField::To,
        EmailField::Cc,
        EmailField::Bcc,
        EmailField::ReplyTo,
        EmailField::Subject,
        EmailField::SentAt,
        EmailField::HasAttachment,
        EmailField::Preview,
        EmailField::BodyValues,
        EmailField::TextBody,
        EmailField::HtmlBody,
        EmailField::Attachments,
    ];
}

impl From<EmailField> for EmailProperty {
    fn from(field: EmailField) -> Self {
        Self::Field(field)
    }
}

impl FromStr for EmailProperty {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("header:") {
            return s.parse().map(Self::Header).map_err(|e| format!("{s}: {e}"));
        }
        s.parse().map(Self::Field).map_err(|e| format!("{e}"))
    }
}

impl fmt::Display for EmailProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(field) => f.write_str(field.as_str()),
            Self::Header(header) => f.write_str(header.as_str()),
        }
    }
}

serde_via_str!(EmailProperty);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(
            "subject".parse::<EmailProperty>().unwrap(),
            EmailProperty::Field(EmailField::Subject)
        );
        assert!(matches!(
            "header:List-Unsubscribe:asURLs"
                .parse::<EmailProperty>()
                .unwrap(),
            EmailProperty::Header(_)
        ));
        assert!("header:From:asDate".parse::<EmailProperty>().is_err());
        assert!("Subject".parse::<EmailProperty>().is_err());
    }
}
