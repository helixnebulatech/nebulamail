// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Email/query` and `Email/queryChanges` (RFC 8621 §4.4, §4.5).

use serde::de;
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

use crate::error::MethodError;
use crate::method::parse::take_argument;
use crate::method::{FilterCondition, QueryArguments, QueryChangesArguments, SortProperty};
use crate::types::{EmailId, Keyword, MailboxId, UnsignedInt, UtcDate};

/// The `header` condition: the field exists, or contains a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderCondition {
    /// Field name (matched case-insensitively).
    pub name: String,
    /// Text the field value must contain; `None`: the field must exist.
    pub value: Option<String>,
}

impl<'de> Deserialize<'de> for HeaderCondition {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut items = Vec::<String>::deserialize(deserializer)?.into_iter();
        let (Some(name), value, None) = (items.next(), items.next(), items.next()) else {
            return Err(de::Error::custom("header must be [name] or [name, value]"));
        };
        if name.is_empty() || !name.bytes().all(|b| matches!(b, 33..=57 | 59..=126)) {
            return Err(de::Error::custom("invalid header field name"));
        }
        Ok(Self { name, value })
    }
}

/// Email `FilterCondition` (RFC 8621 §4.4.1). Every member is optional; all
/// given ones must match.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailFilterCondition {
    /// In this mailbox.
    #[serde(default)]
    pub in_mailbox: Option<MailboxId>,
    /// In some mailbox other than these.
    #[serde(default)]
    pub in_mailbox_other_than: Option<Vec<MailboxId>>,
    /// `receivedAt` before this.
    #[serde(default)]
    pub before: Option<UtcDate>,
    /// `receivedAt` at or after this.
    #[serde(default)]
    pub after: Option<UtcDate>,
    /// `size` at least this.
    #[serde(default)]
    pub min_size: Option<UnsignedInt>,
    /// `size` less than this.
    #[serde(default)]
    pub max_size: Option<UnsignedInt>,
    /// Every email of the thread has this keyword.
    #[serde(default)]
    pub all_in_thread_have_keyword: Option<Keyword>,
    /// Some email of the thread has this keyword.
    #[serde(default)]
    pub some_in_thread_have_keyword: Option<Keyword>,
    /// No email of the thread has this keyword.
    #[serde(default)]
    pub none_in_thread_have_keyword: Option<Keyword>,
    /// Has this keyword.
    #[serde(default)]
    pub has_keyword: Option<Keyword>,
    /// Does not have this keyword.
    #[serde(default)]
    pub not_keyword: Option<Keyword>,
    /// `hasAttachment` is this.
    #[serde(default)]
    pub has_attachment: Option<bool>,
    /// Text anywhere (headers, bodies, attachment names).
    #[serde(default)]
    pub text: Option<String>,
    /// In `From`.
    #[serde(default)]
    pub from: Option<String>,
    /// In `To`.
    #[serde(default)]
    pub to: Option<String>,
    /// In `Cc`.
    #[serde(default)]
    pub cc: Option<String>,
    /// In `Bcc`.
    #[serde(default)]
    pub bcc: Option<String>,
    /// In `Subject`.
    #[serde(default)]
    pub subject: Option<String>,
    /// In a body part.
    #[serde(default)]
    pub body: Option<String>,
    /// A header field exists / contains a value.
    #[serde(default)]
    pub header: Option<HeaderCondition>,
}

impl FilterCondition for EmailFilterCondition {
    const PROPERTIES: &'static [&'static str] = &[
        "inMailbox",
        "inMailboxOtherThan",
        "before",
        "after",
        "minSize",
        "maxSize",
        "allInThreadHaveKeyword",
        "someInThreadHaveKeyword",
        "noneInThreadHaveKeyword",
        "hasKeyword",
        "notKeyword",
        "hasAttachment",
        "text",
        "from",
        "to",
        "cc",
        "bcc",
        "subject",
        "body",
        "header",
    ];
}

/// Email sort properties (RFC 8621 §4.4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmailSort {
    /// `receivedAt`.
    ReceivedAt,
    /// `size`.
    Size,
    /// `from` (first address's name or email).
    From,
    /// `to`.
    To,
    /// `subject` (base subject, without `Re:` and such).
    Subject,
    /// `sentAt`.
    SentAt,
    /// `hasKeyword` with `keyword`.
    HasKeyword(Keyword),
    /// `allInThreadHaveKeyword` with `keyword`.
    AllInThreadHaveKeyword(Keyword),
    /// `someInThreadHaveKeyword` with `keyword`.
    SomeInThreadHaveKeyword(Keyword),
}

impl EmailSort {
    /// The `property` string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReceivedAt => "receivedAt",
            Self::Size => "size",
            Self::From => "from",
            Self::To => "to",
            Self::Subject => "subject",
            Self::SentAt => "sentAt",
            Self::HasKeyword(_) => "hasKeyword",
            Self::AllInThreadHaveKeyword(_) => "allInThreadHaveKeyword",
            Self::SomeInThreadHaveKeyword(_) => "someInThreadHaveKeyword",
        }
    }
}

impl SortProperty for EmailSort {
    fn parse(property: &str, extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        let keyword = |extra: &mut Map<String, Value>| {
            take_argument::<Keyword>(extra, "keyword")?.ok_or_else(|| {
                MethodError::invalid_arguments(format!("sorting by {property} needs a keyword"))
            })
        };
        Ok(match property {
            "receivedAt" => Self::ReceivedAt,
            "size" => Self::Size,
            "from" => Self::From,
            "to" => Self::To,
            "subject" => Self::Subject,
            "sentAt" => Self::SentAt,
            "hasKeyword" => Self::HasKeyword(keyword(extra)?),
            "allInThreadHaveKeyword" => Self::AllInThreadHaveKeyword(keyword(extra)?),
            "someInThreadHaveKeyword" => Self::SomeInThreadHaveKeyword(keyword(extra)?),
            other => {
                return Err(MethodError::unsupported_sort(format!(
                    "cannot sort emails by {other:?}"
                )));
            }
        })
    }
}

/// `Email/query` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailQueryArguments {
    /// The standard arguments.
    pub query: QueryArguments<EmailFilterCondition, EmailSort, EmailId>,
    /// Return only one email per thread.
    pub collapse_threads: bool,
}

impl EmailQueryArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let collapse_threads = take_argument(&mut arguments, "collapseThreads")?.unwrap_or(false);
        Ok(Self {
            query: QueryArguments::parse(arguments)?,
            collapse_threads,
        })
    }
}

/// `Email/queryChanges` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailQueryChangesArguments {
    /// The standard arguments.
    pub query_changes: QueryChangesArguments<EmailFilterCondition, EmailSort, EmailId>,
    /// The original query's `collapseThreads`.
    pub collapse_threads: bool,
}

impl EmailQueryChangesArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let collapse_threads = take_argument(&mut arguments, "collapseThreads")?.unwrap_or(false);
        Ok(Self {
            query_changes: QueryChangesArguments::parse(arguments)?,
            collapse_threads,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::method::Filter;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// RFC 8620 §3.7 / RFC 8621 §4.10 style query, as the web client sends.
    #[test]
    fn web_client_query() {
        let args = EmailQueryArguments::parse(map(json!({
            "accountId": "A1",
            "calculateTotal": true,
            "collapseThreads": true,
            "filter": {"inMailbox": "id_of_inbox"},
            "limit": 50,
            "position": 0,
            "sort": [{"isAscending": false, "property": "receivedAt"}]
        })))
        .unwrap();
        assert!(args.collapse_threads);
        assert!(matches!(
            args.query.filter,
            Some(Filter::Condition(EmailFilterCondition {
                in_mailbox: Some(_),
                ..
            }))
        ));
        assert_eq!(args.query.sort[0].property, EmailSort::ReceivedAt);
    }

    #[test]
    fn conditions() {
        let condition: EmailFilterCondition = serde_json::from_value(json!({
            "after": "2026-01-01T00:00:00Z",
            "hasKeyword": "$Flagged",
            "header": ["List-Id"],
            "minSize": 10
        }))
        .unwrap();
        assert_eq!(condition.has_keyword, Some(Keyword::flagged()));
        assert_eq!(condition.header.unwrap().value, None);
        let with_value: EmailFilterCondition =
            serde_json::from_value(json!({"header": ["X-A", "b"]})).unwrap();
        assert_eq!(with_value.header.unwrap().value.as_deref(), Some("b"));
        for bad in [
            json!({"header": []}),
            json!({"header": ["a", "b", "c"]}),
            json!({"header": ["a:b"]}),
            json!({"before": "2026-01-01"}),
            json!({"hasKeyword": "a b"}),
        ] {
            assert!(
                serde_json::from_value::<EmailFilterCondition>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn keyword_sorts() {
        let args = EmailQueryArguments::parse(map(json!({
            "accountId": "a",
            "sort": [{"property": "hasKeyword", "keyword": "$flagged"}, {"property": "size"}]
        })))
        .unwrap();
        assert_eq!(
            args.query.sort[0].property,
            EmailSort::HasKeyword(Keyword::flagged())
        );
        assert!(
            EmailQueryArguments::parse(map(json!({
                "accountId": "a", "sort": [{"property": "size", "keyword": "$seen"}]
            })))
            .is_err()
        );
    }

    #[test]
    fn query_changes() {
        let args = EmailQueryChangesArguments::parse(map(json!({
            "accountId": "a", "sinceQueryState": "q", "collapseThreads": true
        })))
        .unwrap();
        assert!(args.collapse_threads);
    }
}
