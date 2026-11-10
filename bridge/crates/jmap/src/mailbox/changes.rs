// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Mailbox/changes`. When every updated mailbox only had its counters
//! changed (new mail, read/unread, moves), `updatedProperties` lists the
//! four counters (RFC 8621 §2.2), so the client fetches just those;
//! otherwise it is `null`.

use nebula_jmap_proto::MethodResponse;
use nebula_jmap_proto::method::{ChangesArguments, MailboxChangesResponse};
use nebula_jmap_proto::object::MailboxProperty;
use nebula_store::{DataType, MailboxId};

use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    let (changes, only_counts) =
        crate::changes::changes_detail(ctx, DataType::Mailbox, args, |row| {
            MailboxId::from_raw(row)
                .ok_or(Bug("mailbox id"))
                .and_then(ids::mailbox_id)
        })
        .await?;
    let updated_properties = only_counts.then(|| {
        vec![
            MailboxProperty::TotalEmails,
            MailboxProperty::UnreadEmails,
            MailboxProperty::TotalThreads,
            MailboxProperty::UnreadThreads,
        ]
    });
    Ok(MethodResponse::MailboxChanges(MailboxChangesResponse {
        changes,
        updated_properties,
    }))
}
