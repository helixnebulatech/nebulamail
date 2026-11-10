// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Whatever a server sends: the decoder never panics, and every line it
//! accepts encodes back to bytes that decode to the same line. The data
//! lines also go through the capability and LISTSCRIPTS readers.

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_managesieve_proto::{
    Capabilities, Limits, ListedScript, ResponseDecoder, ScriptName, ServerLine,
};

fuzz_target!(|data: &[u8]| {
    // The first byte splits the input in two feeds (bytes arrive in pieces).
    let (split, data) = match data.split_first() {
        Some((first, rest)) => (usize::from(*first), rest),
        None => return,
    };
    let split = split.min(data.len());
    let limits = Limits {
        max_line: 4096,
        max_literal: 4096,
        max_line_literals: 8192,
        ..Limits::default()
    };
    let mut decoder = ResponseDecoder::new(limits);
    let mut lines = Vec::new();
    for piece in [&data[..split], &data[split..]] {
        decoder.feed(piece);
        while let Ok(Some(line)) = decoder.decode() {
            lines.push(line);
        }
    }
    let mut data_lines = Vec::new();
    for line in lines {
        let mut again = ResponseDecoder::new(Limits::default());
        again.feed(&line.encode());
        assert_eq!(again.decode().ok().flatten().as_ref(), Some(&line));
        if let ServerLine::Data(items) = line {
            let _ = ListedScript::parse(&items);
            data_lines.push(items);
        }
    }
    let _ = Capabilities::parse(&data_lines);
    let _ = ScriptName::from_bytes(data);
});
