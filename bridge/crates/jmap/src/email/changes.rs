// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/changes`.

use nebula_jmap_proto::MethodResponse;
use nebula_jmap_proto::method::ChangesArguments;
use nebula_store::{DataType, EmailId};

use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    let changes = crate::changes::changes(ctx, DataType::Email, args, |row| {
        EmailId::from_raw(row)
            .ok_or(Bug("email id"))
            .and_then(ids::email_id)
    })
    .await?;
    Ok(MethodResponse::EmailChanges(changes))
}
