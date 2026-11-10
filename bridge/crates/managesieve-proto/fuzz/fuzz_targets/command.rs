// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Whatever a client sends: the command decoder never panics, and every
//! command it accepts encodes to bytes that decode to the same command.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_managesieve_proto::{CommandDecoder, Limits};

fuzz_target!(|data: &[u8]| {
    let mut decoder = CommandDecoder::new(Limits::default());
    decoder.feed(data);
    while let Ok(Some(command)) = decoder.decode_command() {
        let mut again = CommandDecoder::new(Limits::default());
        again.feed(&command.encode());
        assert_eq!(again.decode_command().ok().flatten(), Some(command));
    }
    let mut sasl = CommandDecoder::new(Limits::default());
    sasl.feed(data);
    while let Ok(Some(_)) = sasl.decode_sasl() {}
});
