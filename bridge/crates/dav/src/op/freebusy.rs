// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Free-busy through the scheduling outbox (RFC 6638 §5): a POST of a
//! `VFREEBUSY` request to the user's `schedule-outbox-URL`; the server
//! answers each attendee's busy times in a `schedule-response`.
//!
//! The outbox comes from discovery (allowed origin, checked again here);
//! the answer is parsed by `nebula-dav-proto`'s strict XML parser with a
//! small size limit, and only the attendees that were asked for are kept.
//! The `VFREEBUSY` texts are returned as they are: reading them is the
//! caller's (calcard, inside its panic guard).

use http::{Method, StatusCode};
use nebula_dav_proto::{
    CalendarAddress, FreeBusyRequest, Href, RequestStatus, XmlLimits, parse_schedule_response,
};

use crate::core::client::Call;
use crate::{DavClient, DavError};

/// Largest `schedule-response` read: a few busy periods per attendee are
/// some hundred bytes each.
pub const MAX_FREEBUSY_RESPONSE: usize = 1024 * 1024;

/// What the server answered for one attendee.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FreeBusyAnswer {
    /// The attendee, as asked for.
    pub attendee: CalendarAddress,
    /// Its `request-status` (`None` if the server gave none, or did not
    /// mention the attendee at all).
    pub status: Option<RequestStatus>,
    /// The `VFREEBUSY` reply (CRLF line ends), if the server sent one.
    pub calendar_data: Option<String>,
}

impl FreeBusyAnswer {
    /// The server answered with busy times (`2.x` and data).
    #[must_use]
    pub fn has_data(&self) -> bool {
        self.status.as_ref().is_some_and(RequestStatus::is_success) && self.calendar_data.is_some()
    }
}

impl DavClient {
    /// Asks the outbox for the busy times of `request.attendees`. One
    /// answer per attendee, in the order asked (an attendee the server did
    /// not mention has no status and no data).
    ///
    /// # Errors
    ///
    /// [`DavError::Encode`] for a request that cannot be written (no
    /// attendees, too many, bad times); [`DavError::OriginNotAllowed`]
    /// for an outbox elsewhere; [`DavError::Redirect`] for a 301/302/303;
    /// [`DavError::ContentType`] or [`DavError::Xml`] for an answer that
    /// is not a `schedule-response`; or the request's.
    pub async fn free_busy(
        &self,
        outbox: &Href,
        request: &FreeBusyRequest,
    ) -> Result<Vec<FreeBusyAnswer>, DavError> {
        self.check_origin(outbox)?;
        let body = request.to_icalendar()?;
        let limits = self.server().limits;
        let call = Call::new(Method::POST, outbox, MAX_FREEBUSY_RESPONSE)
            .body(body, "text/calendar; charset=utf-8");
        let reply = self.send(&call).await?.success()?;
        if reply.status != StatusCode::OK {
            return Err(DavError::Protocol("expected 200 OK"));
        }
        reply.expect_xml()?;
        let xml = XmlLimits {
            max_bytes: MAX_FREEBUSY_RESPONSE,
            max_nodes: 100_000,
            max_value_bytes: limits.max_icalendar,
            ..XmlLimits::default()
        };
        let responses = parse_schedule_response(&reply.body, &xml)?;
        let answers = request
            .attendees
            .iter()
            .map(|attendee| {
                let found = responses.iter().find(|response| {
                    response
                        .recipient
                        .as_deref()
                        .is_some_and(|recipient| attendee.matches(recipient))
                });
                FreeBusyAnswer {
                    attendee: attendee.clone(),
                    status: found.and_then(|response| response.status.clone()),
                    calendar_data: found.and_then(|response| response.calendar_data.clone()),
                }
            })
            .collect();
        Ok(answers)
    }
}
