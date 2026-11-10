// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Thread (RFC 8621 §3).

use serde::Serialize;

use crate::macros::string_enum;
use crate::types::{EmailId, ThreadId};

string_enum! {
    /// Thread properties.
    pub enum ThreadProperty ("Thread property") {
        /// `id`.
        Id = "id",
        /// `emailIds`.
        EmailIds = "emailIds",
    }
}

/// A Thread as returned by `Thread/get`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<ThreadId>,
    /// `emailIds`, oldest first (by `receivedAt`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_ids: Option<Vec<EmailId>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// RFC 8621 §3.1.
    #[test]
    fn rfc_shape() {
        let thread = Thread {
            id: Some("f123u4".parse().unwrap()),
            email_ids: Some(vec!["eaa623".parse().unwrap(), "f782cbb".parse().unwrap()]),
        };
        assert_eq!(
            serde_json::to_value(&thread).unwrap(),
            json!({"id": "f123u4", "emailIds": ["eaa623", "f782cbb"]})
        );
    }
}
