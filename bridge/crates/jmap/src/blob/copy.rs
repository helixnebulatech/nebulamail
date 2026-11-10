// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Blob/copy` (RFC 8620 §6.3). Only the user's own account has blobs in
//! the bridge (uploads, messages and their parts; downloads of any other
//! account are 404), so:
//!
//! - `accountId` must be the user's own account (`accountNotFound`, or
//!   `accountNotSupportedByMethod` for a shared one, as every call);
//! - `fromAccountId` not an account of the user: `fromAccountNotFound`;
//! - from a shared account (calendars, contacts): every blob is
//!   `notFound` (there is none to copy);
//! - from the same account: a blob that exists is "copied" to itself.
//!   Blob ids of the bridge never change their bytes, so the same id
//!   already is the blob in the target account (the RFC doesn't forbid
//!   this case for blobs, unlike `Foo/copy`).

use std::collections::BTreeMap;

use nebula_jmap_proto::method::{BlobCopyArguments, BlobCopyResponse};
use nebula_jmap_proto::{MethodError, MethodErrorType, MethodResponse, SetError};

use crate::core::context::Ctx;
use crate::error::CallError;

pub(crate) async fn copy(
    ctx: &Ctx<'_>,
    args: BlobCopyArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let same = args.from_account_id == args.account_id;
    if !same
        && crate::sharing::accounts::owner_of(ctx, &args.from_account_id)
            .await?
            .is_none()
    {
        return Err(MethodError::new(MethodErrorType::FromAccountNotFound).into());
    }
    let mut copied = BTreeMap::new();
    let mut not_copied = BTreeMap::new();
    for blob in args.blob_ids {
        if copied.contains_key(&blob) || not_copied.contains_key(&blob) {
            continue;
        }
        let exists = same && {
            // Held while this blob is read: its memory is counted.
            let mut lease = ctx.mail()?.lease();
            match crate::email::build::blob_bytes(ctx, &blob, &mut lease).await {
                Ok(found) => found.is_some(),
                Err(error) => {
                    not_copied.insert(blob, error);
                    continue;
                }
            }
        };
        if exists {
            copied.insert(blob.clone(), blob);
        } else {
            not_copied.insert(blob, SetError::not_found());
        }
    }
    Ok(MethodResponse::BlobCopy(BlobCopyResponse {
        from_account_id: args.from_account_id,
        account_id: ctx.jmap_id(),
        copied,
        not_copied,
    }))
}
