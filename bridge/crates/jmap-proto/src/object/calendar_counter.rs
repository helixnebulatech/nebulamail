// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Proposals of another time, a vendor extension of the Nebula Mail
//! bridge (capability `https://helixnebula.tech/jmap/bridge`).
//! JMAP for Calendars has no method for iTIP
//! `COUNTER` / `DECLINECOUNTER` (RFC 5546 §3.2.7–3.2.8), so the bridge
//! offers two, and builds and mails the messages itself:
//!
//! - `CalendarEvent/counter`: the user, invited to an event, proposes
//!   another time to its organizer. The event is a stored one
//!   (`eventId`, maybe an occurrence id) or the `text/calendar` part of
//!   the invitation email (`blobId`), exactly one of them.
//! - `CalendarEvent/declineCounter`: the user, organizer of `eventId`,
//!   keeps the time; `blobId` is the `COUNTER` part of the guest's email.
//!
//! Both answer `{"accountId": …}` once the email went out.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::MethodError;
use crate::method::parse::from_arguments;
use crate::types::{AccountId, BlobId, CalendarEventId, UtcDate};

/// Longest `comment` accepted, in characters (it is cut to 2000).
const MAX_COMMENT_CHARS: usize = 10_000;

fn check_comment(comment: Option<&String>) -> Result<(), MethodError> {
    if comment.is_some_and(|c| c.chars().count() > MAX_COMMENT_CHARS) {
        return Err(MethodError::invalid_arguments("comment is too long"));
    }
    Ok(())
}

/// Which event a proposal is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CounterSubject {
    /// A stored event or one of its occurrences.
    Event(CalendarEventId),
    /// The iCalendar part of an invitation email.
    Invitation(BlobId),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawCounter {
    account_id: AccountId,
    #[serde(default)]
    event_id: Option<CalendarEventId>,
    #[serde(default)]
    blob_id: Option<BlobId>,
    utc_start: UtcDate,
    utc_end: UtcDate,
    #[serde(default)]
    comment: Option<String>,
}

/// `CalendarEvent/counter` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEventCounterArguments {
    /// The account.
    pub account_id: AccountId,
    /// The event.
    pub subject: CounterSubject,
    /// The proposed start.
    pub utc_start: UtcDate,
    /// The proposed end (not before the start).
    pub utc_end: UtcDate,
    /// A note for the organizer.
    pub comment: Option<String>,
}

impl CalendarEventCounterArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`: unknown or missing members, both or neither of
    /// `eventId` and `blobId`, an end before the start, a huge comment.
    pub fn parse(arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let raw: RawCounter = from_arguments(arguments)?;
        let subject = match (raw.event_id, raw.blob_id) {
            (Some(id), None) => CounterSubject::Event(id),
            (None, Some(blob)) => CounterSubject::Invitation(blob),
            _ => {
                return Err(MethodError::invalid_arguments(
                    "exactly one of eventId and blobId",
                ));
            }
        };
        if raw.utc_end.unix_timestamp() < raw.utc_start.unix_timestamp() {
            return Err(MethodError::invalid_arguments("utcEnd is before utcStart"));
        }
        check_comment(raw.comment.as_ref())?;
        Ok(Self {
            account_id: raw.account_id,
            subject,
            utc_start: raw.utc_start,
            utc_end: raw.utc_end,
            comment: raw.comment,
        })
    }
}

/// `CalendarEvent/declineCounter` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarEventDeclineCounterArguments {
    /// The account.
    pub account_id: AccountId,
    /// The user's event (the series, or one occurrence of it).
    pub event_id: CalendarEventId,
    /// The `COUNTER` part of the guest's email.
    pub blob_id: BlobId,
    /// A note for the guest.
    #[serde(default)]
    pub comment: Option<String>,
}

impl CalendarEventDeclineCounterArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`.
    pub fn parse(arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        check_comment(parsed.comment.as_ref())?;
        Ok(parsed)
    }
}

/// The answer of both methods: the email went out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarItipSentResponse {
    /// The account.
    pub account_id: AccountId,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// The requests of `apps/web/src/calendar/invitations.ts`.
    #[test]
    fn web_requests() {
        let by_event = CalendarEventCounterArguments::parse(map(json!({
            "accountId": "c", "eventId": "v3_1792483200",
            "utcStart": "2026-10-20T08:00:00Z", "utcEnd": "2026-10-20T09:00:00Z",
            "comment": "After lunch?"
        })))
        .unwrap();
        assert!(matches!(by_event.subject, CounterSubject::Event(_)));
        assert_eq!(by_event.comment.as_deref(), Some("After lunch?"));
        let by_blob = CalendarEventCounterArguments::parse(map(json!({
            "accountId": "c", "blobId": "b1",
            "utcStart": "2026-10-20T08:00:00Z", "utcEnd": "2026-10-20T08:00:00Z"
        })))
        .unwrap();
        assert!(matches!(by_blob.subject, CounterSubject::Invitation(_)));
        let decline = CalendarEventDeclineCounterArguments::parse(map(json!({
            "accountId": "c", "eventId": "v3", "blobId": "b2"
        })))
        .unwrap();
        assert!(decline.comment.is_none());
        let sent = serde_json::to_value(CalendarItipSentResponse {
            account_id: "c".parse().unwrap(),
        })
        .unwrap();
        assert_eq!(sent, json!({"accountId": "c"}));
    }

    #[test]
    fn refused() {
        for bad in [
            json!({"accountId": "c", "eventId": "v3", "blobId": "b",
                   "utcStart": "2026-10-20T08:00:00Z", "utcEnd": "2026-10-20T09:00:00Z"}),
            json!({"accountId": "c",
                   "utcStart": "2026-10-20T08:00:00Z", "utcEnd": "2026-10-20T09:00:00Z"}),
            json!({"accountId": "c", "eventId": "v3",
                   "utcStart": "2026-10-20T09:00:00Z", "utcEnd": "2026-10-20T08:00:00Z"}),
            json!({"accountId": "c", "eventId": "v3",
                   "utcStart": "2026-10-20T08:00:00Z", "utcEnd": "2026-10-20T09:00:00Z",
                   "comment": "x".repeat(MAX_COMMENT_CHARS + 1)}),
            json!({"accountId": "c", "eventId": "v3", "extra": 1,
                   "utcStart": "2026-10-20T08:00:00Z", "utcEnd": "2026-10-20T09:00:00Z"}),
        ] {
            assert!(
                CalendarEventCounterArguments::parse(map(bad.clone())).is_err(),
                "{bad}"
            );
        }
        assert!(
            CalendarEventDeclineCounterArguments::parse(map(
                json!({"accountId": "c", "eventId": "v3"})
            ))
            .is_err()
        );
    }
}
