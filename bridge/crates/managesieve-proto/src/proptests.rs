// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Round trips: whatever we encode decodes to the same value, however the
//! bytes are split; random bytes never panic.

#![allow(clippy::arithmetic_side_effects, reason = "test code")]

use proptest::prelude::*;
use zeroize::Zeroizing;

use crate::item::{Framing, Scan, Side, scan_line};
use crate::{
    Atom, Capabilities, Command, CommandDecoder, Completion, Item, Limits, Mechanism, ResponseCode,
    ResponseDecoder, SaslData, ScriptName, ServerLine, Status,
};

fn atom() -> impl Strategy<Value = Atom> {
    "[A-Za-z0-9!#$%&'*+,./:;<=>?@^_`|}~\\[\\]-]{1,12}".prop_filter_map("atom", |s| Atom::new(&s))
}

fn quotable() -> impl Strategy<Value = Vec<u8>> {
    "[^\r\n\u{0}]{0,40}".prop_map(String::into_bytes)
}

fn any_bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..64)
}

fn string_item() -> impl Strategy<Value = Item> {
    prop_oneof![
        quotable().prop_map(Item::Quoted),
        any_bytes().prop_map(Item::Literal),
    ]
}

fn data_item() -> impl Strategy<Value = Item> {
    prop_oneof![string_item(), atom().prop_map(Item::Atom)]
}

fn code() -> impl Strategy<Value = ResponseCode> {
    prop_oneof![
        Just(ResponseCode::Quota),
        Just(ResponseCode::QuotaMaxSize),
        Just(ResponseCode::QuotaMaxScripts),
        Just(ResponseCode::Nonexistent),
        Just(ResponseCode::AlreadyExists),
        Just(ResponseCode::Active),
        Just(ResponseCode::Warnings),
        Just(ResponseCode::TryLater),
        Just(ResponseCode::EncryptNeeded),
        Just(ResponseCode::AuthTooWeak),
        Just(ResponseCode::TransitionNeeded),
        quotable().prop_map(ResponseCode::Tag),
        quotable().prop_map(ResponseCode::Sasl),
        quotable().prop_map(ResponseCode::Referral),
        (atom(), proptest::collection::vec(data_item(), 0..3))
            .prop_filter("not a known code", |(name, _)| {
                ![
                    "AUTH-TOO-WEAK",
                    "ENCRYPT-NEEDED",
                    "QUOTA",
                    "QUOTA/MAXSCRIPTS",
                    "QUOTA/MAXSIZE",
                    "TRANSITION-NEEDED",
                    "TRYLATER",
                    "ACTIVE",
                    "NONEXISTENT",
                    "ALREADYEXISTS",
                    "WARNINGS",
                    "SASL",
                    "TAG",
                    "REFERRAL",
                ]
                .iter()
                .any(|known| name.is(known))
            })
            .prop_map(|(name, data)| ResponseCode::Other { name, data }),
    ]
}

fn server_line() -> impl Strategy<Value = ServerLine> {
    prop_oneof![
        (string_item(), proptest::collection::vec(data_item(), 0..4)).prop_map(|(first, rest)| {
            ServerLine::Data(std::iter::once(first).chain(rest).collect())
        }),
        (
            prop_oneof![Just(Status::Ok), Just(Status::No), Just(Status::Bye)],
            proptest::option::of(code()),
            proptest::option::of(prop_oneof![quotable(), any_bytes()]),
        )
            .prop_map(|(status, code, text)| ServerLine::Done(Completion {
                status,
                code,
                text
            })),
    ]
}

fn name() -> impl Strategy<Value = ScriptName> {
    "[^\u{0}-\u{1f}\u{7f}-\u{9f}\u{2028}\u{2029}]{1,20}"
        .prop_filter_map("name", |s| ScriptName::new(s).ok())
}

fn sasl() -> impl Strategy<Value = SaslData> {
    any_bytes().prop_map(|bytes| SaslData::new(Zeroizing::new(bytes)))
}

fn command() -> impl Strategy<Value = Command> {
    prop_oneof![
        Just(Command::StartTls),
        Just(Command::Logout),
        Just(Command::Capability),
        Just(Command::ListScripts),
        Just(Command::Unauthenticate),
        proptest::option::of(quotable()).prop_map(|tag| Command::Noop { tag }),
        ("[A-Z0-9_-]{1,20}", proptest::option::of(sasl())).prop_map(|(m, initial_response)| {
            Command::Authenticate {
                mechanism: Mechanism::new(&m).unwrap_or_else(Mechanism::plain),
                initial_response,
            }
        }),
        (name(), any::<u64>()).prop_map(|(name, size)| Command::HaveSpace { name, size }),
        (name(), any_bytes()).prop_map(|(name, content)| Command::PutScript { name, content }),
        proptest::option::of(name()).prop_map(|name| Command::SetActive { name }),
        name().prop_map(|name| Command::GetScript { name }),
        name().prop_map(|name| Command::DeleteScript { name }),
        (name(), name()).prop_map(|(from, to)| Command::RenameScript { from, to }),
        any_bytes().prop_map(|content| Command::CheckScript { content }),
    ]
}

/// Feeds `bytes` in pieces cut at `cuts` and collects what comes out.
fn split_feed<T>(
    bytes: &[u8],
    cuts: &[usize],
    mut feed: impl FnMut(&[u8]),
    mut take: impl FnMut() -> Option<T>,
) -> Vec<T> {
    let mut points: Vec<usize> = cuts.iter().map(|c| c % (bytes.len() + 1)).collect();
    points.push(0);
    points.push(bytes.len());
    points.sort_unstable();
    let mut out = Vec::new();
    for pair in points.windows(2) {
        if let [start, end] = pair {
            feed(bytes.get(*start..*end).unwrap_or_default());
            while let Some(value) = take() {
                out.push(value);
            }
        }
    }
    out
}

/// How decoding a buffer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum End {
    /// Waiting for more bytes.
    Incomplete,
    /// An error.
    Error(crate::DecodeError),
}

/// The old way: `scan_line` from the start of the buffer, line after line.
fn scan_whole(bytes: &[u8], limits: &Limits, side: Side) -> (Vec<Vec<Item>>, End) {
    let mut rest = bytes;
    let mut lines = Vec::new();
    loop {
        match scan_line(rest, limits, side) {
            Ok(Scan::Line { items, consumed }) => {
                lines.push(items);
                rest = rest.get(consumed..).unwrap_or_default();
            }
            Ok(Scan::Incomplete) => return (lines, End::Incomplete),
            Err(error) => return (lines, End::Error(error)),
        }
    }
}

/// The framer, fed in pieces cut at `cuts`.
fn frame_pieces(
    bytes: &[u8],
    cuts: &[usize],
    limits: &Limits,
    side: Side,
) -> (Vec<Vec<Item>>, End) {
    let mut points: Vec<usize> = cuts.iter().map(|cut| cut % (bytes.len() + 1)).collect();
    points.push(bytes.len());
    points.sort_unstable();
    let mut framing = Framing::default();
    let mut buffer = Vec::new();
    let mut lines = Vec::new();
    let mut start = 0;
    for point in points {
        buffer.extend_from_slice(&bytes[start..point]);
        start = point;
        loop {
            match framing.next_line(&buffer, limits, side) {
                Ok(Some((items, consumed))) => {
                    lines.push(items);
                    buffer.drain(..consumed);
                }
                Ok(None) => break,
                Err(error) => return (lines, End::Error(error)),
            }
        }
    }
    (lines, End::Incomplete)
}

/// Bytes close to the grammar, so lines often end and literals often fit.
fn near_grammar() -> impl Strategy<Value = Vec<u8>> {
    let piece = prop_oneof![
        Just(&b"a"[..]),
        Just(&b"OK"[..]),
        Just(&b" "[..]),
        Just(&b"\""[..]),
        Just(&b"\\"[..]),
        Just(&b"{"[..]),
        Just(&b"}"[..]),
        Just(&b"+"[..]),
        Just(&b"3"[..]),
        Just(&b"("[..]),
        Just(&b")"[..]),
        Just(&b"\r"[..]),
        Just(&b"\n"[..]),
        Just(&b"\r\n"[..]),
        Just(&b"\r\n"[..]),
        Just(&b"{2}\r\n"[..]),
        Just(&b"{1+}\r\n"[..]),
        Just(&b"\0"[..]),
        Just(&b"\xff"[..]),
    ];
    proptest::collection::vec(piece, 0..40).prop_map(|pieces| pieces.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn server_lines_round_trip(
        lines in proptest::collection::vec(server_line(), 1..6),
        cuts in proptest::collection::vec(any::<usize>(), 0..6),
    ) {
        let bytes: Vec<u8> = lines.iter().flat_map(ServerLine::encode).collect();
        let decoder = std::cell::RefCell::new(ResponseDecoder::new(Limits::default()));
        let decoded = split_feed(
            &bytes,
            &cuts,
            |piece| decoder.borrow_mut().feed(piece),
            || decoder.borrow_mut().decode().unwrap(),
        );
        prop_assert_eq!(decoded, lines);
        prop_assert_eq!(decoder.borrow().buffered(), 0);
    }

    #[test]
    fn commands_round_trip(
        commands in proptest::collection::vec(command(), 1..6),
        cuts in proptest::collection::vec(any::<usize>(), 0..6),
    ) {
        let bytes: Vec<u8> = commands.iter().flat_map(|c| c.encode().to_vec()).collect();
        let decoder = std::cell::RefCell::new(CommandDecoder::new(Limits::default()));
        let decoded = split_feed(
            &bytes,
            &cuts,
            |piece| decoder.borrow_mut().feed(piece),
            || decoder.borrow_mut().decode_command().unwrap(),
        );
        prop_assert_eq!(decoded, commands);
    }

    #[test]
    fn framing_matches_the_whole_buffer_scan(
        bytes in near_grammar(),
        cuts in proptest::collection::vec(any::<usize>(), 0..8),
        client in any::<bool>(),
        small in any::<bool>(),
    ) {
        let side = if client { Side::Client } else { Side::Server };
        let limits = if small {
            Limits { max_line: 24, max_literal: 4, max_line_literals: 6, max_items: 6, max_depth: 2 }
        } else {
            Limits::default()
        };
        let whole = frame_pieces(&bytes, &[], &limits, side);
        // However the bytes are cut, the same lines and the same end.
        prop_assert_eq!(&frame_pieces(&bytes, &cuts, &limits, side), &whole);
        // The same lines as scanning the whole buffer. Where that gives an
        // error, the framer may still be waiting for a CRLF (it leaves
        // grammar errors to the parse of the complete line), and the other
        // way round; the error itself may differ.
        let (old_lines, _old_end) = scan_whole(&bytes, &limits, side);
        prop_assert_eq!(&whole.0, &old_lines);
    }

    #[test]
    fn capabilities_round_trip(
        sieve in proptest::collection::vec("[a-z0-9;:-]{1,12}", 0..20),
        sasl in proptest::collection::vec("[A-Z0-9-]{1,10}", 0..4),
        redirects in proptest::option::of(any::<u32>()),
        starttls in any::<bool>(),
    ) {
        let capabilities = Capabilities {
            implementation: Some("Test".to_owned()),
            sasl,
            sieve,
            starttls,
            max_redirects: redirects,
            version: Some("1.0".to_owned()),
            ..Capabilities::default()
        };
        prop_assert_eq!(Capabilities::parse(&capabilities.to_lines()).unwrap(), capabilities);
    }

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut responses = ResponseDecoder::new(Limits::default());
        responses.feed(&bytes);
        let mut lines = Vec::new();
        while let Ok(Some(ServerLine::Data(items))) = responses.decode() {
            lines.push(items);
        }
        let _ = Capabilities::parse(&lines);
        let mut commands = CommandDecoder::new(Limits::default());
        commands.feed(&bytes);
        while let Ok(Some(_)) = commands.decode_command() {}
    }
}
