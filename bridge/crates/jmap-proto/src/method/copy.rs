// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/copy` (RFC 8620 §5.4) and `Blob/copy` (RFC 8620 §6.3).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::parse::from_arguments;
use crate::error::{MethodError, SetError};
use crate::types::serde_util::empty_map_as_null;
use crate::types::{AccountId, BlobId, CreationId, State};

/// The arguments of a standard `Foo/copy` (RFC 8620 §5.4), with the
/// records to create kept as JSON: the server decides what to do with
/// them only once the accounts are known.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopyArguments {
    /// The account to copy from ("MUST be different from the accountId").
    pub from_account_id: AccountId,
    /// The state the from account must be in.
    #[serde(default)]
    pub if_from_in_state: Option<State>,
    /// The account to copy into.
    pub account_id: AccountId,
    /// The state the target account must be in.
    #[serde(default)]
    pub if_in_state: Option<State>,
    /// Creation id → record (with the `id` in the from account).
    pub create: BTreeMap<CreationId, Map<String, Value>>,
    /// Destroy the originals that were copied.
    #[serde(default)]
    pub on_success_destroy_original: bool,
    /// `ifInState` of the implicit `Foo/set` that destroys them.
    #[serde(default)]
    pub destroy_from_if_in_state: Option<State>,
}

impl CopyArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` beyond `max_objects`.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        if parsed.create.len() > max_objects {
            return Err(MethodError::request_too_large("too many records to copy"));
        }
        Ok(parsed)
    }
}

/// `Blob/copy` arguments (RFC 8620 §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobCopyArguments {
    /// The account to copy blobs from.
    pub from_account_id: AccountId,
    /// The account to copy blobs to.
    pub account_id: AccountId,
    /// The blobs to copy.
    pub blob_ids: Vec<BlobId>,
}

impl BlobCopyArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` beyond `max_objects`.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        if parsed.blob_ids.len() > max_objects {
            return Err(MethodError::request_too_large("too many blobs"));
        }
        Ok(parsed)
    }
}

/// `Blob/copy` response (RFC 8620 §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobCopyResponse {
    /// The account blobs were copied from.
    pub from_account_id: AccountId,
    /// The account blobs were copied to.
    pub account_id: AccountId,
    /// Blob id in the from account → blob id in this one.
    #[serde(serialize_with = "empty_map_as_null")]
    pub copied: BTreeMap<BlobId, BlobId>,
    /// Blob id → why it wasn't copied.
    #[serde(serialize_with = "empty_map_as_null")]
    pub not_copied: BTreeMap<BlobId, SetError>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn copy_arguments() {
        let args = CopyArguments::parse(
            map(json!({
                "fromAccountId": "a", "accountId": "b",
                "create": {"k1": {"id": "e1", "mailboxIds": {"m1": true}}},
                "onSuccessDestroyOriginal": true
            })),
            10,
        )
        .unwrap();
        assert_eq!(args.from_account_id.as_str(), "a");
        assert!(args.on_success_destroy_original);
        assert_eq!(args.create.len(), 1);
        let kind = |value| CopyArguments::parse(map(value), 10).unwrap_err().kind();
        assert_eq!(
            kind(json!({"fromAccountId": "a", "accountId": "b", "create": {}, "x": 1})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "b", "create": {}})),
            MethodErrorType::InvalidArguments
        );
    }

    #[test]
    fn blob_copy() {
        let args = BlobCopyArguments::parse(
            map(json!({"fromAccountId": "a", "accountId": "b", "blobIds": ["bu1"]})),
            10,
        )
        .unwrap();
        assert_eq!(args.blob_ids.len(), 1);
        assert!(
            BlobCopyArguments::parse(
                map(json!({"fromAccountId": "a", "accountId": "b", "blobIds": ["x", "y"]})),
                1,
            )
            .is_err()
        );
        let response = BlobCopyResponse {
            from_account_id: "a".parse().unwrap(),
            account_id: "b".parse().unwrap(),
            copied: BTreeMap::new(),
            not_copied: BTreeMap::from([("bu1".parse().unwrap(), SetError::not_found())]),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({"fromAccountId": "a", "accountId": "b", "copied": null,
                   "notCopied": {"bu1": {"type": "notFound"}}})
        );
    }
}
