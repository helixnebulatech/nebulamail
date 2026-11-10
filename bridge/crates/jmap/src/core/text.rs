// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The text of a stored resource (event or card) from the local copy,
//! read again from the server when that copy doesn't open.

use nebula_groupware::{GroupwareAccount, GroupwareError, ResourceText};
use nebula_store::StoredResource;

/// The text of `resource`: the local copy, or the server's if the copy is
/// unreadable (then stored again). `None`: no such resource any more.
///
/// # Errors
/// What [`GroupwareAccount::resource_text`] and
/// [`GroupwareAccount::refetch`] give.
pub(crate) async fn readable_text(
    dav: &GroupwareAccount,
    resource: &StoredResource,
) -> Result<Option<ResourceText>, GroupwareError> {
    match dav.resource_text(resource.id).await {
        Err(GroupwareError::Unreadable) => {
            tracing::info!("groupware: local copy unreadable; reading it again");
            dav.refetch(resource).await
        }
        other => other,
    }
}
