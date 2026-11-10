// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Header fields in the JMAP forms (RFC 8621 §4.1.2): `headers` (Raw) and
//! `header:{name}[:as{Form}][:all]`.
//!
//! Each form is parsed from the field's raw bytes, whatever `mail-parser`
//! parsed for the field by default: the form decides how a value is read
//! (a `Subject` asked `asRaw` is not decoded, an unknown field may be read
//! `asAddresses`).

use mail_parser::{
    AddressList, DateTime, HeaderForm as ParsedForm, HeaderValue as ParsedValue, Headers, Mailbox,
};
use nebula_jmap_proto::Date;
use nebula_jmap_proto::object::email::{
    EmailAddress, EmailAddressGroup, EmailHeader, HeaderForm, HeaderFormValue, HeaderInstances,
    HeaderProperty, HeaderValue,
};
use time::{OffsetDateTime, UtcOffset};

/// The Raw form (RFC 8621 §4.1.2.1): only the terminating CRLF goes.
fn raw_text(bytes: &[u8]) -> String {
    nebula_mail::header_raw(bytes)
}

/// Every field of a header block in Raw form (`headers`), names as
/// written.
pub(crate) fn all_raw(headers: Headers<'_>) -> Vec<EmailHeader> {
    headers
        .iter()
        .map(|header| EmailHeader {
            name: header.raw_name().to_owned(),
            value: raw_text(header.raw_value()),
        })
        .collect()
}

/// A display name: unfolded, trimmed and in NFC (RFC 8621 §4.1.2.3).
fn name(name: Option<&str>) -> Option<String> {
    name.map(nebula_mail::header_display_name)
}

fn address(mailbox: Mailbox<'_>) -> EmailAddress {
    EmailAddress {
        name: name(mailbox.name()),
        email: mailbox.address().map(str::to_owned).unwrap_or_default(),
    }
}

fn addresses(list: Option<AddressList<'_>>) -> Vec<EmailAddress> {
    list.map(|list| list.mailboxes().map(address).collect())
        .unwrap_or_default()
}

fn grouped(list: Option<AddressList<'_>>) -> Vec<EmailAddressGroup> {
    list.map(|list| {
        list.groups()
            .map(|(name, members)| EmailAddressGroup {
                name: self::name(name),
                addresses: members.map(address).collect(),
            })
            .collect()
    })
    .unwrap_or_default()
}

/// A parsed `Date` field as a JMAP `Date`, keeping the sender's offset.
/// Anyone can write this field, so nothing here may panic: out-of-range
/// parts give `None`, and so does a date `Date::new` won't hold. A leap
/// second (`:60`) is kept (it is the next minute's `:00`); mail-parser's
/// `is_valid` would refuse it, and its 1900..=3000 years too.
pub(crate) fn date(value: &DateTime) -> Option<Date> {
    let in_range = (1..=12).contains(&value.month)
        && (1..=31).contains(&value.day)
        && value.hour <= 23
        && value.minute <= 59
        && value.second <= 60
        && value.tz_hour <= 23
        && value.tz_minute <= 59;
    if !in_range {
        return None;
    }
    let minutes = i32::from(value.tz_hour)
        .saturating_mul(60)
        .saturating_add(i32::from(value.tz_minute));
    let minutes = if value.tz_before_gmt {
        -minutes
    } else {
        minutes
    };
    let offset = UtcOffset::from_whole_seconds(minutes.saturating_mul(60)).ok()?;
    let utc = OffsetDateTime::from_unix_timestamp(value.to_timestamp()).ok()?;
    Date::new(utc.checked_to_offset(offset)?).ok()
}

/// One field's value (the bytes after the colon) in `form`.
fn in_form(bytes: &[u8], form: HeaderForm) -> HeaderFormValue {
    match form {
        HeaderForm::Text => HeaderFormValue::Text(nebula_mail::header_text(bytes)),
        HeaderForm::Addresses => {
            let parsed = ParsedForm::Addresses.parse(bytes);
            HeaderFormValue::Addresses(addresses(parsed.value().as_address()))
        }
        HeaderForm::GroupedAddresses => {
            let parsed = ParsedForm::Addresses.parse(bytes);
            HeaderFormValue::GroupedAddresses(grouped(parsed.value().as_address()))
        }
        HeaderForm::MessageIds => {
            let parsed = ParsedForm::MessageIds.parse(bytes);
            HeaderFormValue::MessageIds(match parsed.value() {
                ParsedValue::TextList(list) => Some(list.iter().map(str::to_owned).collect()),
                ParsedValue::Text(text) => Some(vec![text.to_owned()]),
                _ => None,
            })
        }
        HeaderForm::Date => {
            let parsed = ParsedForm::Date.parse(bytes);
            HeaderFormValue::Date(parsed.value().as_datetime().as_ref().and_then(date))
        }
        HeaderForm::Urls => {
            // RFC 2369 URLs are read like addresses: `<url>, <url>`.
            let parsed = ParsedForm::Addresses.parse(bytes);
            let urls: Vec<String> = addresses(parsed.value().as_address())
                .into_iter()
                .map(|a| a.email)
                .filter(|url| !url.is_empty())
                .collect();
            HeaderFormValue::Urls((!urls.is_empty()).then_some(urls))
        }
        _ => HeaderFormValue::Raw(raw_text(bytes)),
    }
}

/// The value of a `header:` property over a header block.
pub(crate) fn property(headers: Headers<'_>, property: &HeaderProperty) -> HeaderValue {
    let mut values = headers
        .iter()
        .filter(|header| property.matches(header.raw_name()))
        .map(|header| in_form(header.raw_value(), property.form()));
    match property.instances() {
        HeaderInstances::All => HeaderValue::All(values.collect()),
        _ => values
            .next_back()
            .map_or(HeaderValue::Missing, HeaderValue::Last),
    }
}

#[cfg(test)]
mod tests {
    use mail_parser::MessageParser;
    use serde_json::json;

    use super::*;

    const MESSAGE: &[u8] = b"From: \"Ana B.\" <ana@example.com>\r\n\
Subject: =?utf-8?q?Hola_mundo?=\r\n\
List-Unsubscribe: <https://example.com/u?x=1>, <mailto:u@example.com>\r\n\
Authentication-Results: mx.example.net; dkim=pass\r\n\
Authentication-Results: other.example; dkim=fail\r\n\
Date: Thu, 24 Oct 2026 10:00:00 +0200\r\n\
X-Team: Devs: a@x.com, b@y.com;\r\n\
\r\nBody\r\n";

    fn value(name: &str) -> serde_json::Value {
        let message = MessageParser::default().parse(MESSAGE).unwrap();
        let property: HeaderProperty = name.parse().unwrap();
        serde_json::to_value(super::property(message.headers(), &property)).unwrap()
    }

    fn mail_date(text: &str) -> Option<Date> {
        let raw = format!("Date: {text}\r\n\r\n");
        let message = MessageParser::default().parse(raw.as_bytes())?;
        date(&message.date()?.clone())
    }

    #[test]
    fn hostile_dates_give_none() {
        // Year 9999 a little after 23:00 at -01:00 is year 10000 in UTC.
        assert_eq!(mail_date("Fri, 31 Dec 9999 23:30:00 -0100"), None);
        assert_eq!(mail_date("Fri, 31 Dec 9999 24:30:00 +0100"), None);
        assert_eq!(mail_date("Fri, 31 Dec 2026 25:00:00 +0100"), None);
        let far = DateTime {
            year: u16::MAX,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            tz_before_gmt: true,
            tz_hour: 23,
            tz_minute: 59,
        };
        assert_eq!(date(&far), None);
        let mut bad_month = far;
        bad_month.year = 2026;
        bad_month.month = 13;
        assert_eq!(date(&bad_month), None);
    }

    #[test]
    fn normal_and_leap_second_dates() {
        assert_eq!(
            mail_date("Thu, 24 Oct 2026 10:00:00 +0200").map(|d| d.to_string()),
            Some("2026-10-24T10:00:00+02:00".to_owned())
        );
        assert_eq!(
            mail_date("Wed, 31 Dec 2016 23:59:60 +0000").map(|d| d.to_string()),
            Some("2017-01-01T00:00:00Z".to_owned())
        );
        // Old years mail-parser's is_valid() would refuse.
        assert!(mail_date("Mon, 1 Jan 1800 00:00:00 +0000").is_some());
    }

    #[test]
    fn forms() {
        assert_eq!(value("header:Subject"), json!(" =?utf-8?q?Hola_mundo?="));
        assert_eq!(value("header:Subject:asText"), json!("Hola mundo"));
        assert_eq!(
            value("header:From:asAddresses"),
            json!([{"name": "Ana B.", "email": "ana@example.com"}])
        );
        assert_eq!(
            value("header:List-Unsubscribe:asURLs"),
            json!(["https://example.com/u?x=1", "mailto:u@example.com"])
        );
        assert_eq!(
            value("header:Authentication-Results:asText:all"),
            json!(["mx.example.net; dkim=pass", "other.example; dkim=fail"])
        );
        assert_eq!(
            value("header:Date:asDate"),
            json!("2026-10-24T10:00:00+02:00")
        );
        assert_eq!(value("header:X-Missing"), json!(null));
        assert_eq!(value("header:X-Missing:all"), json!([]));
        assert_eq!(
            value("header:X-Team:asGroupedAddresses"),
            json!([{"name": "Devs", "addresses": [
                {"name": null, "email": "a@x.com"}, {"name": null, "email": "b@y.com"}
            ]}])
        );
    }

    /// RFC 8621 §4.1.2.1–§4.1.2.3: Raw only loses the terminating CRLF,
    /// Text only the leading SP (trailing white space stays) and is in NFC,
    /// and display names are trimmed.
    #[test]
    fn forms_keep_what_the_rfc_keeps() {
        let message = MessageParser::default()
            .parse(
                &b"Subject:  Hola \t\r\n  mundo  \r\nX-A: cafe\xcc\x81 \r\n\
To: \"  James Smythe\" <j@e.c>, Friends: =?utf-8?q?_Ana_?= <a@e.c>;\r\n\r\nBody\r\n"[..],
            )
            .unwrap();
        let get = |name: &str| {
            let property: HeaderProperty = name.parse().unwrap();
            serde_json::to_value(super::property(message.headers(), &property)).unwrap()
        };
        assert_eq!(get("header:Subject"), json!("  Hola \t\r\n  mundo  "));
        assert_eq!(get("header:Subject:asText"), json!("Hola \t  mundo  "));
        assert_eq!(get("header:X-A:asText"), json!("caf\u{e9} "));
        assert_eq!(
            get("header:To:asAddresses"),
            json!([
                {"name": "James Smythe", "email": "j@e.c"},
                {"name": "Ana", "email": "a@e.c"}
            ])
        );
        let all = all_raw(message.headers());
        assert_eq!(all[1].value, " cafe\u{301} ");
    }

    #[test]
    fn raw_list() {
        let message = MessageParser::default().parse(MESSAGE).unwrap();
        let all = all_raw(message.headers());
        assert_eq!(all.len(), 7);
        assert_eq!(all[0].name, "From");
        assert_eq!(all[0].value, " \"Ana B.\" <ana@example.com>");
    }
}
