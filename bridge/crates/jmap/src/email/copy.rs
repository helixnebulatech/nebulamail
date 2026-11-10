// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Email/copy` (RFC 8621 §4.7, RFC 8620 §5.4): copying emails between
//! accounts. The bridge gives each user one mail account, its own, and
//! `fromAccountId` "MUST be different from the accountId" (RFC 8620
//! §5.4), so no call can copy anything; the method answers with the
//! error the RFC names for each case:
//!
//! - `accountId` not the user's mail account: `accountNotFound` (or
//!   `accountNotSupportedByMethod` for a shared account, as every call);
//! - `fromAccountId` the same account: `invalidArguments`;
//! - `fromAccountId` a shared account of the user (calendars, contacts):
//!   `fromAccountNotSupportedByMethod` (it has no emails);
//! - any other `fromAccountId`: `fromAccountNotFound`.
//!
//! Moving or copying an email between mailboxes of the account is
//! `Email/set` with `mailboxIds`.

use nebula_jmap_proto::method::CopyArguments;
use nebula_jmap_proto::{AccountId, MethodError, MethodErrorType, MethodResponse};

use crate::core::context::Ctx;
use crate::error::CallError;

/// Why a copy into `to` from `from` can't run, given whether `from` is
/// one of the user's shared accounts.
pub(crate) fn copy_error(to: &AccountId, from: &AccountId, from_is_shared: bool) -> MethodError {
    if from == to {
        MethodError::invalid_arguments("fromAccountId must be different from accountId")
    } else if from_is_shared {
        MethodError::new(MethodErrorType::FromAccountNotSupportedByMethod)
            .with_description("this account has no emails")
    } else {
        MethodError::new(MethodErrorType::FromAccountNotFound)
    }
}

pub(crate) async fn copy(ctx: &Ctx<'_>, args: CopyArguments) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let shared = crate::sharing::accounts::owner_of(ctx, &args.from_account_id)
        .await?
        .is_some();
    Err(copy_error(&args.account_id, &args.from_account_id, shared).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors() {
        let a: AccountId = "a".parse().unwrap();
        let b: AccountId = "b".parse().unwrap();
        assert_eq!(
            copy_error(&a, &a, false).kind(),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            copy_error(&a, &b, true).kind(),
            MethodErrorType::FromAccountNotSupportedByMethod
        );
        assert_eq!(
            copy_error(&a, &b, false).kind(),
            MethodErrorType::FromAccountNotFound
        );
    }
}
