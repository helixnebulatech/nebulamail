// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! iCalendar from a DAV server: calcard never panics parsing it, converting
//! it to JSCalendar and back, or expanding its recurrences. Our wrapper
//! catches panics, so a caught one is turned back into a crash here.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_groupware::ConvertError;
use nebula_groupware::model::convert::{
    expand_icalendar, icalendar_to_jscalendar, jscalendar_to_icalendar,
};

fn check<T>(result: Result<T, ConvertError>) -> Option<T> {
    match result {
        Err(ConvertError::Panicked { format }) => panic!("calcard panicked on {format}"),
        other => other.ok(),
    }
}

fuzz_target!(|data: &[u8]| {
    if let Some(json) = check(icalendar_to_jscalendar(data)) {
        check(jscalendar_to_icalendar(&json.to_string()));
    }
    check(expand_icalendar(data, 1000));
});
