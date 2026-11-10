// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `deliveryStatus` of submissions from the reports that come back (RFC
//! 8621 §7): delivery status notifications (RFC 3464) and read receipts
//! (RFC 8098). The mail layer parses them; here they are matched to the
//! account's submissions and applied.
//!
//! Anyone can send a report, so matching is strict:
//!
//! - only the submissions of the account whose mailbox holds the report
//!   (a report can never touch another account);
//! - if the submission was sent with an ENVID, the report must quote it;
//!   else the report's original `Message-ID` must be the one sent;
//! - the report must have arrived after the message was sent;
//! - only recipients of the submission's envelope are updated (by address
//!   or by its `ORCPT`);
//! - `yes`/`no` are final: a later report can't turn a failure into a
//!   delivery or the other way round, and `displayed: yes` stays.
//!
//! A forged report can still lie about a message whose `Message-ID` the
//! forger knows (a recipient of it), before the real report arrives; it
//! only changes what the sender sees about that message.

use std::collections::BTreeMap;

use nebula_jmap_proto::object::Envelope;
use serde::{Deserialize, Serialize};

use crate::{EmailId, UnixTime};

/// `delivered` of RFC 8621 §7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Delivered {
    /// Still being tried.
    Queued,
    /// Delivered.
    Yes,
    /// Failed.
    No,
    /// Not known (relayed to a server without reports).
    Unknown,
}

impl Delivered {
    fn is_final(self) -> bool {
        matches!(self, Self::Yes | Self::No)
    }
}

/// The status of one recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipientStatus {
    /// The last SMTP reply known for this recipient.
    pub smtp_reply: String,
    /// Whether it was delivered.
    pub delivered: Delivered,
    /// Whether the recipient displayed it (an MDN said so).
    pub displayed: bool,
}

/// What a DSN says happened to one recipient (RFC 3464 `Action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReportAction {
    /// `failed`
    Failed,
    /// `delayed`
    Delayed,
    /// `delivered`
    Delivered,
    /// `relayed` or `expanded`: handed on, no more reports to expect.
    Relayed,
}

/// One recipient of a DSN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientReport {
    /// `Original-Recipient` and `Final-Recipient` addresses (`rfc822`
    /// type only), lower case.
    pub addresses: Vec<String>,
    /// What happened.
    pub action: ReportAction,
    /// The SMTP reply of `Diagnostic-Code: smtp; …`, cleaned.
    pub smtp_reply: Option<String>,
}

/// What kind of report.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReportKind {
    /// A DSN (`multipart/report; report-type=delivery-status`).
    Delivery(Vec<RecipientReport>),
    /// An MDN saying the message was displayed: its recipient addresses
    /// (lower case).
    Displayed(Vec<String>),
}

/// A parsed report, to match to the account's submissions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    /// The email holding the report.
    pub email: EmailId,
    /// When it arrived.
    pub received_at: UnixTime,
    /// `Original-Envelope-Id` (DSN only).
    pub envid: Option<String>,
    /// The `Message-ID` of the message it is about (without `<>`).
    pub original_message_id: Option<String>,
    /// What it says.
    pub kind: ReportKind,
}

/// How long before the send time a report may claim to have arrived
/// (clocks of the bridge and the mail server differ a little).
pub(crate) const CLOCK_SLACK_SECS: i64 = 600;

/// The status before any report: what the submission server said, then
/// unknown.
pub(crate) fn initial(envelope: &Envelope, smtp_reply: &str) -> BTreeMap<String, RecipientStatus> {
    envelope
        .rcpt_to
        .iter()
        .map(|rcpt| {
            (
                rcpt.email.as_str().to_owned(),
                RecipientStatus {
                    smtp_reply: smtp_reply.to_owned(),
                    delivered: Delivered::Unknown,
                    displayed: false,
                },
            )
        })
        .collect()
}

/// The form in which an address of a delivery report (DSN or MDN) is
/// compared with the envelope: lowercase, Unicode aware, so that `Ñandú@x`
/// in the envelope and `ñandú@x` in the report match. Both sides go
/// through this one function. Only for matching report recipients, never
/// for identity (the local part is case-sensitive in theory).
#[must_use]
pub fn report_address_key(address: &str) -> String {
    address.trim().to_lowercase()
}

/// The addresses a report may name for a recipient of the envelope: the
/// address, and the one in its `ORCPT` (`rfc822;addr`).
fn names(envelope: &Envelope) -> Vec<(String, Vec<String>)> {
    envelope
        .rcpt_to
        .iter()
        .map(|rcpt| {
            let mut list = vec![report_address_key(rcpt.email.as_str())];
            let orcpt = rcpt
                .parameters
                .as_ref()
                .and_then(|p| p.iter().find(|(name, _)| name.as_str() == "ORCPT"))
                .and_then(|(_, value)| value.as_ref())
                .and_then(|value| {
                    let (kind, address) = value.as_str().split_once(';')?;
                    kind.eq_ignore_ascii_case("rfc822")
                        .then(|| report_address_key(address))
                });
            list.extend(orcpt);
            (rcpt.email.as_str().to_owned(), list)
        })
        .collect()
}

/// Applies a report to a submission's status. Returns whether anything
/// changed.
pub(crate) fn apply(
    status: &mut BTreeMap<String, RecipientStatus>,
    envelope: &Envelope,
    kind: &ReportKind,
) -> bool {
    let mut changed = false;
    for (key, addresses) in names(envelope) {
        let Some(current) = status.get_mut(&key) else {
            continue;
        };
        let named = |list: &[String]| {
            list.iter()
                .any(|a| addresses.contains(&report_address_key(a)))
        };
        match kind {
            ReportKind::Delivery(recipients) => {
                let Some(report) = recipients.iter().find(|r| named(&r.addresses)) else {
                    continue;
                };
                if current.delivered.is_final() {
                    continue;
                }
                let delivered = match report.action {
                    ReportAction::Failed => Delivered::No,
                    ReportAction::Delayed => Delivered::Queued,
                    ReportAction::Delivered => Delivered::Yes,
                    ReportAction::Relayed => Delivered::Unknown,
                };
                let reply = report
                    .smtp_reply
                    .clone()
                    .unwrap_or_else(|| current.smtp_reply.clone());
                if current.delivered != delivered || current.smtp_reply != reply {
                    current.delivered = delivered;
                    current.smtp_reply = reply;
                    changed = true;
                }
            }
            ReportKind::Displayed(recipients) => {
                if named(recipients) && !current.displayed {
                    current.displayed = true;
                    changed = true;
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn envelope() -> Envelope {
        serde_json::from_value(json!({
            "mailFrom": {"email": "alice@example.com"},
            "rcptTo": [
                {"email": "Bob@Example.com"},
                {"email": "carol@example.com", "parameters": {"ORCPT": "rfc822;carol.old@example.com"}},
                {"email": "Ñandú@ejemplo.es"}
            ]
        }))
        .unwrap()
    }

    fn dsn(address: &str, action: ReportAction, reply: Option<&str>) -> ReportKind {
        ReportKind::Delivery(vec![RecipientReport {
            addresses: vec![address.to_owned()],
            action,
            smtp_reply: reply.map(str::to_owned),
        }])
    }

    #[test]
    fn reports_move_each_recipient() {
        let envelope = envelope();
        let mut status = initial(&envelope, "250 2.0.0 queued");
        assert!(apply(
            &mut status,
            &envelope,
            &dsn(
                "bob@example.com",
                ReportAction::Delayed,
                Some("451 4.4.1 later")
            )
        ));
        assert_eq!(status["Bob@Example.com"].delivered, Delivered::Queued);
        assert_eq!(status["Bob@Example.com"].smtp_reply, "451 4.4.1 later");
        assert_eq!(status["carol@example.com"].delivered, Delivered::Unknown);
        // By ORCPT.
        assert!(apply(
            &mut status,
            &envelope,
            &dsn(
                "carol.old@example.com",
                ReportAction::Failed,
                Some("550 5.1.1 no")
            )
        ));
        assert_eq!(status["carol@example.com"].delivered, Delivered::No);
        // Not ASCII: lowercase on both sides alike.
        assert!(apply(
            &mut status,
            &envelope,
            &dsn("ñandú@ejemplo.es", ReportAction::Delivered, None)
        ));
        assert_eq!(status["Ñandú@ejemplo.es"].delivered, Delivered::Yes);
        assert!(apply(
            &mut status,
            &envelope,
            &ReportKind::Displayed(vec!["ÑANDÚ@EJEMPLO.ES".to_owned()])
        ));
        assert!(status["Ñandú@ejemplo.es"].displayed);
        // Final: a later "delivered" doesn't undo the failure.
        assert!(!apply(
            &mut status,
            &envelope,
            &dsn("carol@example.com", ReportAction::Delivered, None)
        ));
        assert_eq!(status["carol@example.com"].delivered, Delivered::No);
        // Someone not in the envelope: nothing.
        assert!(!apply(
            &mut status,
            &envelope,
            &dsn("mallory@example.com", ReportAction::Delivered, None)
        ));
        assert!(apply(
            &mut status,
            &envelope,
            &ReportKind::Displayed(vec!["bob@example.com".to_owned()])
        ));
        assert!(status["Bob@Example.com"].displayed);
        assert!(!status["carol@example.com"].displayed);
        let json = serde_json::to_value(&status["Bob@Example.com"]).unwrap();
        assert_eq!(
            json,
            json!({"smtpReply": "451 4.4.1 later", "delivered": "queued", "displayed": true})
        );
    }
}
