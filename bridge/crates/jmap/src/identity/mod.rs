// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Identity (RFC 8621 §6). Kept in the bridge's database: IMAP has no
//! identities. Every account gets one for its own address on first use;
//! there are no aliases, so that address is the only one allowed.

pub(crate) mod changes;
pub(crate) mod get;
pub(crate) mod set;

use nebula_store::{Identity, NewIdentity};

use crate::core::context::Ctx;
use crate::error::CallError;

/// The identity every account starts with.
fn default_identity(ctx: &Ctx<'_>) -> NewIdentity {
    NewIdentity {
        name: String::new(),
        email: ctx.account.email.clone(),
        reply_to: None,
        bcc: None,
        text_signature: String::new(),
        html_signature: String::new(),
        may_delete: false,
    }
}

/// The account's identities (creating the default one if needed).
pub(crate) async fn identities(ctx: &Ctx<'_>) -> Result<Vec<Identity>, CallError> {
    Ok(ctx
        .store()
        .identities_or_default(ctx.id(), default_identity(ctx))
        .await?)
}

/// Whether the account may send as `address` (its own address only).
pub(crate) fn may_use(ctx: &Ctx<'_>, address: &str) -> bool {
    address
        .trim()
        .eq_ignore_ascii_case(ctx.account.email.trim())
}
