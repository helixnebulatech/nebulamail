// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The response to a blob upload (RFC 8620 §6.1).

use serde::Serialize;

use crate::types::{AccountId, BlobId, UnsignedInt};

/// JSON body answering a `POST` to the upload URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobUploadResponse {
    /// The account the blob belongs to.
    pub account_id: AccountId,
    /// The new blob's id.
    pub blob_id: BlobId,
    /// The media type the server stored (from `Content-Type`).
    #[serde(rename = "type")]
    pub media_type: String,
    /// Size in bytes.
    pub size: UnsignedInt,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn shape() {
        let response = BlobUploadResponse {
            account_id: "A1".parse().unwrap(),
            blob_id: "B1".parse().unwrap(),
            media_type: "message/rfc822".to_owned(),
            size: UnsignedInt::from(1024_u32),
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({"accountId": "A1", "blobId": "B1", "type": "message/rfc822", "size": 1024})
        );
    }
}
