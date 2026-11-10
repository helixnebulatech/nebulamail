// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP objects that only exist in the bridge: uploaded
//! blobs, sending identities and sent submissions. Identity and submission
//! writes record their JMAP changes in the same transaction.

mod delivery;
mod held;
mod identity;
mod push;
mod submission;
mod upload;

#[cfg(test)]
mod tests;

pub use delivery::{
    Delivered, DeliveryReport, RecipientReport, RecipientStatus, ReportAction, ReportKind,
    report_address_key,
};
pub use held::{Cancel, ClaimedHeld, DueHeld, HeldLimits, NewHeld, Release, SetSignedOut};
pub use identity::{
    Identity, IdentityUpdate, MAX_IDENTITIES, MAX_IDENTITY_ADDRESSES, MAX_IDENTITY_NAME,
    MAX_SIGNATURE, NewIdentity,
};
pub use push::{
    CreatedPushSubscription, MAX_DEVICE_CLIENT_ID, NewPushSubscription, PushSubscriptionChange,
    PushSubscriptionRow, SystemSecret,
};
pub use submission::{NewSubmission, Submission, UndoState};
pub use upload::{MAX_UPLOADS_PER_ACCOUNT, Upload, UploadInfo, UploadOutcome};

use diesel::SqliteConnection;

use crate::{ChangeRetention, Store, StoreError, UnixTime};

impl Store {
    /// Deletes expired uploads and submissions (the submissions' `destroyed`
    /// changes are recorded), then trims the change log
    /// ([`ChangeRetention::default`]).
    ///
    /// # Errors
    /// On database errors.
    pub async fn purge_expired(&self, now: UnixTime) -> Result<(), StoreError> {
        self.write(move |conn: &mut SqliteConnection| {
            upload::purge(conn, now)?;
            submission::purge(conn, now)
        })
        .await?;
        self.trim_change_logs(ChangeRetention::default()).await
    }
}
