// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The answer to a POST on a scheduling outbox (RFC 6638 §§6.1.4, 10):
//! one `C:response` per recipient, with its `C:request-status` and, for
//! a free-busy request, the `VFREEBUSY` reply in `C:calendar-data`.
//!
//! The recipient is a calendar user address (`mailto:…`), not a location:
//! it is kept as text, never resolved or followed.

use crate::xml::{child, crlf, element_children, is, parse_document, text_of};
use crate::{EncodeError, Namespace, ParseError, XmlLimits};

/// Most recipients read from one answer (we never ask for more).
pub const MAX_SCHEDULE_RESPONSES: usize = 64;
/// Longest recipient address kept.
const MAX_RECIPIENT: usize = 1024;

/// A `REQUEST-STATUS` code (RFC 5545 §3.8.8.3): `2.0`, `3.7`… The
/// description after `;` is server text and is not kept.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestStatus {
    class: u8,
    code: String,
}

impl RequestStatus {
    /// Parses `statcode [";" description]`, where `statcode` is
    /// `1*DIGIT 1*2("." 1*DIGIT)` and the class (first number) is 1 to 5.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let code = text.split(';').next()?.trim();
        if code.len() > 16 {
            return None;
        }
        let parts: Vec<&str> = code.split('.').collect();
        let numbers_ok = parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
        if !(2..=3).contains(&parts.len()) || !numbers_ok {
            return None;
        }
        let class: u8 = parts.first()?.parse().ok()?;
        (1..=5).contains(&class).then(|| Self {
            class,
            code: code.to_owned(),
        })
    }

    /// The class: 1 pending, 2 success, 3 client error, 4 scheduling
    /// error, 5 failure.
    #[must_use]
    pub const fn class(&self) -> u8 {
        self.class
    }

    /// Class 2: the request worked for this recipient.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.class == 2
    }

    /// The code (`"2.0"`).
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

/// One recipient of a scheduling answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScheduleResponse {
    /// The recipient's calendar user address, as the server wrote it
    /// (`None` if it was missing, too long or had spaces or controls).
    pub recipient: Option<String>,
    /// Its `request-status` (`None` if missing or malformed).
    pub status: Option<RequestStatus>,
    /// The `calendar-data` (a `VFREEBUSY` reply), with CRLF line ends.
    pub calendar_data: Option<String>,
}

/// Most attendees in one free-busy request (as many as the invitations
/// of one event).
pub const MAX_FREEBUSY_ATTENDEES: usize = 20;
/// Latest time a request may name: 9999-12-31T23:59:59Z.
const MAX_TIME: i64 = 253_402_300_799;

/// A `mailto:` calendar user address for a scheduling request. Parsed
/// strictly, so it can go into an iCalendar line as it is: an `@`, no
/// spaces or controls, none of `"`, `,`, `;`, `:`, `<`, `>`, `\` in the
/// address, at most [`Self::MAX_LEN`] bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CalendarAddress(String);

impl CalendarAddress {
    /// Longest address (without `mailto:`).
    pub const MAX_LEN: usize = 320;

    /// `mailto:user@host` (scheme in any case) or a bare `user@host`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let address = match text.split_once(':') {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("mailto") => rest,
            Some(_) => return None,
            None => text,
        };
        let valid = !address.is_empty()
            && address.len() <= Self::MAX_LEN
            && address.contains('@')
            && !address.starts_with('@')
            && !address.ends_with('@')
            && !address.chars().any(|c| {
                c.is_control()
                    || c.is_whitespace()
                    || matches!(c, '"' | ',' | ';' | ':' | '<' | '>' | '\\')
            });
        valid.then(|| Self(format!("mailto:{address}")))
    }

    /// The URI (`mailto:…`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The address, without `mailto:`.
    #[must_use]
    pub fn address(&self) -> &str {
        self.0.get("mailto:".len()..).unwrap_or_default()
    }

    /// The same address as `uri` (a recipient the server wrote): scheme
    /// and address compared without case, as servers do.
    #[must_use]
    pub fn matches(&self, uri: &str) -> bool {
        Self::parse(uri).is_some_and(|other| other.0.eq_ignore_ascii_case(&self.0))
    }
}

/// A free-busy request (RFC 6638 §5, RFC 5546 §3.3.2): `METHOD:REQUEST`
/// with one `VFREEBUSY` asking for the busy times of `attendees` between
/// `start` and `end` (seconds since the epoch, UTC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeBusyRequest {
    /// The `UID` (letters, digits, `-`, `_`, `.`, `@`; 1 to 255).
    pub uid: String,
    /// `DTSTAMP`.
    pub stamp: i64,
    /// `DTSTART`.
    pub start: i64,
    /// `DTEND` (after `start`).
    pub end: i64,
    /// `ORGANIZER`: one of the user's own addresses (servers check).
    pub organizer: CalendarAddress,
    /// `ATTENDEE`s: 1 to [`MAX_FREEBUSY_ATTENDEES`].
    pub attendees: Vec<CalendarAddress>,
}

impl FreeBusyRequest {
    /// The iCalendar text, with CRLF line ends and lines folded at 75
    /// octets.
    ///
    /// ```
    /// use nebula_dav_proto::{CalendarAddress, FreeBusyRequest};
    ///
    /// let request = FreeBusyRequest {
    ///     uid: "fb-1".to_owned(),
    ///     stamp: 0,
    ///     start: 1_792_454_400,
    ///     end: 1_793_664_000,
    ///     organizer: CalendarAddress::parse("alice@example.com").unwrap(),
    ///     attendees: vec![CalendarAddress::parse("mailto:bob@example.com").unwrap()],
    /// };
    /// let text = request.to_icalendar().unwrap();
    /// assert!(text.contains("\r\nDTSTART:20261020T000000Z\r\nDTEND:20261103T000000Z\r\n"));
    /// assert!(text.contains("\r\nATTENDEE:mailto:bob@example.com\r\n"));
    /// ```
    ///
    /// # Errors
    ///
    /// [`EncodeError::Range`] for a bad `uid`, no attendees or too many,
    /// or times out of order or outside 1970–9999.
    pub fn to_icalendar(&self) -> Result<String, EncodeError> {
        let uid_ok = !self.uid.is_empty()
            && self.uid.len() <= 255
            && self
                .uid
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'));
        if !uid_ok
            || self.attendees.is_empty()
            || self.attendees.len() > MAX_FREEBUSY_ATTENDEES
            || self.start >= self.end
        {
            return Err(EncodeError::Range);
        }
        let stamp = utc_stamp(self.stamp)?;
        let start = utc_stamp(self.start)?;
        let end = utc_stamp(self.end)?;
        let mut lines = vec![
            "BEGIN:VCALENDAR".to_owned(),
            "VERSION:2.0".to_owned(),
            "PRODID:-//Nebula Mail//bridge//EN".to_owned(),
            "METHOD:REQUEST".to_owned(),
            "BEGIN:VFREEBUSY".to_owned(),
            format!("UID:{}", self.uid),
            format!("DTSTAMP:{stamp}"),
            format!("DTSTART:{start}"),
            format!("DTEND:{end}"),
            format!("ORGANIZER:{}", self.organizer.as_str()),
        ];
        for attendee in &self.attendees {
            lines.push(format!("ATTENDEE:{}", attendee.as_str()));
        }
        lines.push("END:VFREEBUSY".to_owned());
        lines.push("END:VCALENDAR".to_owned());
        let mut out = String::new();
        for line in &lines {
            fold_into(&mut out, line);
        }
        Ok(out)
    }
}

/// Appends `line` folded at 75 octets (RFC 5545 §3.1), never inside a
/// character, and CRLF.
fn fold_into(out: &mut String, line: &str) {
    let mut width = 0_usize;
    for c in line.chars() {
        let size = c.len_utf8();
        if width + size > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += size;
    }
    out.push_str("\r\n");
}

/// `YYYYMMDDTHHMMSSZ` for seconds since the epoch, 1970 to 9999.
fn utc_stamp(seconds: i64) -> Result<String, EncodeError> {
    let seconds = u64::try_from(seconds)
        .ok()
        .filter(|s| *s <= MAX_TIME.unsigned_abs())
        .ok_or(EncodeError::Range)?;
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    // Howard Hinnant's civil_from_days, for days >= 0 (no negative eras).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    Ok(format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    ))
}

/// Parses a `C:schedule-response` body.
///
/// # Errors
///
/// [`ParseError`] for the XML limits, malformed XML, a DTD, a root that
/// is not `C:schedule-response`, a `calendar-data` longer than
/// `limits.max_value_bytes`, or more than [`MAX_SCHEDULE_RESPONSES`]
/// recipients.
pub fn parse_schedule_response(
    body: &[u8],
    limits: &XmlLimits,
) -> Result<Vec<ScheduleResponse>, ParseError> {
    let document = parse_document(body, limits)?;
    let root = document.root_element();
    if !is(root, Namespace::CalDav, "schedule-response") {
        return Err(ParseError::Invalid("root is not C:schedule-response"));
    }
    let mut responses = Vec::new();
    for node in element_children(root).filter(|n| is(*n, Namespace::CalDav, "response")) {
        if responses.len() >= MAX_SCHEDULE_RESPONSES {
            return Err(ParseError::Invalid("too many recipients"));
        }
        let recipient = child(node, Namespace::CalDav, "recipient")
            .and_then(|r| child(r, Namespace::Dav, "href"))
            .and_then(|href| text_of(href, MAX_RECIPIENT).ok().flatten())
            .map(|text| text.trim().to_owned())
            .filter(|text| {
                !text.is_empty() && !text.chars().any(|c| c.is_control() || c.is_whitespace())
            });
        let status = child(node, Namespace::CalDav, "request-status")
            .and_then(|s| text_of(s, 1024).ok().flatten())
            .as_deref()
            .and_then(RequestStatus::parse);
        let calendar_data = match child(node, Namespace::CalDav, "calendar-data") {
            Some(data) => text_of(data, limits.max_value_bytes)?.map(|text| crlf(&text)),
            None => None,
        };
        responses.push(ScheduleResponse {
            recipient,
            status,
            calendar_data,
        });
    }
    Ok(responses)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Stalwart 0.16.27 answered to a free-busy request for alice
    /// (busy), someone outside (3.7) and bob (nothing shared).
    const STALWART: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><A:schedule-response xmlns:D=\"DAV:\" xmlns:A=\"urn:ietf:params:xml:ns:caldav\"><A:response><A:recipient><D:href>mailto:alice@nebulamail.test</D:href></A:recipient><A:request-status>2.0;Success</A:request-status><A:calendar-data><![CDATA[BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//Stalwart Labs LLC//Stalwart Server//EN\nMETHOD:REPLY\nBEGIN:VFREEBUSY\nDTSTART:20261020T000000Z\nDTEND:20261103T000000Z\nDTSTAMP:20261027T192349Z\nFREEBUSY;FBTYPE=BUSY:20261020T133000Z/20261020T143000Z,20261021T150000Z/202\n 61021T160000Z\nORGANIZER:mailto:alice@nebulamail.test\nATTENDEE:mailto:alice@nebulamail.test\nUID:fb-test-1\nEND:VFREEBUSY\nEND:VCALENDAR\n]]></A:calendar-data></A:response><A:response><A:recipient><D:href>mailto:nobody@example.com</D:href></A:recipient><A:request-status>3.7;Invalid calendar user or insufficient permissions</A:request-status></A:response><A:response><A:recipient><D:href>mailto:bob@nebulamail.test</D:href></A:recipient><A:request-status>2.0;Success</A:request-status><A:calendar-data><![CDATA[BEGIN:VCALENDAR\nMETHOD:REPLY\nBEGIN:VFREEBUSY\nEND:VFREEBUSY\nEND:VCALENDAR\n]]></A:calendar-data></A:response></A:schedule-response>";

    #[test]
    fn stalwart_answer() {
        let responses =
            parse_schedule_response(STALWART.as_bytes(), &XmlLimits::default()).unwrap();
        assert_eq!(responses.len(), 3);
        let alice = &responses[0];
        assert_eq!(
            alice.recipient.as_deref(),
            Some("mailto:alice@nebulamail.test")
        );
        assert!(alice.status.as_ref().unwrap().is_success());
        let data = alice.calendar_data.as_deref().unwrap();
        assert!(
            data.starts_with("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n"),
            "{data}"
        );
        assert!(data.contains("/202\r\n 61021T160000Z\r\n"), "{data}");
        let outside = &responses[1];
        assert_eq!(outside.status.as_ref().unwrap().code(), "3.7");
        assert!(!outside.status.as_ref().unwrap().is_success());
        assert_eq!(outside.calendar_data, None);
        assert!(responses[2].calendar_data.is_some());
    }

    #[test]
    fn utc_stamps() {
        for (seconds, text) in [
            (0, "19700101T000000Z"),
            (951_782_400, "20000229T000000Z"),
            (1_792_454_400, "20261020T000000Z"),
            (1_793_059_199, "20261026T235959Z"),
            (253_402_300_799, "99991231T235959Z"),
        ] {
            assert_eq!(utc_stamp(seconds).unwrap(), text);
        }
        assert_eq!(utc_stamp(-1), Err(EncodeError::Range));
        assert_eq!(utc_stamp(253_402_300_800), Err(EncodeError::Range));
    }

    fn request(attendees: &[&str]) -> FreeBusyRequest {
        FreeBusyRequest {
            uid: "nebula-fb-1".to_owned(),
            stamp: 1_793_059_199,
            start: 1_792_454_400,
            end: 1_793_664_000,
            organizer: CalendarAddress::parse("mailto:alice@nebulamail.test").unwrap(),
            attendees: attendees
                .iter()
                .map(|a| CalendarAddress::parse(a).unwrap())
                .collect(),
        }
    }

    #[test]
    fn free_busy_request() {
        let text = request(&["bob@nebulamail.test", "MAILTO:carol@nebulamail.test"])
            .to_icalendar()
            .unwrap();
        assert_eq!(
            text,
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Nebula Mail//bridge//EN\r\n\
             METHOD:REQUEST\r\nBEGIN:VFREEBUSY\r\nUID:nebula-fb-1\r\n\
             DTSTAMP:20261026T235959Z\r\nDTSTART:20261020T000000Z\r\n\
             DTEND:20261103T000000Z\r\nORGANIZER:mailto:alice@nebulamail.test\r\n\
             ATTENDEE:mailto:bob@nebulamail.test\r\nATTENDEE:mailto:carol@nebulamail.test\r\n\
             END:VFREEBUSY\r\nEND:VCALENDAR\r\n"
        );
        // A long address is folded at 75 octets, never inside a character.
        let long = format!("{}ñ@example.com", "a".repeat(70));
        let text = request(&[&long]).to_icalendar().unwrap();
        for line in text.split("\r\n") {
            assert!(line.len() <= 75, "{line}");
        }
        assert!(
            text.replace("\r\n ", "")
                .contains(&format!("ATTENDEE:mailto:{long}\r\n"))
        );
    }

    #[test]
    fn free_busy_request_limits() {
        let mut bad = request(&["bob@nebulamail.test"]);
        bad.end = bad.start;
        assert_eq!(bad.to_icalendar(), Err(EncodeError::Range));
        assert_eq!(request(&[]).to_icalendar(), Err(EncodeError::Range));
        let many: Vec<String> = (0..=MAX_FREEBUSY_ATTENDEES)
            .map(|n| format!("u{n}@example.com"))
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(request(&many).to_icalendar(), Err(EncodeError::Range));
        let mut uid = request(&["bob@nebulamail.test"]);
        uid.uid = "a\r\nATTENDEE:mailto:x@y".to_owned();
        assert_eq!(uid.to_icalendar(), Err(EncodeError::Range));
    }

    #[test]
    fn calendar_addresses() {
        let bob = CalendarAddress::parse(" mailto:Bob@Example.com ").unwrap();
        assert_eq!(bob.as_str(), "mailto:Bob@Example.com");
        assert_eq!(bob.address(), "Bob@Example.com");
        assert!(bob.matches("MAILTO:bob@example.com"));
        assert!(!bob.matches("mailto:bobby@example.com"));
        assert!(!bob.matches("/principals/bob/"));
        for bad in [
            "",
            "mailto:",
            "bob",
            "@example.com",
            "bob@",
            "http://example.com/bob@x",
            "mailto:bob@example.com\r\nATTENDEE:mailto:eve@x",
            "mailto:bob smith@example.com",
            "mailto:bob@example.com;x=y",
            "mailto:\"bob\"@example.com",
            "/alice/",
        ] {
            assert_eq!(CalendarAddress::parse(bad), None, "{bad:?}");
        }
        let long = format!("{}@x", "a".repeat(CalendarAddress::MAX_LEN));
        assert_eq!(CalendarAddress::parse(&long), None);
    }

    #[test]
    fn request_statuses() {
        for (text, class) in [
            ("2.0;Success", 2),
            ("2.0", 2),
            (" 3.7 ;Invalid", 3),
            ("1.2;Delivered", 1),
            ("5.3.1;Nope", 5),
        ] {
            assert_eq!(RequestStatus::parse(text).unwrap().class(), class, "{text}");
        }
        for bad in [
            "",
            "2",
            "6.0",
            "0.1",
            "2.x",
            "2..0",
            "2.0.0.0",
            "a.b",
            "22222222222222.0",
        ] {
            assert_eq!(RequestStatus::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hostile_answers() {
        let limits = XmlLimits::default();
        // Wrong root, DTD, malformed.
        assert!(parse_schedule_response(br#"<multistatus xmlns="DAV:"/>"#, &limits).is_err());
        assert_eq!(
            parse_schedule_response(
                br#"<!DOCTYPE a [<!ENTITY x SYSTEM "file:///etc/passwd">]><c:schedule-response xmlns:c="urn:ietf:params:xml:ns:caldav">&x;</c:schedule-response>"#,
                &limits
            ),
            Err(ParseError::Dtd)
        );
        // A recipient with spaces or controls, a missing status: kept as
        // unknown, never guessed.
        let body = r#"<c:schedule-response xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:d="DAV:"><c:response><c:recipient><d:href>mailto:a b@x</d:href></c:recipient></c:response></c:schedule-response>"#;
        let parsed = parse_schedule_response(body.as_bytes(), &limits).unwrap();
        assert_eq!(parsed[0].recipient, None);
        assert_eq!(parsed[0].status, None);
        // Too many recipients.
        let many = format!(
            r#"<c:schedule-response xmlns:c="urn:ietf:params:xml:ns:caldav">{}</c:schedule-response>"#,
            "<c:response/>".repeat(MAX_SCHEDULE_RESPONSES + 1)
        );
        assert!(parse_schedule_response(many.as_bytes(), &limits).is_err());
        // calendar-data over the value limit.
        let small = XmlLimits {
            max_value_bytes: 8,
            ..XmlLimits::default()
        };
        let big = r#"<c:schedule-response xmlns:c="urn:ietf:params:xml:ns:caldav"><c:response><c:calendar-data>0123456789</c:calendar-data></c:response></c:schedule-response>"#;
        assert_eq!(
            parse_schedule_response(big.as_bytes(), &small),
            Err(ParseError::ValueTooLarge)
        );
    }
}
