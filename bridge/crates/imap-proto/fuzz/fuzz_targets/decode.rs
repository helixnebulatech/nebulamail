// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The response decoder must never panic, whatever the server sends.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_imap_proto::response::{FrameLimits, ResponseDecoder};

fuzz_target!(|data: &[u8]| {
    let mut decoder = ResponseDecoder::new(FrameLimits::default());
    decoder.feed(data);
    while let Ok(Some(_)) = decoder.decode() {}
});
