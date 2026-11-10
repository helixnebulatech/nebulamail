// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Header and property values (`DAV:` header, ETag, sync token, ctag,
//! colour, status line): never panic, and the opaque ones that parse are
//! safe to send back in a header.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_dav_proto::{CTag, CalendarColor, DavCapabilities, ETag, Status, SyncToken};

fn header_safe(text: &str) -> bool {
    text.bytes().all(|b| (0x20..0x7f).contains(&b))
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = DavCapabilities::from_headers(text.split('\n'));
    if let Some(etag) = ETag::parse(text) {
        assert!(header_safe(etag.as_str()));
    }
    if let Some(token) = SyncToken::parse(text) {
        assert!(header_safe(token.as_str()));
    }
    if let Some(ctag) = CTag::parse(text) {
        assert!(header_safe(ctag.as_str()));
    }
    if let Some(color) = CalendarColor::parse(text) {
        assert_eq!(CalendarColor::parse(&color.to_string()), Some(color));
    }
    if let Some(status) = Status::parse_line(text) {
        assert!((100..=599).contains(&status.code()));
    }
});
