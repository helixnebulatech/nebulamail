// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `EmailSubmission` (RFC 8621 §7): sending through the configured SMTP
//! server. Messages go out at once (`final`), or are held by the bridge
//! with RFC 4865 FUTURERELEASE (`pending` until `release` sends them, and
//! cancelable until then): undo send and scheduled send.

pub(crate) mod get;
pub(crate) mod hold;
pub(crate) mod query;
pub(crate) mod release;
pub(crate) mod send;
pub(crate) mod set;

use std::collections::BTreeMap;

use nebula_jmap_proto::object::{
    Delivered, DeliveryStatus, Displayed, EmailSubmission, EmailSubmissionProperty, UndoStatus,
};
use nebula_store::{Delivered as StoreDelivered, Submission, UndoState};

use crate::blob::BlobRef;
use crate::core::ids;
use crate::email::get::utc_date;
use crate::error::Bug;

/// The `smtpReply` of a submission whose reply was never stored: the row
/// is stored before sending (`set`), so the bridge stopped, or couldn't
/// store the reply, in between. It may or may not have gone out.
const UNKNOWN_REPLY: &str = "unknown: no reply from the server was recorded";

/// What the submission server said, as shown to the client: an empty
/// reply is never shown as a success.
fn shown_reply(stored: &str) -> String {
    if stored.trim().is_empty() {
        UNKNOWN_REPLY.to_owned()
    } else {
        stored.to_owned()
    }
}

/// The JMAP `undoStatus`: one being sent is still `pending` (it hasn't
/// gone out), though too late to cancel.
pub(crate) fn undo_status(undo: UndoState) -> UndoStatus {
    match undo {
        UndoState::Pending | UndoState::Sending => UndoStatus::Pending,
        UndoState::Canceled => UndoStatus::Canceled,
        _ => UndoStatus::Final,
    }
}

/// The JMAP form of a stored submission. `signed_out`: the bridge sends
/// held messages after sign-out, so the vendor property
/// `sendWhenSignedOut` is shown.
pub(crate) fn to_jmap(
    submission: &Submission,
    properties: Option<&[EmailSubmissionProperty]>,
    signed_out: bool,
) -> Result<EmailSubmission, Bug> {
    let wants = |p| properties.is_none_or(|list| list.contains(&p));
    let sent = &submission.sent;
    let mut out = EmailSubmission {
        id: Some(ids::submission_id(submission.id)?),
        ..EmailSubmission::default()
    };
    if wants(EmailSubmissionProperty::IdentityId) {
        out.identity_id = Some(ids::identity_id(sent.identity)?);
    }
    if wants(EmailSubmissionProperty::EmailId) {
        out.email_id = Some(ids::email_id(sent.email)?);
    }
    if wants(EmailSubmissionProperty::ThreadId) {
        out.thread_id = Some(ids::thread_id(sent.thread)?);
    }
    if wants(EmailSubmissionProperty::Envelope) {
        out.envelope = Some(Some(sent.envelope.clone()));
    }
    if wants(EmailSubmissionProperty::SendAt) {
        out.send_at = Some(utc_date(sent.send_at.secs())?);
    }
    if wants(EmailSubmissionProperty::UndoStatus) {
        out.undo_status = Some(undo_status(submission.undo));
    }
    if wants(EmailSubmissionProperty::DeliveryStatus) {
        // What the submission server said, then what the reports that
        // came back say (`nebula_mail` reads them, the store applies them).
        let status: BTreeMap<String, DeliveryStatus> = submission
            .status
            .iter()
            .map(|(rcpt, status)| {
                (
                    rcpt.clone(),
                    DeliveryStatus {
                        smtp_reply: shown_reply(&status.smtp_reply),
                        delivered: match status.delivered {
                            StoreDelivered::Queued => Delivered::Queued,
                            StoreDelivered::Yes => Delivered::Yes,
                            StoreDelivered::No => Delivered::No,
                            _ => Delivered::Unknown,
                        },
                        displayed: if status.displayed {
                            Displayed::Yes
                        } else {
                            Displayed::Unknown
                        },
                    },
                )
            })
            .collect();
        out.delivery_status = Some(Some(status));
    }
    // The reports are the emails that hold them (`bm…` blobs).
    let blobs = |list: &[nebula_store::EmailId]| {
        list.iter()
            .map(|email| BlobRef::Message(*email).to_blob_id())
            .collect::<Result<Vec<_>, _>>()
    };
    if wants(EmailSubmissionProperty::DsnBlobIds) {
        out.dsn_blob_ids = Some(blobs(&submission.dsn_emails)?);
    }
    if wants(EmailSubmissionProperty::MdnBlobIds) {
        out.mdn_blob_ids = Some(blobs(&submission.mdn_emails)?);
    }
    if signed_out && wants(EmailSubmissionProperty::SendWhenSignedOut) {
        out.send_when_signed_out = Some(submission.send_when_signed_out);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_reply_is_unknown() {
        assert_eq!(shown_reply(""), UNKNOWN_REPLY);
        assert_eq!(shown_reply(" "), UNKNOWN_REPLY);
        assert_eq!(shown_reply("250 2.0.0 OK"), "250 2.0.0 OK");
    }
}
