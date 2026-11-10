// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The IMAP server's `[ALERT]`s in the JMAP session (RFC 3501 §7.1, RFC
//! 9051 §7.1: they MUST be presented to the user).

use std::time::UNIX_EPOCH;

use nebula_jmap_proto::session::ServerAlert;
use nebula_jmap_proto::types::UtcDate;
use nebula_store::AccountId;

use super::backend::Inner;

/// The account's current alerts, for its bridge capability.
pub(crate) fn server_alerts(inner: &Inner, account: AccountId) -> Vec<ServerAlert> {
    let Some(mail) = inner.mail.account(account) else {
        return Vec::new();
    };
    mail.alerts()
        .into_iter()
        .filter_map(|alert| {
            let seconds = alert
                .received_at
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|since| i64::try_from(since.as_secs()).ok())?;
            Some(ServerAlert {
                id: alert.id,
                text: alert.text.to_string(),
                received_at: UtcDate::from_unix_timestamp(seconds).ok()?,
            })
        })
        .collect()
}
