// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fmt::Write as _;

use url::Url;

use super::parse_multistatus;
use crate::{
    Component, HrefError, Multistatus, ParseError, Precondition, Privilege, Property, Report,
    Status, XmlLimits,
};

fn parse_at(base: &str, body: &str) -> Result<Multistatus, ParseError> {
    parse_multistatus(
        body.as_bytes(),
        &Url::parse(base).unwrap(),
        &XmlLimits::default(),
    )
}

fn parse(body: &str) -> Result<Multistatus, ParseError> {
    parse_at("https://dav.example.com/dav/cal/alice/", body)
}

/// A Depth 1 PROPFIND on a calendar home, as Fastmail (Cyrus) answers in
/// libdav's tests, with every collection property we ask for.
#[test]
fn calendar_home_listing() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:CS="http://calendarserver.org/ns/" xmlns:A="http://apple.com/ns/ical/">
  <response>
    <href>/dav/cal/alice/</href>
    <propstat>
      <prop><resourcetype><collection/></resourcetype></prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
    <propstat>
      <prop><displayname/><CS:getctag/></prop>
      <status>HTTP/1.1 404 Not Found</status>
    </propstat>
  </response>
  <response>
    <href>/dav/cal/alice/cc396171-0227-4e1c-b5ee-d42b5e17d533/</href>
    <propstat>
      <prop>
        <resourcetype><collection/><C:calendar/></resourcetype>
        <displayname>Work &amp; more</displayname>
        <A:calendar-color>#1E90FFFF</A:calendar-color>
        <A:calendar-order>3</A:calendar-order>
        <C:supported-calendar-component-set><C:comp name="VEVENT"/><C:comp name="VTODO"/><C:comp name="VPOLL"/></C:supported-calendar-component-set>
        <current-user-privilege-set><privilege><read/></privilege><privilege><write-content/></privilege><privilege><bind/></privilege></current-user-privilege-set>
        <sync-token>https://dav.example.com/ns/sync/1234</sync-token>
        <CS:getctag>"ctag-1"</CS:getctag>
        <supported-report-set>
          <supported-report><report><sync-collection/></report></supported-report>
          <supported-report><report><C:calendar-multiget/></report></supported-report>
        </supported-report-set>
        <C:calendar-timezone><![CDATA[BEGIN:VCALENDAR
BEGIN:VTIMEZONE
TZID:Europe/Madrid
END:VTIMEZONE
END:VCALENDAR
]]></C:calendar-timezone>
      </prop>
      <status>HTTP/1.1 200 OK</status>
    </propstat>
  </response>
</multistatus>"#;
    let multistatus = parse(body).unwrap();
    let [home, work] = multistatus.responses.as_slice() else {
        panic!("two responses");
    };
    assert_eq!(home.href().path(), "/dav/cal/alice/");
    assert_eq!(
        home.status_of(Property::DisplayName),
        Some(Status::NOT_FOUND)
    );
    assert_eq!(home.status_of(Property::GetCTag), Some(Status::NOT_FOUND));
    assert!(home.properties.resource_type.unwrap().collection);

    let p = &work.properties;
    assert!(work.href().is_member_of(home.href()));
    let kind = p.resource_type.unwrap();
    assert!(kind.calendar && kind.collection && !kind.addressbook);
    assert_eq!(p.display_name.as_deref(), Some("Work & more"));
    assert_eq!(p.calendar_color.unwrap().to_string(), "#1E90FFFF");
    assert_eq!(p.calendar_order, Some(3));
    let components = p.supported_components.as_ref().unwrap();
    assert!(components.contains(Component::Event) && components.contains(Component::Todo));
    assert_eq!(components.components().len(), 2);
    let privileges = p.privileges.unwrap();
    assert!(privileges.can_write_content() && privileges.can_create());
    assert!(!privileges.can_delete());
    assert!(!privileges.allows(Privilege::WriteProperties));
    assert_eq!(
        p.sync_token.as_ref().unwrap().as_str(),
        "https://dav.example.com/ns/sync/1234"
    );
    assert_eq!(p.ctag.as_ref().unwrap().as_str(), "\"ctag-1\"");
    let reports = p.supported_reports.unwrap();
    assert!(reports.contains(Report::SyncCollection) && reports.contains(Report::CalendarMultiget));
    assert!(!reports.contains(Report::AddressbookMultiget));
    assert_eq!(
        p.calendar_timezone.as_deref(),
        Some(
            "BEGIN:VCALENDAR\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Madrid\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n"
        )
    );
}

/// Discovery: the principal's properties, prefixes of every kind
/// (Radicale and Stalwart write `D:`, `SabreDAV` `d:`, iCloud a default
/// namespace), relative hrefs and `mailto:` addresses.
#[test]
fn principal_properties() {
    let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/dav/principals/alice/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal><d:href>/dav/principals/alice/</d:href></d:current-user-principal>
        <cal:calendar-home-set><d:href>/dav/cal/alice/</d:href></cal:calendar-home-set>
        <card:addressbook-home-set><d:href>../../card/alice/</d:href></card:addressbook-home-set>
        <cal:calendar-user-address-set>
          <d:href>mailto:alice@example.com</d:href>
          <d:href>/dav/principals/alice/</d:href>
          <d:href>mail to:bad</d:href>
        </cal:calendar-user-address-set>
        <cal:schedule-outbox-URL><d:href>/dav/cal/alice/outbox/</d:href></cal:schedule-outbox-URL>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"#;
    // `../../card/alice/` is refused (dot segments are never resolved).
    assert_eq!(
        parse_at("https://dav.example.com/dav/principals/alice/", body),
        Err(ParseError::Href(HrefError::Segment))
    );
    let body = body.replace("../../card/alice/", "/dav/card/alice/");
    let multistatus = parse_at("https://dav.example.com/dav/principals/alice/", &body).unwrap();
    let p = &multistatus.responses[0].properties;
    assert_eq!(
        p.current_user_principal.as_ref().unwrap().path(),
        "/dav/principals/alice/"
    );
    assert_eq!(p.calendar_home_set[0].path(), "/dav/cal/alice/");
    assert_eq!(p.addressbook_home_set[0].path(), "/dav/card/alice/");
    assert_eq!(
        p.calendar_user_address_set,
        ["mailto:alice@example.com", "/dav/principals/alice/"]
    );
    assert_eq!(
        p.schedule_outbox_url.as_ref().unwrap().path(),
        "/dav/cal/alice/outbox/"
    );
}

/// Stalwart 0.16.27's principal listing (PROPFIND Depth 1 on
/// `principal-collection-set`), for the directory of `Principal/query`.
#[test]
fn principal_listing() {
    let body = r#"<?xml version="1.0" encoding="UTF-8"?><D:multistatus xmlns:D="DAV:" xmlns:A="urn:ietf:params:xml:ns:caldav"><D:response><D:href>/dav/pal/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response><D:response><D:href>/dav/pal/bob%40nebulamail.test/</D:href><D:propstat><D:prop><D:displayname>Test account</D:displayname><D:resourcetype><D:principal/><D:collection/></D:resourcetype><A:calendar-user-address-set><D:href>mailto:bob@nebulamail.test</D:href></A:calendar-user-address-set><A:calendar-user-type>INDIVIDUAL</A:calendar-user-type><D:principal-collection-set><D:href>/dav/pal/</D:href></D:principal-collection-set></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
    let multistatus = parse_at("http://127.0.0.1:8080/dav/pal/", body).unwrap();
    let bob = &multistatus.responses[1];
    assert_eq!(bob.href().path(), "/dav/pal/bob@nebulamail.test/");
    let p = &bob.properties;
    assert!(p.resource_type.unwrap().principal);
    assert_eq!(p.display_name.as_deref(), Some("Test account"));
    assert_eq!(p.calendar_user_address_set, ["mailto:bob@nebulamail.test"]);
    assert_eq!(
        p.calendar_user_type,
        Some(crate::CalendarUserType::Individual)
    );
    assert_eq!(p.principal_collection_set[0].path(), "/dav/pal/");
    // A principal collection on another host is parsed (the client checks
    // the origin), one with dot segments fails the body.
    let elsewhere = body.replace(
        "<D:principal-collection-set><D:href>/dav/pal/",
        "<D:principal-collection-set><D:href>https://evil.example/pal/",
    );
    let parsed = parse_at("http://127.0.0.1:8080/dav/pal/", &elsewhere).unwrap();
    let set = &parsed.responses[1].properties.principal_collection_set;
    assert_eq!(set[0].url().host_str(), Some("evil.example"));
    let dots = body.replace(
        "<D:principal-collection-set><D:href>/dav/pal/",
        "<D:principal-collection-set><D:href>/dav/pal/../../",
    );
    assert_eq!(
        parse_at("http://127.0.0.1:8080/dav/pal/", &dots),
        Err(ParseError::Href(HrefError::Segment))
    );
}

#[test]
fn unauthenticated_principal() {
    let body = r#"<multistatus xmlns="DAV:"><response><href>/</href><propstat>
        <prop><current-user-principal><unauthenticated/></current-user-principal></prop>
        <status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    let multistatus = parse(body).unwrap();
    let response = &multistatus.responses[0];
    assert!(response.properties.has(Property::CurrentUserPrincipal));
    assert_eq!(response.properties.current_user_principal, None);
}

/// A sync-collection answer (RFC 6578 §6.2): changed members with etags,
/// a deleted one with 404, the truncation marker (507 on the collection)
/// and the new token at the end.
#[test]
fn sync_collection_answer() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/cal/alice/work/1.ics</D:href>
    <D:propstat><D:prop><D:getetag>"00001-abcd1"</D:getetag></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/cal/alice/work/2.ics</D:href>
    <D:status>HTTP/1.1 404 Not Found</D:status>
  </D:response>
  <D:response>
    <D:href>/dav/cal/alice/work/3.ics</D:href>
    <D:propstat><D:prop><D:getetag/></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/cal/alice/work/</D:href>
    <D:status>HTTP/1.1 507 Insufficient Storage</D:status>
    <D:error><D:number-of-matches-within-limits/></D:error>
  </D:response>
  <D:sync-token>http://example.com/ns/sync/1235</D:sync-token>
</D:multistatus>"#;
    let multistatus = parse(body).unwrap();
    let [changed, deleted, no_etag, truncated] = multistatus.responses.as_slice() else {
        panic!("four responses");
    };
    assert_eq!(
        changed.properties.etag.as_ref().unwrap().as_str(),
        "\"00001-abcd1\""
    );
    assert_eq!(deleted.status, Some(Status::NOT_FOUND));
    // An empty getetag is "present but unknown": the caller fetches it.
    assert!(no_etag.properties.has(Property::GetETag));
    assert_eq!(no_etag.properties.etag, None);
    assert_eq!(truncated.status, Some(Status::INSUFFICIENT_STORAGE));
    assert_eq!(
        truncated.precondition,
        Some(Precondition::NumberOfMatchesWithinLimits)
    );
    assert_eq!(
        multistatus.sync_token.unwrap().as_str(),
        "http://example.com/ns/sync/1235"
    );
}

/// calendar-data without CR (roxmltree drops them, and some servers never
/// send them) comes back with CRLF, as in libdav's
/// `get_newline_corrected_text` tests.
#[test]
fn multiget_data_gets_crlf() {
    let body = "<ns0:multistatus xmlns:ns0=\"DAV:\" xmlns:ns1=\"urn:ietf:params:xml:ns:caldav\"><ns0:response><ns0:href>/dav/cal/alice/1.ics</ns0:href><ns0:propstat><ns0:status>HTTP/1.1 200 OK</ns0:status><ns0:prop><ns0:getetag>\"2d2c\"</ns0:getetag><ns1:calendar-data>BEGIN:VCALENDAR\nSUMMARY:hello\\, testing\nEND:VCALENDAR\n</ns1:calendar-data></ns0:prop></ns0:propstat></ns0:response></ns0:multistatus>";
    let multistatus = parse(body).unwrap();
    assert_eq!(
        multistatus.responses[0].properties.calendar_data.as_deref(),
        Some("BEGIN:VCALENDAR\r\nSUMMARY:hello\\, testing\r\nEND:VCALENDAR\r\n")
    );
    // Escaped CRs survive as they are.
    let escaped = body.replace("BEGIN:VCALENDAR\n", "BEGIN:VCALENDAR&#13;\n");
    assert_eq!(
        parse(&escaped).unwrap().responses[0]
            .properties
            .calendar_data
            .as_deref()
            .unwrap()
            .lines()
            .next(),
        Some("BEGIN:VCALENDAR")
    );
}

#[test]
fn multistatus_with_several_hrefs_and_one_status() {
    let body = r#"<multistatus xmlns="DAV:"><response>
        <href>/dav/cal/alice/a.ics</href><href>/dav/cal/alice/b.ics</href>
        <status>HTTP/1.1 404 Not Found</status></response></multistatus>"#;
    let multistatus = parse(body).unwrap();
    let response = &multistatus.responses[0];
    assert_eq!(response.hrefs().count(), 2);
    let bad = r#"<multistatus xmlns="DAV:"><response>
        <href>/a</href><href>/b</href>
        <propstat><prop/><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    assert!(matches!(parse(bad), Err(ParseError::Invalid(_))));
}

/// Cyrus answers PROPPATCH in the wrong namespace (cyrus-imapd#4489): the
/// status still counts. Values in a wrong namespace never do.
#[test]
fn cyrus_namespace_quirk_only_for_statuses() {
    let body = r#"<multistatus xmlns="DAV:"><response><href>/dav/cal/alice/work/</href>
        <propstat><prop><calendar-color xmlns="DAV:"/></prop><status>HTTP/1.1 403 Forbidden</status></propstat>
        <propstat><prop><calendar-order xmlns="DAV:">5</calendar-order></prop><status>HTTP/1.1 200 OK</status></propstat>
        </response></multistatus>"#;
    let response = &parse(body).unwrap().responses[0];
    assert_eq!(
        response.status_of(Property::CalendarColor),
        Status::new(403)
    );
    assert_eq!(response.properties.calendar_order, None);
    assert_eq!(response.status_of(Property::CalendarOrder), None);
}

#[test]
fn structure_is_checked() {
    let cases = [
        r#"<propfind xmlns="DAV:"/>"#,
        r"<multistatus/>",
        r#"<multistatus xmlns="urn:not-dav"/>"#,
        r#"<multistatus xmlns="DAV:"><response><status>HTTP/1.1 200 OK</status></response></multistatus>"#,
        r#"<multistatus xmlns="DAV:"><response><href>/a</href><status>200 OK</status></response></multistatus>"#,
        r#"<multistatus xmlns="DAV:"><response><href>/a</href><propstat><prop/></propstat></response></multistatus>"#,
        r#"<multistatus xmlns="DAV:"><response><href><b/></href></response></multistatus>"#,
    ];
    for body in cases {
        assert!(matches!(parse(body), Err(ParseError::Invalid(_))), "{body}");
    }
    // A prefix bound to the wrong URI is not DAV:, whatever its name.
    let fake = r#"<D:multistatus xmlns:D="urn:evil"><D:response><D:href>/a</D:href></D:response></D:multistatus>"#;
    assert!(matches!(parse(fake), Err(ParseError::Invalid(_))));
    // An empty multistatus is fine.
    assert_eq!(
        parse(r#"<multistatus xmlns="DAV:"/>"#).unwrap().responses,
        []
    );
}

#[test]
fn hostile_hrefs_drop_their_response() {
    for href in [
        "/dav/cal/alice/%2e%2e/bob/x.ics",
        "/dav/cal/alice/../bob/x.ics",
        "/dav/cal/alice/a%2Fb.ics",
        "//evil.example/x",
        "javascript:alert(1)",
        "/dav/x%0d%0aHost:%20evil",
        "/dav/cal/alice/a%5Cb.ics",
        "/dav/cal/alice/%FF.ics",
    ] {
        let body = format!(
            r#"<multistatus xmlns="DAV:"><response><href>{href}</href><status>HTTP/1.1 404 Not Found</status></response><response><href>/dav/cal/alice/ok.ics</href><propstat><prop><getetag>"1"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#
        );
        let multistatus = parse(&body).unwrap();
        assert_eq!(multistatus.refused, 1, "{href}");
        assert_eq!(multistatus.responses.len(), 1, "{href}");
        assert_eq!(
            multistatus.responses[0].href().path(),
            "/dav/cal/alice/ok.ics"
        );
    }
    // One bad href among several of a shared status drops that response.
    let body = r#"<multistatus xmlns="DAV:"><response><href>/dav/cal/alice/a.ics</href><href>/dav/cal/alice/%2e%2e/b.ics</href><status>HTTP/1.1 404 Not Found</status></response></multistatus>"#;
    let multistatus = parse(body).unwrap();
    assert_eq!((multistatus.responses.len(), multistatus.refused), (0, 1));
    // An href too long for one is dropped too; other errors still fail.
    let long = format!(
        r#"<multistatus xmlns="DAV:"><response><href>/{}</href><status>HTTP/1.1 404 Not Found</status></response></multistatus>"#,
        "a".repeat(crate::Href::MAX_LEN + 100)
    );
    assert_eq!(parse(&long).unwrap().refused, 1);
    let elements = r#"<multistatus xmlns="DAV:"><response><href><x/></href><status>HTTP/1.1 404 Not Found</status></response></multistatus>"#;
    assert!(matches!(parse(elements), Err(ParseError::Invalid(_))));
}

/// XXE, billion laughs, a DTD with nothing in it: all refused before any
/// entity is looked at.
#[test]
fn dtds_and_entities() {
    let xxe = r#"<?xml version="1.0"?>
<!DOCTYPE d [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
<multistatus xmlns="DAV:"><response><href>/a</href><propstat><prop><displayname>&xxe;</displayname></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    assert_eq!(parse(xxe), Err(ParseError::Dtd));
    let laughs = format!(
        "<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\">{}]><multistatus xmlns=\"DAV:\">&lol9;</multistatus>",
        (1..10).fold(String::new(), |mut out, i| {
            let previous = if i == 1 {
                "lol".to_owned()
            } else {
                format!("lol{}", i - 1)
            };
            let _ = write!(
                out,
                "<!ENTITY lol{i} \"{}\">",
                format!("&{previous};").repeat(10)
            );
            out
        })
    );
    assert_eq!(parse(&laughs), Err(ParseError::Dtd));
    assert_eq!(
        parse(r#"<!DOCTYPE multistatus><multistatus xmlns="DAV:"/>"#),
        Err(ParseError::Dtd)
    );
    // Predefined and numeric entities are XML itself, not a DTD.
    let ok = r#"<multistatus xmlns="DAV:"><response><href>/a</href><propstat><prop><displayname>&lt;&#65;&#x42;&gt;</displayname></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    assert_eq!(
        parse(ok).unwrap().responses[0]
            .properties
            .display_name
            .as_deref(),
        Some("<AB>")
    );
}

#[test]
fn deep_nesting_inside_a_property() {
    let deep = format!(
        r#"<multistatus xmlns="DAV:"><response><href>/a</href><propstat><prop><resourcetype>{}{}</resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#,
        "<x>".repeat(100),
        "</x>".repeat(100)
    );
    assert_eq!(parse(&deep), Err(ParseError::TooDeep));
}

#[test]
fn value_limits() {
    let limits = XmlLimits {
        max_value_bytes: 10,
        ..XmlLimits::default()
    };
    let body = r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><response><href>/a</href><propstat><prop><D:getetag xmlns:D="DAV:">"1"</D:getetag><C:calendar-data>BEGIN:VCALENDAR</C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response><response><href>/b</href><propstat><prop><C:calendar-data>BEGIN</C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    let base = Url::parse("https://dav.example.com/").unwrap();
    // Too large for one resource: 507 for it, the rest of the body is read.
    let multistatus = parse_multistatus(body.as_bytes(), &base, &limits).unwrap();
    let [large, small] = multistatus.responses.as_slice() else {
        panic!("two responses");
    };
    assert_eq!(large.properties.calendar_data, None);
    assert!(large.properties.etag.is_some());
    assert_eq!(
        large.status_of(Property::CalendarData),
        Some(Status::INSUFFICIENT_STORAGE)
    );
    assert_eq!(small.properties.calendar_data.as_deref(), Some("BEGIN"));
    assert_eq!(small.status_of(Property::CalendarData), Some(Status::OK));
    for (property, element) in [
        (Property::AddressData, "R:address-data"),
        (Property::CalendarTimezone, "C:calendar-timezone"),
    ] {
        let body = format!(
            r#"<multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:R="urn:ietf:params:xml:ns:carddav"><response><href>/a</href><propstat><prop><{element}>BEGIN:VCARD-LONG</{element}></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#
        );
        let multistatus = parse_multistatus(body.as_bytes(), &base, &limits).unwrap();
        assert_eq!(
            multistatus.responses[0].status_of(property),
            Some(Status::INSUFFICIENT_STORAGE)
        );
    }
    // A display name that is too long or has control characters is dropped.
    let long = format!(
        r#"<multistatus xmlns="DAV:"><response><href>/a</href><propstat><prop><displayname>{}</displayname></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#,
        "a".repeat(5000)
    );
    assert_eq!(
        parse(&long).unwrap().responses[0].properties.display_name,
        None
    );
    let control = r#"<multistatus xmlns="DAV:"><response><href>/a</href><propstat><prop><displayname>a&#133;b</displayname></prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    assert_eq!(
        parse(control).unwrap().responses[0].properties.display_name,
        None
    );
}

#[test]
fn bad_values_are_absent() {
    let body = r#"<multistatus xmlns="DAV:" xmlns:A="http://apple.com/ns/ical/"><response><href>/a/</href><propstat><prop>
        <getetag>"a" "b"</getetag><A:calendar-color>red</A:calendar-color><A:calendar-order>x</A:calendar-order>
        <sync-token>a&#10;b</sync-token>
        </prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
    let p = &parse(body).unwrap().responses[0].properties;
    assert_eq!(p.etag, None);
    assert_eq!(p.calendar_color, None);
    assert_eq!(p.calendar_order, None);
    assert_eq!(p.sync_token, None);
    assert!(p.has(Property::GetETag));
}

#[test]
fn addressbook_properties() {
    let body = r#"<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/dav/card/alice/default/</d:href><d:propstat><d:prop>
        <d:resourcetype><d:collection/><card:addressbook/></d:resourcetype>
        <card:supported-address-data><card:address-data-type content-type="text/vcard" version="4.0"/><card:address-data-type/></card:supported-address-data>
        <card:addressbook-description>Mis contactos</card:addressbook-description>
        </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    let p = &parse(body).unwrap().responses[0].properties;
    assert!(p.resource_type.unwrap().addressbook);
    let types = p.supported_address_data.as_ref().unwrap();
    assert_eq!(types[0].version, "4.0");
    assert_eq!(types[1].content_type, "text/vcard");
    assert_eq!(types[1].version, "3.0");
    assert_eq!(p.addressbook_description.as_deref(), Some("Mis contactos"));
}

/// Time zone and default alarms as Stalwart (default namespace on the
/// alarm elements), Radicale (an empty element) and sabre/dav answer them;
/// a zone id with spaces and an oversized alarm value are left out, not
/// errors.
#[test]
fn calendar_defaults() {
    let body = r#"<?xml version="1.0" encoding="UTF-8"?><D:multistatus xmlns:D="DAV:" xmlns:A="urn:ietf:params:xml:ns:caldav"><D:response><D:href>/dav/cal/a/w/</D:href><D:propstat><D:prop><A:calendar-timezone-id>Europe/Madrid</A:calendar-timezone-id><default-alarm-vevent-datetime xmlns="urn:ietf:params:xml:ns:caldav">BEGIN:VALARM
ACTION:DISPLAY
TRIGGER:-PT15M
END:VALARM
</default-alarm-vevent-datetime><A:default-alarm-vevent-date /></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
    let multistatus = parse(body).unwrap();
    let p = &multistatus.responses[0].properties;
    assert_eq!(p.calendar_timezone_id.as_deref(), Some("Europe/Madrid"));
    assert_eq!(
        p.default_alarm_datetime.as_deref(),
        Some("BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM")
    );
    assert_eq!(p.default_alarm_date.as_deref(), Some(""));

    let huge = "X".repeat(crate::MAX_DEFAULT_ALARMS + 1);
    let body = format!(
        r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:response><D:href>/c/</D:href><D:propstat><D:prop><C:calendar-timezone-id>Europe/ Madrid</C:calendar-timezone-id><C:default-alarm-vevent-date>{huge}</C:default-alarm-vevent-date><C:displayname/></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#
    );
    let multistatus = parse(&body).unwrap();
    let p = &multistatus.responses[0].properties;
    assert_eq!(p.calendar_timezone_id, None);
    assert_eq!(p.default_alarm_date, None);
}

/// Nextcloud 34: a subscription without the webcal header, a published
/// calendar, a deleted calendar and the trash bin's events (answers seen
/// live, 2026-10-30).
#[test]
fn nextcloud_subscriptions_publishing_and_trash() {
    let base = "http://127.0.0.1:5234/remote.php/dav/calendars/alice/";
    let body = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:nc="http://nextcloud.org/ns">
<d:response><d:href>/remote.php/dav/calendars/alice/sub1/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cs:subscribed/></d:resourcetype><cs:source><d:href>https://example.com/holidays.ics</d:href></cs:source></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/bad/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><cs:subscribed/></d:resourcetype><cs:source><d:href>file:///etc/passwd</d:href></cs:source></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/work/</d:href><d:propstat><d:prop><cs:publish-url><d:href>http://127.0.0.1:5234/remote.php/dav/public-calendars/XFE3yf6GQQXDoPZQ</d:href></cs:publish-url><d:resourcetype><d:collection/><cal:calendar/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/old/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><x2:deleted-calendar xmlns:x2="http://nextcloud.com/ns"/></d:resourcetype><x2:deleted-at xmlns:x2="http://nextcloud.com/ns">2026-10-28T09:31:37+00:00</x2:deleted-at></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/trashbin/objects/51.ics</d:href><d:propstat><d:prop><x1:deleted-at xmlns:x1="http://nextcloud.com/ns">2026-10-30T03:36:50+00:00</x1:deleted-at><x1:calendar-uri xmlns:x1="http://nextcloud.com/ns">probe-share</x1:calendar-uri><x1:trash-bin-retention-duration xmlns:x1="http://nextcloud.com/ns">2592000</x1:trash-bin-retention-duration></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
<d:response><d:href>/remote.php/dav/calendars/alice/trashbin/objects/52.ics</d:href><d:propstat><d:prop><x1:calendar-uri xmlns:x1="http://nextcloud.com/ns">a/b</x1:calendar-uri></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>
</d:multistatus>"#;
    let ms = parse_at(base, body).unwrap();
    let [sub, bad, work, old, event, odd] = ms.responses.as_slice() else {
        panic!("six responses");
    };
    let kind = sub.properties.resource_type.unwrap();
    assert!(kind.subscribed && !kind.calendar);
    assert_eq!(
        sub.properties
            .source
            .as_ref()
            .map(crate::ExternalUrl::as_str),
        Some("https://example.com/holidays.ics")
    );
    assert!(bad.properties.has(Property::Source) && bad.properties.source.is_none());
    assert_eq!(
        work.properties
            .publish_url
            .as_ref()
            .map(crate::ExternalUrl::as_str),
        Some("http://127.0.0.1:5234/remote.php/dav/public-calendars/XFE3yf6GQQXDoPZQ")
    );
    let kind = old.properties.resource_type.unwrap();
    assert!(kind.deleted_calendar && !kind.calendar);
    assert_eq!(
        old.properties.deleted_at.as_deref(),
        Some("2026-10-28T09:31:37+00:00")
    );
    assert_eq!(
        event.properties.calendar_uri.as_deref(),
        Some("probe-share")
    );
    assert_eq!(event.properties.trash_bin_retention, Some(2_592_000));
    assert_eq!(
        odd.properties.calendar_uri, None,
        "a slash is not one segment"
    );
}
