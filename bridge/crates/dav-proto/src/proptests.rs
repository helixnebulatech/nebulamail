// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Properties of the parsers and writers on random input.

use proptest::prelude::*;
use url::Url;

use crate::request::{self, PropValue};
use crate::xml::{check_markup, is, parse_document};
use crate::{CTag, ETag, Href, Namespace, ResourceName, SyncToken, XmlLimits, parse_multistatus};

fn base() -> Href {
    Href::from_url(&Url::parse("https://dav.example.com/dav/cal/alice/").unwrap()).unwrap()
}

/// Text from the characters that matter in hrefs.
fn href_text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just("/".to_owned()),
            Just(".".to_owned()),
            Just("..".to_owned()),
            Just("%".to_owned()),
            Just("%2e".to_owned()),
            Just("%2F".to_owned()),
            Just("%5c".to_owned()),
            Just("%00".to_owned()),
            Just("%C3%A9".to_owned()),
            Just("https://dav.example.com".to_owned()),
            Just("http://evil.example".to_owned()),
            Just(":".to_owned()),
            Just("@".to_owned()),
            "[a-zA-Z0-9 _~!$&'()*+,;=-]{1,4}",
            any::<char>().prop_map(String::from),
        ],
        0..24,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Whatever the text, an accepted href is canonical: no dot or empty
    /// segment, ASCII only, and reading its own path gives it back.
    #[test]
    fn hrefs_are_canonical(text in href_text()) {
        let base = base();
        if let Ok(href) = Href::resolve(&text, &base) {
            let path = href.path();
            prop_assert!(path.starts_with('/'));
            prop_assert!(path.is_ascii());
            prop_assert!(!path.contains("//"));
            for segment in path.split('/') {
                let lower = segment.to_ascii_lowercase();
                prop_assert!(lower != "." && lower != ".." && lower != "%2e" && lower != "%2e%2e");
                prop_assert!(!lower.contains("%2f") && !lower.contains("%5c"));
            }
            let again = Href::resolve(path, &href).unwrap();
            prop_assert_eq!(&again, &href);
            let absolute = Href::resolve(href.url().as_str(), &base).unwrap();
            prop_assert_eq!(&absolute, &href);
            for segment in href.segments() {
                prop_assert!(!segment.is_empty() && !segment.contains(['/', '\\']));
                prop_assert!(!segment.chars().any(char::is_control));
            }
        }
    }

    /// A member never escapes its collection, whatever the name.
    #[test]
    fn members_stay_inside(name in "\\PC{1,40}") {
        let collection = Href::resolve("work/", &base()).unwrap();
        if let Ok(name) = ResourceName::try_from(name.as_str()) {
            let member = collection.member(&name, false).unwrap();
            prop_assert!(member.is_member_of(&collection));
            prop_assert_eq!(member.last_segment(), Some(name.as_str()));
            let back = Href::resolve(member.path(), &collection).unwrap();
            prop_assert_eq!(back, member);
        }
    }

    /// Text written into a request reads back the same (escaping is
    /// complete), or is refused if XML cannot carry it.
    #[test]
    fn written_text_reads_back(text in "\\PC*|[\\x00-\\x7f]*") {
        if let Ok(body) = request::proppatch(&[PropValue::DisplayName(text.clone())], &[]) {
                let document = parse_document(body.as_bytes(), &XmlLimits::default()).unwrap();
                let name = document
                    .descendants()
                    .find(|n| is(*n, Namespace::Dav, "displayname"))
                    .unwrap();
                let read: String = name.children().filter_map(|c| c.text()).collect();
                prop_assert_eq!(read, text);
        } else {
                let unsafe_char = |c: char| {
                    (c < ' ' && !matches!(c, '\t' | '\n' | '\r')) || matches!(c, '\u{fffe}' | '\u{ffff}')
                };
                prop_assert!(text.chars().any(unsafe_char));
        }
    }

    /// Opaque values that parse are safe in a header line.
    #[test]
    fn opaque_values_are_header_safe(text in "\\PC*|[\\x00-\\x7f]*") {
        let safe = |s: &str| s.bytes().all(|b| (0x20..0x7f).contains(&b));
        if let Some(etag) = ETag::parse(&text) {
            prop_assert!(safe(etag.as_str()));
        }
        if let Some(token) = SyncToken::parse(&text) {
            prop_assert!(safe(token.as_str()));
        }
        if let Some(ctag) = CTag::parse(&text) {
            prop_assert!(safe(ctag.as_str()));
        }
    }

    /// The parser never panics, whatever the bytes.
    #[test]
    fn multistatus_never_panics(body in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let base = Url::parse("https://dav.example.com/dav/").unwrap();
        let _ = parse_multistatus(&body, &base, &XmlLimits::default());
        let _ = crate::parse_error_body(&body);
    }

    /// Nor on XML-like text built from the elements it knows.
    #[test]
    fn multistatus_never_panics_on_xmlish(parts in proptest::collection::vec(prop_oneof![
        Just("<D:multistatus xmlns:D=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">"),
        Just("</D:multistatus>"),
        Just("<D:response>"), Just("</D:response>"),
        Just("<D:href>"), Just("</D:href>"), Just("/dav/a/"), Just("../x"),
        Just("<D:propstat>"), Just("</D:propstat>"),
        Just("<D:prop>"), Just("</D:prop>"),
        Just("<D:status>HTTP/1.1 200 OK</D:status>"),
        Just("<D:status>HTTP/1.1 404 x</D:status>"),
        Just("<D:getetag>\"x\"</D:getetag>"),
        Just("<D:resourcetype><D:collection/><C:calendar/></D:resourcetype>"),
        Just("<C:calendar-data>"), Just("</C:calendar-data>"),
        Just("<D:sync-token>t</D:sync-token>"),
        Just("<D:error><D:valid-sync-token/></D:error>"),
        Just("<![CDATA[x]]>"), Just("&amp;"), Just("&#0;"), Just("<!DOCTYPE x>"),
    ], 0..40)) {
        let body = parts.concat();
        let base = Url::parse("https://dav.example.com/dav/").unwrap();
        let _ = parse_multistatus(body.as_bytes(), &base, &XmlLimits::default());
    }

    /// The pre-pass before roxmltree only sets limits: well-formed XML
    /// within them (attributes, namespaces, quotes holding `>` or `/>`,
    /// markup-like text in comments, CDATA and instructions) passes it.
    #[test]
    fn markup_check_accepts_what_is_within_the_limits(
        parts in proptest::collection::vec(0_u8..9, 0..30),
    ) {
        let mut body = String::from(r#"<r xmlns="DAV:" a='1'>"#);
        let mut open: Vec<String> = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            let piece = match part {
                0 => {
                    open.push(format!("e{i}"));
                    format!(r#"<e{i} x="a>b" y='/>' xmlns:p{i}="u">"#)
                }
                1 => open.pop().map_or_else(String::new, |name| format!("</{name}>")),
                2 => format!("<p{i}:f xmlns:p{i}=\"v\" k = \"1\" />"),
                3 => "<!-- <a b=\"1\" c='2'> = < -->".to_owned(),
                4 => "<![CDATA[<a b=\"1\"> ]] = ]]>".to_owned(),
                5 => "<?pi <a b='1'> ?>".to_owned(),
                6 => "a=b &lt;c d=e&gt;".to_owned(),
                7 => "<g/>".to_owned(),
                _ => "<h></h>".to_owned(),
            };
            body.push_str(&piece);
        }
        while let Some(name) = open.pop() {
            body.push_str("</");
            body.push_str(&name);
            body.push('>');
        }
        body.push_str("</r>");
        prop_assert!(roxmltree::Document::parse(&body).is_ok(), "{}", body);
        prop_assert_eq!(check_markup(&body, &XmlLimits::default()), Ok(()));
    }
}
