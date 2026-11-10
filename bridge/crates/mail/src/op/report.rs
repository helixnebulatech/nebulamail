// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Reports arriving in the mail (DSN, MDN) update the `deliveryStatus` of
//! the account's submissions. The sync calls this for new messages whose
//! `Content-Type` is `multipart/report` with a known `report-type`, and
//! only while the account has recent submissions. Only the report's
//! machine-readable part and the first bytes of the returned message are
//! fetched; a report that can't be read is skipped (it never fails the
//! sync).

use nebula_imap::proto::command::PartPath;
use nebula_imap::proto::types::Uid;
use nebula_store::{DeliveryReport, EmailId, MailboxId, UnixTime};

use super::stream::Source;
use crate::MailError;
use crate::core::account::MailAccount;
use crate::model::layout::encoding_of;
use crate::model::report::{self, MAX_RETURNED_HEADER_BYTES, MAX_STATUS_BYTES, ReportType};
use crate::model::transfer::Decoder;

/// A new message that looks like a report.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub(crate) uid: Uid,
    pub(crate) received_at: UnixTime,
    pub(crate) kind: ReportType,
    /// The report's own `In-Reply-To`, if it has exactly one.
    pub(crate) in_reply_to: Option<String>,
}

impl Candidate {
    /// A candidate if the message's header says it is a report.
    pub(crate) fn of(message: &nebula_store::NewMessage) -> Option<Self> {
        let kind = report::report_type(&message.meta.headers)?;
        let in_reply_to = match message.meta.in_reply_to.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        };
        Some(Self {
            uid: message.uid,
            received_at: message.meta.received_at,
            kind,
            in_reply_to,
        })
    }
}

impl MailAccount {
    /// Reads and applies these new reports of a mailbox. Errors are only
    /// logged.
    pub(crate) async fn apply_reports(&self, mailbox: MailboxId, candidates: Vec<Candidate>) {
        for candidate in candidates {
            if let Err(error) = self.apply_report(mailbox, &candidate).await {
                tracing::debug!(error = %error, "delivery report skipped");
            }
        }
    }

    async fn apply_report(
        &self,
        mailbox: MailboxId,
        candidate: &Candidate,
    ) -> Result<(), MailError> {
        let now = UnixTime::now()?;
        if !self
            .store()
            .expects_reports(self.id, candidate.received_at, now)
            .await?
        {
            return Ok(());
        }
        let Some(email) = self
            .store()
            .emails_at(self.id, mailbox, vec![candidate.uid])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(());
        };
        let Some(report) = self.read_report(email, candidate).await? else {
            return Ok(());
        };
        let recorded = self.store().apply_report(self.id, report, now).await?;
        if !recorded.is_empty() {
            tracing::info!(
                account = self.id.get(),
                "delivery status updated from a report"
            );
        }
        self.emit(&recorded);
        Ok(())
    }

    async fn read_report(
        &self,
        email: EmailId,
        candidate: &Candidate,
    ) -> Result<Option<DeliveryReport>, MailError> {
        let location = self
            .email_locations(email)
            .await?
            .into_iter()
            .find(|l| l.uid == candidate.uid)
            .ok_or(MailError::NotFound("report"))?;
        let Some(structure) = self.structure_at(&location).await? else {
            return Ok(None);
        };
        let Some(parts) = report::report_parts(&structure.0, candidate.kind) else {
            return Ok(None);
        };
        let (status_path, status_part) = parts.status;
        if status_part.size > MAX_STATUS_BYTES {
            return Ok(None);
        }
        let Some(encoding) = encoding_of(&status_part.encoding) else {
            return Ok(None);
        };
        let path = PartPath::new(status_path).map_err(|_| MailError::Invalid("part path"))?;
        let encoded = self
            .fetch_piece(&location, &Source::Body(path), 0, MAX_STATUS_BYTES)
            .await?;
        let status = Decoder::decode_all(encoding, &encoded);
        let Some((envid, mut original, kind)) = report::parse(candidate.kind, &status) else {
            return Ok(None);
        };
        if original.is_none()
            && let Some(returned) = parts.returned
        {
            let path = PartPath::new(returned).map_err(|_| MailError::Invalid("part path"))?;
            let head = self
                .fetch_piece(&location, &Source::Body(path), 0, MAX_RETURNED_HEADER_BYTES)
                .await?;
            original = report::returned_message_id(&head);
        }
        let original = original.or_else(|| candidate.in_reply_to.clone());
        Ok(Some(DeliveryReport {
            email,
            received_at: candidate.received_at,
            envid,
            original_message_id: original,
            kind,
        }))
    }
}
