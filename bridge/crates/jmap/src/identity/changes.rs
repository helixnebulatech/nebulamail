// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Identity/changes`.

use nebula_jmap_proto::MethodResponse;
use nebula_jmap_proto::method::ChangesArguments;
use nebula_store::{DataType, IdentityId};

use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    let changes = crate::changes::changes(ctx, DataType::Identity, args, |row| {
        IdentityId::from_raw(row)
            .ok_or(Bug("identity id"))
            .and_then(ids::identity_id)
    })
    .await?;
    Ok(MethodResponse::IdentityChanges(changes))
}
