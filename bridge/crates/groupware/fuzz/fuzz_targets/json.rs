// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JSCalendar/JSContact JSON from a JMAP client (`CalendarEvent/set`,
//! `ContactCard/set`): calcard never panics writing it as iCalendar/vCard.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_groupware::ConvertError;
use nebula_groupware::model::convert::{jscalendar_to_icalendar, jscontact_to_vcard};

fn check<T>(result: Result<T, ConvertError>) {
    if let Err(ConvertError::Panicked { format }) = result {
        panic!("calcard panicked on {format}");
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(json) = std::str::from_utf8(data) {
        check(jscalendar_to_icalendar(json));
        check(jscontact_to_vcard(json));
    }
});
