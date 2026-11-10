// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `SearchSnippet` (RFC 8621 §5).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::email::EmailFilterCondition;
use crate::error::MethodError;
use crate::method::Filter;
use crate::method::parse::from_arguments;
use crate::types::{AccountId, EmailId};

/// Highlighted text of a search hit. `subject` and `preview` are HTML with
/// `<mark>` around matches; everything else is escaped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippet {
    /// The Email.
    pub email_id: EmailId,
    /// Subject with matches marked, or `None` (`null`) if it has none.
    pub subject: Option<String>,
    /// Body text around matches, or `None` (`null`).
    pub preview: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    account_id: AccountId,
    email_ids: Vec<EmailId>,
}

/// `SearchSnippet/get` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSnippetGetArguments {
    /// The account.
    pub account_id: AccountId,
    /// The search, as in `Email/query`.
    pub filter: Option<Filter<EmailFilterCondition>>,
    /// The Emails to make snippets for.
    pub email_ids: Vec<EmailId>,
}

impl SearchSnippetGetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter`, or `requestTooLarge` if
    /// more than `max_objects` ids are given.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let filter = match arguments.remove("filter") {
            None | Some(Value::Null) => None,
            Some(value) => Some(Filter::parse(value)?),
        };
        let wire: Wire = from_arguments(arguments)?;
        if wire.email_ids.len() > max_objects {
            return Err(MethodError::request_too_large(format!(
                "more than {max_objects} emailIds"
            )));
        }
        Ok(Self {
            account_id: wire.account_id,
            filter,
            email_ids: wire.email_ids,
        })
    }
}

/// `SearchSnippet/get` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippetGetResponse {
    /// The account.
    pub account_id: AccountId,
    /// One snippet per Email found.
    pub list: Vec<SearchSnippet>,
    /// Ids that don't exist (`null` if none).
    #[serde(serialize_with = "crate::types::serde_util::empty_vec_as_null")]
    pub not_found: Vec<EmailId>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;

    /// RFC 8621 §5.2.
    #[test]
    fn rfc_example() {
        let args = SearchSnippetGetArguments::parse(
            json!({
                "accountId": "ue150411c",
                "filter": {"text": "foo"},
                "emailIds": ["M44200ec123de277c0c1ce69c", "M7bcbcb0b58d7729686e83d99",
                             "M28d12783a0969584b6deaac0"]
            })
            .as_object()
            .unwrap()
            .clone(),
            100,
        )
        .unwrap();
        assert_eq!(args.email_ids.len(), 3);
        let response = SearchSnippetGetResponse {
            account_id: args.account_id,
            list: vec![SearchSnippet {
                email_id: "M44200ec123de277c0c1ce69c".parse().unwrap(),
                subject: None,
                preview: None,
            }, SearchSnippet {
                email_id: "M7bcbcb0b58d7729686e83d99".parse().unwrap(),
                subject: Some("The <mark>Foo</mark>sball competition".to_owned()),
                preview: Some("...year the <mark>foo</mark>sball competition will be held in the Stadium de ...".to_owned()),
            }],
            not_found: vec![],
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "ue150411c",
                "list": [{
                    "emailId": "M44200ec123de277c0c1ce69c",
                    "subject": null,
                    "preview": null
                }, {
                    "emailId": "M7bcbcb0b58d7729686e83d99",
                    "subject": "The <mark>Foo</mark>sball competition",
                    "preview": "...year the <mark>foo</mark>sball competition will be held in the Stadium de ..."
                }],
                "notFound": null
            })
        );
    }

    #[test]
    fn limits() {
        let error = SearchSnippetGetArguments::parse(
            json!({"accountId": "a", "emailIds": ["a", "b"]})
                .as_object()
                .unwrap()
                .clone(),
            1,
        )
        .unwrap_err();
        assert_eq!(error.kind(), MethodErrorType::RequestTooLarge);
    }
}
