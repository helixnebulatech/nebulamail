// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Proposals of another time (`model::counter`): the iCalendar of an
//! invitation or a `COUNTER` email comes from anyone. Never a panic (ours,
//! or calcard's caught by our wrapper), and whatever is built parses.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_groupware::ConvertError;
use nebula_groupware::model::calendar::RecurrenceKey;
use nebula_groupware::model::counter::{
    CounterError, CounterSource, Proposal, counter, decline_counter,
};
use nebula_groupware::model::itip::ItipMessage;

const STORED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
    UID:s@example.com\r\nDTSTAMP:20261001T000000Z\r\nSEQUENCE:1\r\n\
    DTSTART;TZID=Europe/Madrid:20261019T100000\r\nDURATION:PT1H\r\n\
    RRULE:FREQ=WEEKLY;COUNT=4\r\nSUMMARY:Weekly\r\nORGANIZER:mailto:alice@example.com\r\n\
    ATTENDEE:mailto:bob@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn check(result: Result<ItipMessage, CounterError>) {
    match result {
        Err(CounterError::Convert(ConvertError::Panicked { format })) => {
            panic!("calcard panicked on {format}")
        }
        Ok(message) => assert!(
            message.ics.starts_with("BEGIN:VCALENDAR"),
            "not iCalendar"
        ),
        Err(_) => {}
    }
}

fuzz_target!(|data: &[u8]| {
    let bob = vec!["bob@example.com".to_owned()];
    let alice = vec!["alice@example.com".to_owned()];
    let comment = std::str::from_utf8(data).ok();
    let Some(proposal) = Proposal::new(1_792_483_200, 1_792_486_800) else {
        return;
    };
    check(counter(
        CounterSource::Invitation(data),
        &bob,
        proposal,
        comment,
        0,
    ));
    let key = data
        .get(..8)
        .and_then(|b| <[u8; 8]>::try_from(b).ok())
        .and_then(|b| RecurrenceKey::from_naive(i64::from_le_bytes(b) % 400_000_000_000));
    check(counter(
        CounterSource::Stored {
            text: data,
            occurrence: key,
        },
        &bob,
        proposal,
        None,
        0,
    ));
    check(counter(
        CounterSource::Stored {
            text: STORED.as_bytes(),
            occurrence: key,
        },
        &bob,
        proposal,
        comment,
        0,
    ));
    check(decline_counter(STORED.as_bytes(), data, &alice, comment, 0));
    check(decline_counter(data, data, &alice, None, 0));
});
