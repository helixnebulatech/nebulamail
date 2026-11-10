// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Decoder tests: real server responses, gaps of `imap-proto`, hostile input.

use proptest::prelude::*;

use super::*;
use crate::types::{Flag, SpecialUse};

fn parse(input: &[u8]) -> Response {
    parse_response(input, NameEncoding::ModifiedUtf7, &FrameLimits::default())
        .expect("one complete response")
}

fn untagged(input: &[u8]) -> Untagged {
    match parse(input) {
        Response::Untagged(data) => data,
        other => panic!("not untagged: {other:?}"),
    }
}

fn tagged(input: &[u8]) -> Tagged {
    match parse(input) {
        Response::Tagged(tagged) => tagged,
        other => panic!("not tagged: {other:?}"),
    }
}

fn fetch(input: &[u8]) -> FetchData {
    match untagged(input) {
        Untagged::Fetch(data) => *data,
        other => panic!("not a fetch: {other:?}"),
    }
}

fn is_unknown(input: &[u8]) -> bool {
    matches!(untagged(input), Untagged::Unknown(_))
}

#[test]
fn greeting_with_capabilities() {
    let Untagged::Status(status) = untagged(
        b"* OK [CAPABILITY IMAP4rev2 IMAP4rev1 AUTH=PLAIN SASL-IR LITERAL+] Stalwart ready\r\n",
    ) else {
        panic!()
    };
    assert_eq!(status.condition, Condition::Ok);
    let Some(ResponseCode::Capabilities(caps)) = status.code else {
        panic!()
    };
    assert!(caps.has_auth(&crate::types::AuthMechanism::Plain));
    assert_eq!(status.text, "Stalwart ready");
}

#[test]
fn capability_lines() {
    let Untagged::Capabilities(caps) = untagged(b"* CAPABILITY IMAP4rev2 IDLE MOVE\r\n") else {
        panic!()
    };
    assert!(caps.has(&Capability::Move));
    let Untagged::Enabled(list) = untagged(b"* ENABLED QRESYNC UTF8=ACCEPT\r\n") else {
        panic!()
    };
    assert_eq!(list, vec![Capability::QResync, Capability::Utf8Accept]);
}

#[test]
fn tagged_codes() {
    let done = tagged(b"A1 OK [READ-WRITE] SELECT completed\r\n");
    assert_eq!(done.status, Completion::Ok);
    assert_eq!(done.code, Some(ResponseCode::ReadWrite));
    let done = tagged(b"A2 NO [AUTHENTICATIONFAILED] Authentication failed.\r\n");
    assert_eq!(done.status, Completion::No);
    assert_eq!(done.code, Some(ResponseCode::AuthenticationFailed));
    assert_eq!(done.text, "Authentication failed.");
    let done = tagged(b"A3 OK [APPENDUID 38505 3955] APPEND completed\r\n");
    assert!(
        matches!(done.code, Some(ResponseCode::AppendUid { uid_validity, ref uids })
        if uid_validity.get() == 38505 && uids.to_string() == "3955")
    );
    let done = tagged(b"A4 OK [COPYUID 38505 304,319:320 3956:3958] Done\r\n");
    assert!(
        matches!(done.code, Some(ResponseCode::CopyUid { ref source, ref destination, .. })
        if source.to_string() == "304,319:320" && destination.to_string() == "3956:3958")
    );
    let done = tagged(b"A5 OK [MODIFIED 7,9] Conditional STORE failed\r\n");
    assert!(matches!(done.code, Some(ResponseCode::Modified(_))));
    let done = tagged(b"A6 BAD\r\n");
    assert_eq!(done.status, Completion::Bad);
    let done = tagged(b"A7 OK [CAPABILITY IMAP4rev2 IDLE] Logged in\r\n");
    assert!(matches!(done.code, Some(ResponseCode::Capabilities(_))));
}

#[test]
fn untagged_codes() {
    let Untagged::Status(status) =
        untagged(b"* OK [NOMODSEQ] Sorry, this mailbox format doesn't support modsequences\r\n")
    else {
        panic!()
    };
    assert_eq!(status.code, Some(ResponseCode::NoModSeq));
    let Untagged::Status(status) = untagged(b"* OK [CLOSED] Previous mailbox closed\r\n") else {
        panic!()
    };
    assert_eq!(status.code, Some(ResponseCode::Closed));
    let Untagged::Status(status) = untagged(b"* OK [HIGHESTMODSEQ 715194045007]\r\n") else {
        panic!()
    };
    assert_eq!(
        status.code,
        Some(ResponseCode::HighestModSeq(
            ModSeq::new(715_194_045_007).unwrap()
        ))
    );
    let Untagged::Status(status) = untagged(b"* OK [UIDNEXT 0] bad\r\n") else {
        panic!()
    };
    assert_eq!(
        status.code,
        Some(ResponseCode::Malformed("UIDNEXT".to_owned()))
    );
    let Untagged::Status(status) =
        untagged(b"* OK [PERMANENTFLAGS (\\Deleted \\Seen \\*)] Limited\r\n")
    else {
        panic!()
    };
    assert!(
        matches!(status.code, Some(ResponseCode::PermanentFlags { new_keywords: NewKeywords::Allowed, ref flags }) if flags.len() == 2)
    );
    let Untagged::Status(bye) = untagged(b"* BYE Autologout\r\n") else {
        panic!()
    };
    assert_eq!(bye.condition, Condition::Bye);
}

#[test]
fn fetch_with_extensions() {
    let data = fetch(
        b"* 12 FETCH (UID 3955 FLAGS (\\Seen $Forwarded) MODSEQ (624140003) EMAILID (M6d99ac3275bb4e) \
          THREADID (T64b478a75b7ea9) RFC822.SIZE 4286 INTERNALDATE \"17-Jul-1996 02:44:25 -0700\" \
          PREVIEW \"Hola, qu\xc3\xa9 tal\" BODY[HEADER.FIELDS (FROM SUBJECT)] {22}\r\nSubject: hi\r\nFrom: a\r\n)\r\n",
    );
    assert_eq!(data.seq.get(), 12);
    assert_eq!(data.uid.map(Uid::get), Some(3955));
    assert_eq!(data.flags.as_deref().map(<[Flag]>::len), Some(2));
    assert_eq!(data.mod_seq.map(ModSeq::get), Some(624_140_003));
    assert_eq!(
        data.email_id.as_ref().map(ObjectId::as_str),
        Some("M6d99ac3275bb4e")
    );
    assert!(matches!(data.thread_id, Some(ThreadId::Id(_))));
    assert_eq!(data.size, Some(4286));
    assert_eq!(
        data.internal_date.map(|d| d.unix_timestamp()),
        Some(837_596_665)
    );
    assert_eq!(
        data.preview,
        Some(Preview::Text("Hola, qué tal".to_owned()))
    );
    assert_eq!(data.sections.len(), 1);
    assert_eq!(data.sections[0].section, SectionKind::Header);
    assert_eq!(
        data.sections[0].data.as_deref(),
        Some(&b"Subject: hi\r\nFrom: a\r\n"[..])
    );
}

#[test]
fn fetch_objectid_plus() {
    let data = fetch(b"* 1 FETCH (UID 4 OBJECTID (EMAILID e123 THREADID t9) FLAGS ())\r\n");
    assert_eq!(data.email_id.as_ref().map(ObjectId::as_str), Some("e123"));
    assert_eq!(
        data.thread_id,
        Some(ThreadId::Id(ObjectId::new("t9").unwrap()))
    );
    assert_eq!(data.flags, Some(Vec::new()));
    assert!(is_unknown(
        b"* 1 FETCH (UID 4 OBJECTID (EMAILID \"a b\"))\r\n"
    ));
}

#[test]
fn fetch_nil_values() {
    let data = fetch(b"* 1 FETCH (UID 1 THREADID NIL PREVIEW NIL)\r\n");
    assert_eq!(data.thread_id, Some(ThreadId::None));
    assert_eq!(data.preview, Some(Preview::None));
    let data = fetch(b"* 1 FETCH (UID 1 PREVIEW {5}\r\nhello)\r\n");
    assert_eq!(data.preview, Some(Preview::Text("hello".to_owned())));
}

#[test]
fn fetch_body_structure() {
    let data = fetch(
        b"* 1 FETCH (UID 2 BODYSTRUCTURE ((\"TEXT\" \"PLAIN\" (\"CHARSET\" \"utf-8\") NIL NIL \"7BIT\" 10 1 NIL NIL NIL NIL)\
          (\"APPLICATION\" \"PDF\" (\"NAME\" \"a.pdf\") NIL NIL \"BASE64\" 100 NIL (\"ATTACHMENT\" (\"FILENAME\" \"a.pdf\")) NIL NIL) \
          \"MIXED\" (\"BOUNDARY\" \"x\") NIL NIL NIL))\r\n",
    );
    let body = data.body_structure.unwrap();
    assert!(body.has_attachment());
    let BodyStructure::Multipart(multipart) = body else {
        panic!()
    };
    assert_eq!(multipart.common.subtype, "mixed");
    assert_eq!(multipart.parts.len(), 2);
}

#[test]
fn invalid_values_become_unknown() {
    assert!(is_unknown(b"* 1 FETCH (UID 0)\r\n"));
    assert!(is_unknown(b"* 0 EXPUNGE\r\n"));
    assert!(is_unknown(b"* 1 FETCH (EMAILID (a.b))\r\n"));
    // A flag we cannot represent is dropped, not the whole FETCH.
    assert_eq!(
        fetch(b"* 1 FETCH (FLAGS (\\*))\r\n").flags,
        Some(Vec::new())
    );
}

#[test]
fn unknown_says_what_it_was() {
    let about = |input: &[u8]| match untagged(input) {
        Untagged::Unknown(unknown) => unknown.about,
        other => panic!("{other:?}"),
    };
    assert_eq!(about(b"* SEARCH 0\r\n"), UnknownKind::Search);
    assert_eq!(about(b"* ESEARCH UID ALL 1:*\r\n"), UnknownKind::ESearch);
    assert_eq!(about(b"* 1 FETCH (UID 0)\r\n"), UnknownKind::Fetch);
    assert_eq!(
        about(b"* METADATA \"INBOX\" (/shared/comment \"x\")\r\n"),
        UnknownKind::Other
    );
    assert_eq!(about(b"* 99 BOGUS stuff\r\n"), UnknownKind::Other);
}

#[test]
fn fetch_keeps_valid_flags() {
    let long = "k".repeat(300);
    let input = format!("* 1 FETCH (UID 4 FLAGS (\\Seen {long} $Junk))\r\n");
    let data = fetch(input.as_bytes());
    assert_eq!(data.uid.map(Uid::get), Some(4));
    assert_eq!(
        data.flags,
        Some(vec![Flag::Seen, Flag::parse("$Junk").unwrap()])
    );
    // Same in a BINARY FETCH (our own parser) and in PERMANENTFLAGS.
    let input = format!("* 1 FETCH (UID 4 FLAGS ({long} \\Seen) BINARY.SIZE[1] 3)\r\n");
    assert_eq!(fetch(input.as_bytes()).flags, Some(vec![Flag::Seen]));
    let input = format!("* OK [PERMANENTFLAGS (\\Seen {long} \\*)] ok\r\n");
    let Untagged::Status(status) = untagged(input.as_bytes()) else {
        panic!()
    };
    assert_eq!(
        status.code,
        Some(ResponseCode::PermanentFlags {
            flags: vec![Flag::Seen],
            new_keywords: NewKeywords::Allowed
        })
    );
}

#[test]
fn fetch_with_bad_internaldate_has_none() {
    for date in [
        "\"99-Foo-2024 00:00:00 +0000\"",
        "\"31-Dec-9999 23:59:59 -0001\"",
    ] {
        let input = format!("* 1 FETCH (UID 7 INTERNALDATE {date} FLAGS (\\Seen))\r\n");
        let data = fetch(input.as_bytes());
        assert_eq!(data.uid.map(Uid::get), Some(7));
        assert_eq!(data.internal_date, None);
        assert_eq!(data.flags, Some(vec![Flag::Seen]));
    }
}

#[test]
fn fetch_with_bracket_keyword() {
    let data = fetch(b"* 1 FETCH (UID 5 FLAGS (\\Seen [x a[b))\r\n");
    assert_eq!(
        data.flags,
        Some(vec![
            Flag::Seen,
            Flag::parse("[x").unwrap(),
            Flag::parse("a[b").unwrap()
        ])
    );
    let data = fetch(b"* 1 FETCH (UID 5 FLAGS (\\Seen [x) BODY[] {3}\r\nabc)\r\n");
    assert_eq!(data.flags.map(|flags| flags.len()), Some(2));
    assert_eq!(data.sections[0].data.as_deref(), Some(&b"abc"[..]));
}

#[test]
fn section_brackets_still_work() {
    let data = fetch(b"* 1 FETCH (UID 1 BODY[HEADER.FIELDS (FROM \"X]\")]<0> {3}\r\nabc)\r\n");
    assert_eq!(data.sections.len(), 1);
    assert_eq!(data.sections[0].data.as_deref(), Some(&b"abc"[..]));
    let data = fetch(b"* 1 FETCH (UID 1 BINARY.SIZE[1] 3)\r\n");
    assert_eq!(data.binary_sizes, vec![(vec![1], 3)]);
}

#[test]
fn glued_quote_is_malformed() {
    assert!(is_unknown(b"* 1 FETCH (UID 1 X a\"b\" FLAGS ())\r\n"));
    assert!(is_unknown(b"* 1 FETCH (UID 1 X a{1}\r\nb FLAGS ())\r\n"));
    assert!(is_unknown(b"* 1 FETCH (UID 1 X \"a\"(b) FLAGS ())\r\n"));
}

#[test]
fn unknown_items_must_be_flat() {
    // Accepted: atom, NIL, quoted string, list of atoms (SAVEDATE…).
    let data =
        fetch(b"* 1 FETCH (UID 1 SAVEDATE \"17-Jul-1996 02:44:25 -0700\" X NIL Y (a b) Z 5)\r\n");
    assert_eq!(data.uid.map(Uid::get), Some(1));
    // Refused: a literal, a nested list or a string inside a list.
    assert!(is_unknown(b"* 1 FETCH (UID 1 X {1}\r\na)\r\n"));
    assert!(is_unknown(b"* 1 FETCH (UID 1 X ((a)))\r\n"));
    assert!(is_unknown(b"* 1 FETCH (UID 1 X (\"a)\"))\r\n"));
    assert!(is_unknown(b"* 1 FETCH (UID 1 OBJECTID (FOO {1}\r\na))\r\n"));
}

/// `* 1 FETCH (<name> ({n}\r\n) BODYSTRUCTURE (... very deep ...))`: the
/// literal is one opaque value for us, but the catch-all of `imap-proto`
/// stops at its first byte (`)`) and parses the rest as protocol.
fn hidden_bodystructure(name: &str, levels: usize) -> Vec<u8> {
    let mut content =
        b") BODYSTRUCTURE (\"A\" \"B\" NIL NIL NIL \"7BIT\" 0 NIL NIL NIL NIL ".to_vec();
    content.extend(std::iter::repeat_n(b'(', levels));
    content.extend(std::iter::repeat_n(b')', levels));
    let mut frame = format!("* 1 FETCH ({name} ({{{}}}\r\n", content.len()).into_bytes();
    frame.extend_from_slice(&content);
    frame.extend_from_slice(b"))\r\n");
    frame
}

fn parse_on_small_stack(frame: Vec<u8>, limits: FrameLimits, stack: usize) -> Option<Response> {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || parse_response(&frame, NameEncoding::ModifiedUtf7, &limits))
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn unknown_attribute_literal_cannot_hide_bodystructure() {
    for name in ["X", "SAVEDATE", "FLAGS", "BODYSTRUCTURE", "OBJECTID"] {
        let frame = hidden_bodystructure(name, 100_000);
        let response = parse_on_small_stack(frame, FrameLimits::default(), 256 * 1024);
        assert!(
            matches!(response, Some(Response::Untagged(Untagged::Unknown(_)))),
            "{name}: {response:?}"
        );
    }
}

#[test]
fn bracket_atom_cannot_hide_bodystructure() {
    let mut frame = b"* 1 FETCH (UID 1 X [ BODYSTRUCTURE ".to_vec();
    frame.extend(std::iter::repeat_n(b'(', 20_000));
    frame.extend(std::iter::repeat_n(b')', 20_000));
    frame.extend_from_slice(b" Y ])\r\n");
    let limits = FrameLimits {
        max_line: 1 << 20,
        ..FrameLimits::default()
    };
    let response = parse_on_small_stack(frame, limits, 256 * 1024);
    assert!(
        matches!(response, Some(Response::Untagged(Untagged::Unknown(_)))),
        "{response:?}"
    );
}

#[test]
fn literal_with_parentheses_in_bodystructure_still_parses() {
    let name = "Factura (1).pdf";
    let input = format!(
        "* 1 FETCH (UID 2 BODYSTRUCTURE ((\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 1 1 NIL NIL NIL NIL)\
         (\"APPLICATION\" \"PDF\" (\"NAME\" {{{}}}\r\n{name}) NIL NIL \"BASE64\" 100 NIL NIL NIL NIL) \
         \"MIXED\" NIL NIL NIL NIL))\r\n",
        name.len()
    );
    let data = fetch(input.as_bytes());
    assert!(data.body_structure.is_some());
}

#[test]
fn token_flood_is_refused() {
    let limits = FrameLimits {
        max_line: 1 << 20,
        ..FrameLimits::default()
    };
    let mut frame = b"* 1 FETCH (UID 1 FLAGS (".to_vec();
    for _ in 0..200_000 {
        frame.extend_from_slice(b"a ");
    }
    frame.extend_from_slice(b"))\r\n");
    let response = parse_response(&frame, NameEncoding::ModifiedUtf7, &limits);
    assert!(
        matches!(response, Some(Response::Untagged(Untagged::Unknown(_)))),
        "{response:?}"
    );
    // A realistic big BODYSTRUCTURE (200 parts) is far below the limit.
    let part = "(\"TEXT\" \"PLAIN\" (\"CHARSET\" \"utf-8\") NIL NIL \"7BIT\" 10 1 NIL NIL NIL NIL)";
    let body = format!(
        "({} \"MIXED\" (\"BOUNDARY\" \"x\") NIL NIL NIL)",
        part.repeat(200)
    );
    let input = format!("* 1 FETCH (UID 2 BODYSTRUCTURE {body})\r\n");
    let response = parse_response(input.as_bytes(), NameEncoding::ModifiedUtf7, &limits);
    let Some(Response::Untagged(Untagged::Fetch(data))) = response else {
        panic!("{response:?}")
    };
    let Some(BodyStructure::Multipart(multipart)) = data.body_structure else {
        panic!()
    };
    assert_eq!(multipart.parts.len(), 200);
}

#[test]
fn search_lines_have_their_own_limit() {
    // 20,000 scattered 6-digit UIDs: about 140 KB, over `max_line`.
    let uids: Vec<u32> = (0..20_000).map(|i| 100_000 + i * 7).collect();
    let mut line = b"* SEARCH".to_vec();
    for uid in &uids {
        line.extend_from_slice(format!(" {uid}").as_bytes());
    }
    line.extend_from_slice(b"\r\n");
    let mut decoder = ResponseDecoder::new(FrameLimits::default());
    decoder.feed(&line);
    let Ok(Some(Response::Untagged(Untagged::Search(result)))) = decoder.decode() else {
        panic!()
    };
    let found: Vec<u32> = result.numbers.iter().map(|n| n.get()).collect();
    assert_eq!(found, uids);
    // Other lines keep `max_line` (a smaller one here): over it, the
    // response is dropped, not the connection.
    let small = FrameLimits {
        max_line: 64 * 1024,
        ..FrameLimits::default()
    };
    let too_large = |decoded: Result<Option<Response>, DecodeError>| {
        matches!(
            decoded,
            Ok(Some(Response::Untagged(Untagged::Unknown(Unknown {
                reason: UnknownReason::TooLarge,
                ..
            }))))
        )
    };
    let mut list = b"* LIST () \"/\" \"".to_vec();
    list.extend(std::iter::repeat_n(b'a', 70 * 1024));
    list.extend_from_slice(b"\"\r\n");
    let mut decoder = ResponseDecoder::new(small);
    decoder.feed(&list);
    assert!(too_large(decoder.decode()));
    // A SEARCH line of the same size fits `max_search_line`.
    let mut decoder = ResponseDecoder::new(small);
    decoder.feed(&line);
    assert!(matches!(
        decoder.decode(),
        Ok(Some(Response::Untagged(Untagged::Search(_))))
    ));
    // And SEARCH lines keep `max_search_line`.
    let mut decoder = ResponseDecoder::new(FrameLimits {
        max_search_line: 100,
        ..FrameLimits::default()
    });
    decoder.feed(&line);
    assert!(too_large(decoder.decode()));
}

#[test]
fn search_spacing() {
    for input in [&b"* SEARCH\r\n"[..], b"* SEARCH \r\n", b"* SEARCH  \r\n"] {
        let Untagged::Search(result) = untagged(input) else {
            panic!()
        };
        assert!(result.numbers.is_empty());
    }
    let Untagged::Search(result) = untagged(b"* SEARCH 1  2 \r\n") else {
        panic!()
    };
    assert_eq!(result.numbers.len(), 2);
    let Untagged::Search(result) = untagged(b"* SEARCH 1 (MODSEQ 9) \r\n") else {
        panic!()
    };
    assert_eq!(result.highest_mod_seq.map(ModSeq::get), Some(9));
    assert!(is_unknown(b"* SEARCH 1 x\r\n"));
    assert!(is_unknown(b"* SEARCH 1 (MODSEQ 9) 2\r\n"));
    assert!(is_unknown(b"* SEARCH 1 (FOO 9)\r\n"));
}

#[test]
fn vanished_and_expunge() {
    let Untagged::Vanished(vanished) = untagged(b"* VANISHED (EARLIER) 41,43:116,118\r\n") else {
        panic!()
    };
    assert_eq!(vanished.kind, VanishedKind::Earlier);
    assert_eq!(vanished.uids.to_string(), "41,43:116,118");
    let Untagged::Vanished(vanished) = untagged(b"* VANISHED 405\r\n") else {
        panic!()
    };
    assert_eq!(vanished.kind, VanishedKind::Now);
    assert_eq!(
        untagged(b"* 3 EXPUNGE\r\n"),
        Untagged::Expunge(SeqNum::new(3).unwrap())
    );
    assert_eq!(untagged(b"* 18 EXISTS\r\n"), Untagged::Exists(18));
}

#[test]
fn list_and_status() {
    let Untagged::List(entry) =
        untagged(b"* LIST (\\HasNoChildren \\Sent) \"/\" \"Enviados &AOk-\"\r\n")
    else {
        panic!()
    };
    assert_eq!(entry.special_use(), Some(SpecialUse::Sent));
    assert_eq!(entry.delimiter, Some('/'));
    assert_eq!(entry.name.as_str(), "Enviados é");
    let Untagged::List(entry) = untagged(b"* LIST (\\Noselect) NIL Archive\r\n") else {
        panic!()
    };
    assert!(!entry.is_selectable());
    assert_eq!(entry.delimiter, None);
    // Empty names are refused.
    assert!(is_unknown(b"* LIST (\\Noselect) \"/\" \"\"\r\n"));
}

#[test]
fn list_extended_data() {
    let Untagged::List(entry) =
        untagged(b"* LIST (\\Subscribed) \"/\" \"Foo\" (\"CHILDINFO\" (\"SUBSCRIBED\"))\r\n")
    else {
        panic!()
    };
    assert_eq!(entry.name.as_str(), "Foo");
    assert_eq!(entry.attributes, vec![MailboxAttribute::Subscribed]);
}

#[test]
fn status_response() {
    let Untagged::MailboxStatus(status) =
        untagged(b"* STATUS \"INBOX\" (MESSAGES 231 UIDNEXT 44292 UIDVALIDITY 1 UNSEEN 3 HIGHESTMODSEQ 7011231777)\r\n")
    else {
        panic!()
    };
    assert!(status.mailbox.is_inbox());
    assert_eq!(status.messages, Some(231));
    assert_eq!(status.uid_next.map(Uid::get), Some(44292));
    assert_eq!(status.highest_mod_seq.map(ModSeq::get), Some(7_011_231_777));
}

#[test]
fn search_results() {
    let Untagged::Search(result) =
        untagged(b"* SEARCH 2 5 6 7 11 12 18 19 20 23 (MODSEQ 917162500)\r\n")
    else {
        panic!()
    };
    assert_eq!(result.numbers.len(), 10);
    assert_eq!(result.highest_mod_seq.map(ModSeq::get), Some(917_162_500));
    let Untagged::Search(result) = untagged(b"* SEARCH\r\n") else {
        panic!()
    };
    assert!(result.numbers.is_empty());
    assert!(is_unknown(b"* SEARCH 0\r\n"));
}

#[test]
fn esearch_results() {
    let Untagged::ESearch(result) =
        untagged(b"* ESEARCH (TAG \"A0007\") UID ALL 1:3,7 MODSEQ 917 COUNT 4\r\n")
    else {
        panic!()
    };
    assert_eq!(result.tag, Some(Tag::new("A0007").unwrap()));
    assert!(result.uid);
    assert_eq!(result.count, Some(4));
    assert_eq!(result.mod_seq.map(ModSeq::get), Some(917));
    let Some(ESearchAll::Uids(all)) = result.all else {
        panic!()
    };
    assert_eq!(all.to_string(), "1:3,7");
    // Nothing found: no ALL.
    let Untagged::ESearch(result) = untagged(b"* ESEARCH (TAG \"A1\") UID\r\n") else {
        panic!()
    };
    assert!(result.all.is_none());
    // Sequence numbers, unknown return data with a list value.
    let Untagged::ESearch(result) = untagged(b"* ESEARCH ALL 2:4 PARTIAL (1:10 2:4) MIN 2\r\n")
    else {
        panic!()
    };
    assert!(!result.uid);
    assert!(matches!(result.all, Some(ESearchAll::SeqNums(_))));
    assert_eq!(result.min.map(std::num::NonZeroU32::get), Some(2));
    assert!(is_unknown(b"* ESEARCH UID ALL 1:*\r\n"));
    assert!(is_unknown(b"* ESEARCH UID ALL 0\r\n"));
    assert!(is_unknown(b"* ESEARCH UID ALL\r\n"));
}

#[test]
fn fetch_binary() {
    let data = fetch(
        b"* 3 FETCH (UID 9 BINARY.SIZE[1] 3 BINARY[1] ~{3}\r\na\0b BINARY[2]<4> \"xy\" BINARY[] NIL FLAGS (\\Seen) MODSEQ (12))\r\n",
    );
    assert_eq!(data.uid.map(Uid::get), Some(9));
    assert_eq!(data.binary_sizes, vec![(vec![1], 3)]);
    assert_eq!(data.binary.len(), 3);
    assert_eq!(data.binary[0].part, vec![1]);
    assert_eq!(data.binary[0].data.as_deref(), Some(&b"a\0b"[..]));
    assert_eq!(data.binary[1].origin, Some(4));
    assert_eq!(data.binary[2].part, Vec::<u32>::new());
    assert_eq!(data.binary[2].data, None);
    assert_eq!(data.flags, Some(vec![Flag::Seen]));
    assert_eq!(data.mod_seq.map(ModSeq::get), Some(12));
    assert!(is_unknown(b"* 3 FETCH (UID 9 BINARY[0] NIL)\r\n"));
    assert!(is_unknown(b"* 3 FETCH (UID 9 BINARY[1]<x> NIL)\r\n"));
}

#[test]
fn namespace() {
    let Untagged::Namespace(namespaces) = untagged(b"* NAMESPACE ((\"\" \"/\")) ((\"~\" \"/\")) ((\"#shared/\" \"/\")(\"#public/\" \"/\"))\r\n") else {
        panic!()
    };
    assert_eq!(
        namespaces.personal,
        vec![NamespaceEntry {
            prefix: String::new(),
            delimiter: Some('/')
        }]
    );
    assert_eq!(namespaces.shared.len(), 2);
    let Untagged::Namespace(namespaces) = untagged(b"* NAMESPACE ((\"INBOX.\" \".\")) NIL NIL\r\n")
    else {
        panic!()
    };
    assert_eq!(namespaces.personal[0].prefix, "INBOX.");
    assert!(namespaces.other_users.is_empty());
}

#[test]
fn quota() {
    let Untagged::QuotaRoot(root) = untagged(b"* QUOTAROOT INBOX \"\"\r\n") else {
        panic!()
    };
    assert_eq!(root.roots, vec![String::new()]);
    let Untagged::Quota(quota) = untagged(b"* QUOTA \"\" (STORAGE 10 512)\r\n") else {
        panic!()
    };
    assert_eq!(quota.resources[0].name, QuotaResourceName::Storage);
    assert_eq!(quota.resources[0].limit, 512);
}

#[test]
fn continuation() {
    let Response::Continuation(cont) = parse(b"+ Ready for literal data\r\n") else {
        panic!()
    };
    assert_eq!(cont.text, "Ready for literal data");
    let Response::Continuation(cont) = parse(b"+ \r\n") else {
        panic!()
    };
    assert_eq!(cont.text, "");
    let Response::Continuation(cont) = parse(b"+\r\n") else {
        panic!()
    };
    assert_eq!(cont.code, None);
}

/// Inputs on which `imap-proto` 0.17 panics or recurses without limit.
#[test]
fn hostile_inputs_do_not_panic() {
    // `INTERNALDATE NIL` hits an `unwrap` in imap-proto.
    assert!(is_unknown(b"* 1 FETCH (INTERNALDATE NIL)\r\n"));
    // Non-UTF-8 METADATA hits an `unwrap`.
    assert!(is_unknown(
        b"* METADATA \"\xff\" (/shared/comment \"x\")\r\n"
    ));
    // A multi-byte character right after a code is sliced at byte 1.
    let Untagged::Status(status) = untagged("* OK [ALERT]é\r\n".as_bytes()) else {
        panic!()
    };
    assert_eq!(status.code, Some(ResponseCode::Alert));
    let done = tagged("A1 OK [READ-ONLY]ñ done\r\n".as_bytes());
    assert_eq!(done.status, Completion::Ok);
    // Deep nesting would overflow the stack in the recursive parser.
    let mut deep = b"* 1 FETCH (BODYSTRUCTURE ".to_vec();
    deep.extend(std::iter::repeat_n(b'(', 50_000));
    deep.extend(std::iter::repeat_n(b')', 50_000));
    deep.extend_from_slice(b")\r\n");
    let limits = FrameLimits {
        max_line: 1 << 20,
        ..FrameLimits::default()
    };
    let response = parse_response(&deep, NameEncoding::ModifiedUtf7, &limits).unwrap();
    assert!(matches!(
        response,
        Response::Untagged(Untagged::Unknown(Unknown {
            reason: UnknownReason::TooDeep,
            ..
        }))
    ));
    // A tagged completion is never lost, even if it looks odd.
    let mut deep_tagged = b"A9 OK ".to_vec();
    deep_tagged.extend(std::iter::repeat_n(b'(', 1000));
    deep_tagged.extend_from_slice(b"\r\n");
    assert_eq!(tagged(&deep_tagged).status, Completion::Ok);
}

#[test]
fn decoder_streams_and_limits() {
    let mut decoder = ResponseDecoder::new(FrameLimits::default());
    let input = b"* 2 EXISTS\r\n* 1 FETCH (UID 7 BODY[] {3}\r\nabc)\r\nA1 OK done\r\n";
    for byte in input {
        decoder.feed(std::slice::from_ref(byte));
    }
    let mut responses = Vec::new();
    while let Some(response) = decoder.decode().unwrap() {
        responses.push(response);
    }
    assert_eq!(responses.len(), 3);
    assert_eq!(decoder.buffered(), 0);

    let limits = FrameLimits {
        max_literal: 10,
        ..FrameLimits::default()
    };
    let mut decoder = ResponseDecoder::new(limits);
    decoder.feed(b"* 1 FETCH (BODY[] {11}\r\n");
    assert_eq!(decoder.decode(), Ok(None));
    assert_eq!(decoder.buffered(), 0);
    decoder.feed(b"hello world)\r\nA1 OK done\r\n");
    assert!(is_too_large(&decoder.decode().unwrap().unwrap()));
    let Some(Response::Tagged(done)) = decoder.decode().unwrap() else {
        panic!()
    };
    assert_eq!(done.text, "done");
}

fn is_too_large(response: &Response) -> bool {
    matches!(
        response,
        Response::Untagged(Untagged::Unknown(Unknown {
            reason: UnknownReason::TooLarge,
            ..
        }))
    )
}

fn decode_all(decoder: &mut ResponseDecoder, input: &[u8], chunk: usize) -> (Vec<Response>, usize) {
    let mut responses = Vec::new();
    let mut largest = 0;
    for piece in input.chunks(chunk) {
        decoder.feed(piece);
        largest = largest.max(decoder.buffered());
        while let Some(response) = decoder.decode().unwrap() {
            responses.push(response);
        }
    }
    (responses, largest)
}

#[test]
fn huge_bodystructure_is_skipped_and_the_next_response_read() {
    let limits = FrameLimits {
        max_line: 1024,
        ..FrameLimits::default()
    };
    let mut input = b"* 1 FETCH (UID 7 BODYSTRUCTURE (".to_vec();
    for _ in 0..2000 {
        input.extend_from_slice(b"(\"text\" \"plain\" NIL NIL NIL \"7bit\" 0 0)");
    }
    input.extend_from_slice(b" \"mixed\"))\r\nA1 OK done\r\n");
    assert!(input.len() > 64 * 1024);
    let mut decoder = ResponseDecoder::new(limits);
    let (responses, largest) = decode_all(&mut decoder, &input, 100);
    assert!(largest <= 1024 + 100, "buffer grew to {largest}");
    let [first, Response::Tagged(done)] = responses.as_slice() else {
        panic!("{responses:?}")
    };
    assert!(is_too_large(first));
    assert_eq!(done.status, Completion::Ok);
    assert_eq!(decoder.buffered(), 0);
}

#[test]
fn long_tagged_and_continuation_lines_keep_their_kind() {
    let limits = FrameLimits {
        max_line: 1024,
        ..FrameLimits::default()
    };
    let mut input = b"A1 OK [ALERT] ".to_vec();
    input.extend(std::iter::repeat_n(b'x', 2048));
    input.extend_from_slice(b"\r\n+ ");
    input.extend(std::iter::repeat_n(b'y', 2048));
    input.extend_from_slice(b"\r\n");
    let mut decoder = ResponseDecoder::new(limits);
    let (responses, _) = decode_all(&mut decoder, &input, 333);
    let [Response::Tagged(done), Response::Continuation(more)] = responses.as_slice() else {
        panic!("{responses:?}")
    };
    assert_eq!(done.tag.as_str(), "A1");
    assert_eq!(done.status, Completion::Ok);
    assert_eq!(done.code, None);
    assert!(done.text.len() < 1024);
    assert!(more.text.starts_with("yyy"));
    // The fuzzer API agrees.
    let tagged_only = input.get(..input.len() - 2052).unwrap();
    assert!(matches!(
        parse_response(tagged_only, NameEncoding::ModifiedUtf7, &limits),
        Some(Response::Tagged(_))
    ));
}

#[test]
fn too_many_literals_are_skipped() {
    let limits = FrameLimits {
        max_literals: 2,
        ..FrameLimits::default()
    };
    let input =
        b"* 1 FETCH (A {1}\r\na B {1}\r\nb C {1}\r\nc D {1}\r\nd E {1}\r\ne)\r\n* 2 EXISTS\r\n";
    let mut decoder = ResponseDecoder::new(limits);
    let (responses, _) = decode_all(&mut decoder, input, 5);
    let [first, Response::Untagged(Untagged::Exists(2))] = responses.as_slice() else {
        panic!("{responses:?}")
    };
    assert!(is_too_large(first));
}

#[test]
fn utf8_mailbox_names() {
    let response = parse_response(
        "* LIST () \"/\" \"Enviados é\"\r\n".as_bytes(),
        NameEncoding::Utf8,
        &FrameLimits::default(),
    );
    let Some(Response::Untagged(Untagged::List(entry))) = response else {
        panic!()
    };
    assert_eq!(entry.name.as_str(), "Enviados é");
}

/// Pieces of real responses, glued at random: reaches deeper into the
/// parser than random bytes.
fn fragment() -> impl Strategy<Value = &'static [u8]> {
    prop::sample::select(vec![
        &b"* "[..],
        b"A1 ",
        b"+ ",
        b"OK ",
        b"NO ",
        b"BAD ",
        b"BYE ",
        b"PREAUTH ",
        b"[",
        b"]",
        b"(",
        b")",
        b"\"",
        b"\\",
        b"{3}\r\n",
        b"{0}\r\n",
        b"\r\n",
        b" ",
        b"NIL",
        b"0",
        b"1",
        b"4294967296",
        b"FETCH",
        b"UID",
        b"FLAGS",
        b"BODY[",
        b"BODYSTRUCTURE",
        b"INTERNALDATE",
        b"PREVIEW",
        b"EMAILID",
        b"THREADID",
        b"MODSEQ",
        b"LIST",
        b"STATUS",
        b"SEARCH",
        b"NAMESPACE",
        b"VANISHED",
        b"(EARLIER)",
        b"METADATA",
        b"CAPABILITY",
        b"ENABLED",
        b"COPYUID",
        b"APPENDUID",
        b"HIGHESTMODSEQ",
        b"\xc3\xa9",
        b"\xff",
        b"\\Seen",
        b"QUOTA",
        b"QUOTAROOT",
        b"ID",
        b"EXISTS",
        b"EXPUNGE",
        b"ALERT",
        b"X-GM-LABELS",
        b"HEADER.FIELDS",
        b"ESEARCH",
        b"(TAG \"A1\")",
        b"ALL",
        b"BINARY[",
        b"BINARY.SIZE[",
        b"~{3}\r\n",
    ])
}

/// Pieces to build FETCH frames that try to hide depth from our checks:
/// mostly valid items (so `imap-proto` gets far into the frame), values
/// whose literal starts with `)` and holds a deep structure (the known way
/// to make the parsers disagree), and some noise.
fn deep_piece() -> impl Strategy<Value = Vec<u8>> {
    let valid = prop::sample::select(vec![
        &b"UID 1 "[..],
        b"FLAGS (\\Seen) ",
        b"X NIL ",
        b"X (a b) ",
        b"X \"a\" ",
        b"PREVIEW \"p\" ",
        b"BODY[] NIL ",
        b"BODYSTRUCTURE (\"A\" \"B\" NIL NIL NIL \"7BIT\" 0) ",
    ])
    .prop_map(<[u8]>::to_vec);
    let noise = prop::sample::select(vec![
        &b" "[..],
        b"(",
        b")",
        b"\"",
        b"\")\"",
        b"[",
        b"]",
        b"((((((((((",
        b"))))))))))",
    ])
    .prop_map(<[u8]>::to_vec);
    let literal = |content: Vec<u8>| {
        let mut literal = format!("{{{}}}\r\n", content.len()).into_bytes();
        literal.extend_from_slice(&content);
        literal
    };
    let names = prop::sample::select(vec![
        "X",
        "FLAGS",
        "BODYSTRUCTURE",
        "OBJECTID",
        "PREVIEW",
        "UID",
        "BODY[]",
        "X-GM-LABELS",
        "ENVELOPE",
        "MODSEQ",
        "EMAILID",
        "THREADID",
        "BINARY[1]",
    ]);
    let hidden = (names, 0usize..3, 0usize..3, prop::bool::ANY).prop_map(
        move |(name, before, after, quoted)| {
            let mut content =
                b") BODYSTRUCTURE (\"A\" \"B\" NIL NIL NIL \"7BIT\" 0 NIL NIL NIL NIL ".to_vec();
            content.extend(std::iter::repeat_n(b'(', 5000));
            content.extend(std::iter::repeat_n(b')', 5000));
            let mut piece = format!("{name} ").into_bytes();
            piece.extend(std::iter::repeat_n(b'(', before));
            if quoted {
                piece.extend_from_slice(b"\")\" ");
            }
            piece.extend(literal(content));
            piece.extend(std::iter::repeat_n(b')', after));
            piece.push(b' ');
            piece
        },
    );
    prop_oneof![4 => valid, 2 => hidden, 1 => noise]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Differential check of the guards against `imap-proto`: with a small
    /// depth limit, whatever passes them parses within a small stack. A
    /// way to hide depth from the guards would make `imap-proto` recurse
    /// and overflow the stack (which aborts the test run).
    #[test]
    fn guards_bound_imap_proto_depth(parts in proptest::collection::vec(deep_piece(), 0..8)) {
        let mut frame = b"* 1 FETCH (".to_vec();
        frame.extend_from_slice(&parts.concat());
        frame.extend_from_slice(b")\r\n");
        let limits = FrameLimits {
            max_depth: 4,
            ..FrameLimits::default()
        };
        let _ = parse_on_small_stack(frame, limits, 1024 * 1024);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn decoder_never_panics(input in proptest::collection::vec(any::<u8>(), 0..300)) {
        let mut decoder = ResponseDecoder::new(FrameLimits::default());
        decoder.feed(&input);
        while let Ok(Some(_)) = decoder.decode() {}
    }

    #[test]
    fn decoder_never_panics_on_fragments(parts in proptest::collection::vec(fragment(), 0..40)) {
        let mut decoder = ResponseDecoder::new(FrameLimits::default());
        decoder.feed(&parts.concat());
        decoder.feed(b"\r\n");
        while let Ok(Some(_)) = decoder.decode() {}
    }
}

/// At the default depth limit, the recursive parser must fit in a small
/// stack (tokio worker threads have 2 MiB).
#[test]
fn depth_limit_fits_small_stack() {
    let leaf = "(\"TEXT\" \"PLAIN\" NIL NIL NIL \"7BIT\" 1 1)";
    let mut body = leaf.to_owned();
    // Each multipart level adds one level of parentheses.
    let levels = FrameLimits::default().max_depth - 2;
    for _ in 0..levels {
        body = format!("({body} \"MIXED\")");
    }
    let input = format!("* 1 FETCH (UID 1 BODYSTRUCTURE {body})\r\n");
    let handle = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            parse_response(
                input.as_bytes(),
                NameEncoding::ModifiedUtf7,
                &FrameLimits::default(),
            )
        })
        .unwrap();
    let response = handle.join().unwrap();
    assert!(
        matches!(response, Some(Response::Untagged(Untagged::Fetch(_)))),
        "{response:?}"
    );
}
