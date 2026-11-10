// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The strict JSON parser never panics, never accepts what serde_json
//! rejects, and when it accepts, gives the same value.

#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;
use nebula_jmap_proto::request::from_slice_strict;

fuzz_target!(|data: &[u8]| {
    if let Ok(strict) = from_slice_strict(data) {
        let loose: Value = serde_json::from_slice(data).expect("serde_json accepts it too");
        assert_eq!(strict, loose);
    }
});
