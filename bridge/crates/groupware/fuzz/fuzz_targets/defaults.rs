// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What a DAV server says of a calendar's defaults (`default-alarm-vevent-*`
//! values, zone names) and the text the query filters read: never a panic
//! (ours, or calcard's caught by our wrapper), and the alarms the bridge
//! writes back read the same.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_groupware::ConvertError;
use nebula_groupware::model::defaults::{DefaultAlerts, SignedDuration, calendar_zone};
use nebula_groupware::model::search::{CardText, EventText, fold};

fn check<T>(result: Result<T, ConvertError>) {
    if let Err(ConvertError::Panicked { format }) = result {
        panic!("calcard panicked on {format}");
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let alerts = DefaultAlerts::from_valarms(text);
        let again = DefaultAlerts::from_valarms(&alerts.to_valarms());
        assert_eq!(alerts, again, "default alarms do not read back the same");
        if let Some(duration) = SignedDuration::parse(text) {
            assert_eq!(SignedDuration::parse(&duration.to_string()), Some(duration));
        }
        let _ = calendar_zone(Some(text), Some(text));
        let _ = fold(text);
    }
    check(EventText::parse(data));
    check(CardText::parse(data));
});
