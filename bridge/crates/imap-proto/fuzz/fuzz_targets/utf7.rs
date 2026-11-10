// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Modified UTF-7: decoding never panics, and valid names round-trip.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_imap_proto::types::utf7;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(name) = utf7::decode(text) {
        assert_eq!(utf7::encode(&name), text);
    }
    assert_eq!(utf7::decode(&utf7::encode(text)).as_deref(), Ok(text));
});
