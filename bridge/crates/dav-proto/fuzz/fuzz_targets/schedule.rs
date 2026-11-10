// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Free-busy and principal answers (`schedule-response`,
//! `principal-search-property-set`) and the values that go back into a
//! free-busy request: never panic; a parsed address or status code is
//! safe to write into an iCalendar line.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_dav_proto::{
    CalendarAddress, CalendarUserType, RequestStatus, XmlLimits,
    parse_principal_search_property_set, parse_schedule_response,
};

fn line_safe(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, ';' | ',' | '"'))
}

fuzz_target!(|data: &[u8]| {
    let limits = XmlLimits::default();
    if let Ok(responses) = parse_schedule_response(data, &limits) {
        for response in responses {
            if let Some(data) = &response.calendar_data {
                assert!(data.len() <= limits.max_value_bytes * 2);
            }
        }
    }
    let _ = parse_principal_search_property_set(data, &limits);
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = CalendarUserType::parse(text);
    if let Some(status) = RequestStatus::parse(text) {
        assert!((1..=5).contains(&status.class()));
        assert!(line_safe(status.code()));
    }
    if let Some(address) = CalendarAddress::parse(text) {
        assert!(line_safe(address.address()));
        assert!(address.as_str().starts_with("mailto:"));
        assert_eq!(CalendarAddress::parse(address.as_str()), Some(address));
    }
});
