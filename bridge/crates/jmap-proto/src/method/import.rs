// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Email/import` (RFC 8621 §4.8).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::parse::{PropertyErrors, from_arguments};
use crate::error::{MethodError, SetError};
use crate::object::Email;
use crate::references::IdOrRef;
use crate::types::serde_util::empty_map_as_null;
use crate::types::{AccountId, BlobId, CreationId, Keyword, MailboxId, State, TrueSet, UtcDate};

/// `Email/import` arguments. Each import stays as JSON until
/// [`EmailImport::parse`], so one bad entry fails only itself.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmailImportArguments {
    /// The account.
    pub account_id: AccountId,
    /// If given, must match the current Email state.
    #[serde(default)]
    pub if_in_state: Option<State>,
    /// Creation id → message to import.
    pub emails: BTreeMap<CreationId, Map<String, Value>>,
}

impl EmailImportArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` for more than
    /// `max_objects` emails.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        if parsed.emails.len() > max_objects {
            return Err(MethodError::request_too_large(format!(
                "more than {max_objects} emails to import"
            )));
        }
        Ok(parsed)
    }
}

/// One message to import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailImport {
    /// The uploaded raw message.
    pub blob_id: BlobId,
    /// Mailboxes (at least one).
    pub mailbox_ids: BTreeSet<IdOrRef<MailboxId>>,
    /// Keywords.
    pub keywords: BTreeSet<Keyword>,
    /// `receivedAt` (default: from the last `Received` header, or now).
    pub received_at: Option<UtcDate>,
}

impl EmailImport {
    /// Parses one entry of `emails`.
    ///
    /// # Errors
    /// `invalidProperties` listing every problem.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut blob_id = None;
        let mut mailbox_ids = None;
        let mut keywords = None;
        let mut received_at = None;
        for (key, value) in object {
            match key.as_str() {
                "blobId" => blob_id = errors.parse(&key, value),
                "mailboxIds" => {
                    mailbox_ids = errors.parse::<TrueSet<IdOrRef<MailboxId>>>(&key, value);
                }
                "keywords" => {
                    keywords = errors
                        .parse::<Option<TrueSet<Keyword>>>(&key, value)
                        .flatten();
                }
                "receivedAt" => {
                    received_at = errors.parse::<Option<UtcDate>>(&key, value).flatten();
                }
                _ => errors.invalid(key),
            }
        }
        if mailbox_ids.as_ref().is_some_and(TrueSet::is_empty) {
            errors.invalid_because("mailboxIds", "at least one mailbox is required");
        }
        let blob_id = errors.required("blobId", blob_id);
        let mailbox_ids = errors.required("mailboxIds", mailbox_ids);
        errors.finish()?;
        match (blob_id, mailbox_ids) {
            (Some(blob_id), Some(mailbox_ids)) => Ok(Self {
                blob_id,
                mailbox_ids: mailbox_ids.into_set(),
                keywords: keywords.map(TrueSet::into_set).unwrap_or_default(),
                received_at,
            }),
            // `required` recorded these, so `finish` already returned.
            _ => Err(SetError::invalid_properties(["blobId", "mailboxIds"])),
        }
    }
}

/// `Email/import` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailImportResponse {
    /// The account.
    pub account_id: AccountId,
    /// Email state before, or `None` if unknown.
    pub old_state: Option<State>,
    /// Email state after.
    pub new_state: State,
    /// Creation id → `id`, `blobId`, `threadId`, `size` of the new Email.
    #[serde(serialize_with = "empty_map_as_null")]
    pub created: BTreeMap<CreationId, Email>,
    /// Creation id → why it failed.
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_created: BTreeMap<CreationId, SetError>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;
    use crate::types::UnsignedInt;

    #[test]
    fn parses_imports() {
        let args = EmailImportArguments::parse(
            json!({
                "accountId": "a",
                "emails": {
                    "k1": {"blobId": "b1", "mailboxIds": {"m1": true}, "keywords": {"$seen": true},
                           "receivedAt": "2026-10-24T10:00:00Z"},
                    "k2": {"mailboxIds": {}}
                }
            })
            .as_object()
            .unwrap()
            .clone(),
            10,
        )
        .unwrap();
        let mut emails = args.emails.into_values();
        let first = EmailImport::parse(emails.next().unwrap()).unwrap();
        assert_eq!(first.blob_id.as_str(), "b1");
        assert!(first.keywords.contains(&Keyword::seen()));
        let second = EmailImport::parse(emails.next().unwrap()).unwrap_err();
        assert_eq!(second.kind(), SetErrorType::InvalidProperties);
        assert_eq!(second.properties(), ["mailboxIds", "blobId"]);
    }

    #[test]
    fn response_shape() {
        let mut response = EmailImportResponse {
            account_id: "a".parse().unwrap(),
            old_state: None,
            new_state: State::new("2"),
            created: BTreeMap::new(),
            not_created: BTreeMap::new(),
        };
        response.created.insert(
            "k1".parse().unwrap(),
            Email {
                id: Some("e1".parse().unwrap()),
                blob_id: Some("b1".parse().unwrap()),
                thread_id: Some("t1".parse().unwrap()),
                size: Some(UnsignedInt::from(10_u32)),
                ..Email::default()
            },
        );
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "a", "oldState": null, "newState": "2",
                "created": {"k1": {"id": "e1", "blobId": "b1", "threadId": "t1", "size": 10}},
                "notCreated": null
            })
        );
    }
}
