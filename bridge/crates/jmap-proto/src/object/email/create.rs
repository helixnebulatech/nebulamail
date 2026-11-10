// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Creating an Email with `Email/set` (RFC 8621 §4.6), e.g. a draft.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::address::{EmailAddress, is_single_line};
use super::body::{BodyPartContent, EmailBodyPartCreate, EmailBodyValue, PartId};
use super::header::{HeaderProperties, HeaderProperty, HeaderValue};
use crate::error::SetError;
use crate::method::parse::PropertyErrors;
use crate::references::IdOrRef;
use crate::types::{Date, Keyword, MailboxId, TrueSet, UtcDate};

/// The body of an Email to create: either a full MIME tree, or the parts
/// and the server builds the tree (RFC 8621 §4.6).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmailCreateBody {
    /// `bodyStructure`.
    Structure(Box<EmailBodyPartCreate>),
    /// `textBody`, `htmlBody` and `attachments`.
    Parts {
        /// The `text/plain` body.
        text: Option<Box<EmailBodyPartCreate>>,
        /// The `text/html` body.
        html: Option<Box<EmailBodyPartCreate>>,
        /// Attachments.
        attachments: Vec<EmailBodyPartCreate>,
    },
}

/// An Email to create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailCreate {
    /// Mailboxes (at least one; `#creationId` allowed).
    pub mailbox_ids: BTreeSet<IdOrRef<MailboxId>>,
    /// Keywords.
    pub keywords: BTreeSet<Keyword>,
    /// `receivedAt` (default: now).
    pub received_at: Option<UtcDate>,
    /// `Message-ID` (the server makes one if `None`).
    pub message_id: Option<Vec<String>>,
    /// `In-Reply-To`.
    pub in_reply_to: Option<Vec<String>>,
    /// `References`.
    pub references: Option<Vec<String>>,
    /// `Sender`.
    pub sender: Option<Vec<EmailAddress>>,
    /// `From`.
    pub from: Option<Vec<EmailAddress>>,
    /// `To`.
    pub to: Option<Vec<EmailAddress>>,
    /// `Cc`.
    pub cc: Option<Vec<EmailAddress>>,
    /// `Bcc`.
    pub bcc: Option<Vec<EmailAddress>>,
    /// `Reply-To`.
    pub reply_to: Option<Vec<EmailAddress>>,
    /// `Subject`.
    pub subject: Option<String>,
    /// `Date` (the server uses now if `None`).
    pub sent_at: Option<Date>,
    /// The body.
    pub body: EmailCreateBody,
    /// Text of the parts that use `partId`.
    pub body_values: BTreeMap<PartId, String>,
    /// Other header fields (`header:…`), never `Content-*`.
    pub header_properties: HeaderProperties,
}

/// Header fields each convenience property writes, to reject a second
/// copy through a `header:` property.
const CONVENIENCE_FIELDS: &[(&str, &str)] = &[
    ("messageId", "message-id"),
    ("inReplyTo", "in-reply-to"),
    ("references", "references"),
    ("sender", "sender"),
    ("from", "from"),
    ("to", "to"),
    ("cc", "cc"),
    ("bcc", "bcc"),
    ("replyTo", "reply-to"),
    ("subject", "subject"),
    ("sentAt", "date"),
];

/// Properties the client must not send on create.
const SERVER_SET: &[&str] = &[
    "id",
    "blobId",
    "threadId",
    "size",
    "hasAttachment",
    "preview",
    "headers",
];

#[derive(Default)]
struct Fields {
    mailbox_ids: Option<TrueSet<IdOrRef<MailboxId>>>,
    keywords: Option<TrueSet<Keyword>>,
    received_at: Option<UtcDate>,
    message_id: Option<Vec<String>>,
    in_reply_to: Option<Vec<String>>,
    references: Option<Vec<String>>,
    sender: Option<Vec<EmailAddress>>,
    from: Option<Vec<EmailAddress>>,
    to: Option<Vec<EmailAddress>>,
    cc: Option<Vec<EmailAddress>>,
    bcc: Option<Vec<EmailAddress>>,
    reply_to: Option<Vec<EmailAddress>>,
    subject: Option<String>,
    sent_at: Option<Date>,
    body_structure: Option<EmailBodyPartCreate>,
    text_body: Option<Vec<EmailBodyPartCreate>>,
    html_body: Option<Vec<EmailBodyPartCreate>>,
    attachments: Option<Vec<EmailBodyPartCreate>>,
    body_values: BTreeMap<PartId, String>,
    headers: HeaderProperties,
    given: BTreeSet<String>,
}

fn addresses(errors: &mut PropertyErrors, key: &str, value: Value) -> Option<Vec<EmailAddress>> {
    let list: Option<Vec<EmailAddress>> = errors.parse::<Option<_>>(key, value).flatten();
    if list
        .iter()
        .flatten()
        .any(|address| !address.is_single_line())
    {
        errors.invalid_because(key, "address has a line break");
        return None;
    }
    list
}

fn lines(errors: &mut PropertyErrors, key: &str, value: Value) -> Option<Vec<String>> {
    let list: Option<Vec<String>> = errors.parse::<Option<_>>(key, value).flatten();
    if list.iter().flatten().any(|item| !is_single_line(item)) {
        errors.invalid_because(key, "value has a line break");
        return None;
    }
    list
}

fn body_parts(
    errors: &mut PropertyErrors,
    key: &str,
    value: Value,
) -> Option<Vec<EmailBodyPartCreate>> {
    let Value::Array(items) = value else {
        errors.invalid_because(key, "must be an array of body parts");
        return None;
    };
    let parsed = items
        .into_iter()
        .map(|item| EmailBodyPartCreate::parse(item, 0))
        .collect::<Result<Vec<_>, _>>();
    match parsed {
        Ok(parts) => Some(parts),
        Err(why) => {
            errors.invalid_because(key, why);
            None
        }
    }
}

fn body_values(errors: &mut PropertyErrors, value: Value) -> BTreeMap<PartId, String> {
    let Some(values) = errors.parse::<BTreeMap<PartId, EmailBodyValue>>("bodyValues", value) else {
        return BTreeMap::new();
    };
    if values
        .values()
        .any(|v| v.is_truncated || v.is_encoding_problem)
    {
        errors.invalid_because(
            "bodyValues",
            "isTruncated and isEncodingProblem must be false",
        );
    }
    values.into_iter().map(|(id, v)| (id, v.value)).collect()
}

fn read_property(fields: &mut Fields, errors: &mut PropertyErrors, key: &str, value: Value) {
    match key {
        "mailboxIds" => fields.mailbox_ids = errors.parse(key, value),
        "keywords" => fields.keywords = errors.parse::<Option<_>>(key, value).flatten(),
        "receivedAt" => fields.received_at = errors.parse::<Option<_>>(key, value).flatten(),
        "messageId" => fields.message_id = lines(errors, key, value),
        "inReplyTo" => fields.in_reply_to = lines(errors, key, value),
        "references" => fields.references = lines(errors, key, value),
        "sender" => fields.sender = addresses(errors, key, value),
        "from" => fields.from = addresses(errors, key, value),
        "to" => fields.to = addresses(errors, key, value),
        "cc" => fields.cc = addresses(errors, key, value),
        "bcc" => fields.bcc = addresses(errors, key, value),
        "replyTo" => fields.reply_to = addresses(errors, key, value),
        "subject" => {
            fields.subject = errors.parse::<Option<String>>(key, value).flatten();
            if fields
                .subject
                .as_deref()
                .is_some_and(|s| !is_single_line(s))
            {
                errors.invalid_because(key, "has a line break");
            }
        }
        "sentAt" => fields.sent_at = errors.parse::<Option<_>>(key, value).flatten(),
        "bodyStructure" => match EmailBodyPartCreate::parse(value, 0) {
            Ok(part) => fields.body_structure = Some(part),
            Err(why) => errors.invalid_because(key, why),
        },
        "textBody" => fields.text_body = body_parts(errors, key, value),
        "htmlBody" => fields.html_body = body_parts(errors, key, value),
        "attachments" => fields.attachments = body_parts(errors, key, value),
        "bodyValues" => fields.body_values = body_values(errors, value),
        header if header.starts_with("header:") => read_header(fields, errors, header, value),
        server_set if SERVER_SET.contains(&server_set) => {
            errors.invalid_because(server_set, "set by the server");
        }
        unknown => errors.invalid(unknown),
    }
}

fn read_header(fields: &mut Fields, errors: &mut PropertyErrors, key: &str, value: Value) {
    let property = match key.parse::<HeaderProperty>() {
        Ok(property) => property,
        Err(error) => return errors.invalid_because(key, error.to_string()),
    };
    let lower = property.name().to_ascii_lowercase();
    if lower.starts_with("content-") {
        return errors.invalid_because(key, "Content-* fields go on body parts");
    }
    if lower == "mime-version" {
        return errors.invalid_because(key, "MIME-Version is written by the server");
    }
    match HeaderValue::from_json(&property, value) {
        Ok(HeaderValue::All(list)) if list.len() > 1 && SINGLE_FIELDS.contains(&lower.as_str()) => {
            errors.invalid_because(
                key,
                format!("only one {} field is allowed", property.name()),
            );
        }
        Ok(value) => fields.headers.insert(property, value),
        Err(why) => errors.invalid_because(key, why),
    }
}

/// Fields a message has at most once (RFC 5322 §3.6), lower case: `:all`
/// with more than one value is refused for them.
const SINGLE_FIELDS: &[&str] = &[
    "date",
    "from",
    "sender",
    "reply-to",
    "to",
    "cc",
    "bcc",
    "message-id",
    "in-reply-to",
    "references",
    "subject",
];

/// Most body parts (leaves and multiparts) of one Email to create.
const MAX_BODY_PARTS: usize = 256;

impl EmailCreate {
    /// Parses one object of `Email/set` `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every problem found.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut fields = Fields::default();
        for (key, value) in object {
            fields.given.insert(key.clone());
            read_property(&mut fields, &mut errors, &key, value);
        }
        check_duplicate_headers(&fields, &mut errors);
        let mailbox_ids = errors
            .required("mailboxIds", fields.mailbox_ids.take())
            .map(TrueSet::into_set)
            .unwrap_or_default();
        if fields.given.contains("mailboxIds") && mailbox_ids.is_empty() {
            errors.invalid_because("mailboxIds", "at least one mailbox is required");
        }
        let body = build_body(&mut fields, &mut errors);
        check_part_ids(&body, &fields.body_values, &mut errors);
        errors.finish()?;
        Ok(Self {
            mailbox_ids,
            keywords: fields.keywords.map(TrueSet::into_set).unwrap_or_default(),
            received_at: fields.received_at,
            message_id: fields.message_id,
            in_reply_to: fields.in_reply_to,
            references: fields.references,
            sender: fields.sender,
            from: fields.from,
            to: fields.to,
            cc: fields.cc,
            bcc: fields.bcc,
            reply_to: fields.reply_to,
            subject: fields.subject,
            sent_at: fields.sent_at,
            body,
            body_values: fields.body_values,
            header_properties: fields.headers,
        })
    }
}

/// Two properties for one header field (`from` and `header:From`, or
/// `header:X` and `header:x:asText`) are not allowed.
fn check_duplicate_headers(fields: &Fields, errors: &mut PropertyErrors) {
    let mut seen: BTreeSet<String> = CONVENIENCE_FIELDS
        .iter()
        .filter(|(property, _)| fields.given.contains(*property))
        .map(|(_, field)| (*field).to_owned())
        .collect();
    for (property, _) in fields.headers.iter() {
        if !seen.insert(property.name().to_ascii_lowercase()) {
            errors.invalid_because(property.as_str(), "duplicates another property");
        }
    }
}

fn single_part(
    errors: &mut PropertyErrors,
    key: &str,
    parts: Option<Vec<EmailBodyPartCreate>>,
    media_type: &str,
) -> Option<Box<EmailBodyPartCreate>> {
    let mut parts = parts?.into_iter();
    match (parts.next(), parts.next()) {
        (Some(part), None) if part.media_type == media_type => Some(Box::new(part)),
        _ => {
            errors.invalid_because(key, format!("must be exactly one {media_type} part"));
            None
        }
    }
}

fn build_body(fields: &mut Fields, errors: &mut PropertyErrors) -> EmailCreateBody {
    let has_parts = ["textBody", "htmlBody", "attachments"]
        .iter()
        .any(|key| fields.given.contains(*key));
    if let Some(structure) = fields.body_structure.take() {
        if has_parts {
            errors.invalid_because(
                "bodyStructure",
                "bodyStructure excludes textBody, htmlBody and attachments",
            );
        }
        return EmailCreateBody::Structure(Box::new(structure));
    }
    EmailCreateBody::Parts {
        text: single_part(errors, "textBody", fields.text_body.take(), "text/plain"),
        html: single_part(errors, "htmlBody", fields.html_body.take(), "text/html"),
        attachments: fields.attachments.take().unwrap_or_default(),
    }
}

/// Every `partId` used must be in `bodyValues`, and used once (RFC 8621
/// §4.6); and there are at most [`MAX_BODY_PARTS`] parts.
fn check_part_ids(
    body: &EmailCreateBody,
    values: &BTreeMap<PartId, String>,
    errors: &mut PropertyErrors,
) {
    fn visit<'a>(part: &'a EmailBodyPartCreate, out: &mut Vec<&'a PartId>, count: &mut usize) {
        *count = count.saturating_add(1);
        match &part.content {
            BodyPartContent::Value(id) => out.push(id),
            BodyPartContent::Blob(_) => {}
            BodyPartContent::Multipart(children) => {
                for child in children {
                    visit(child, out, count);
                }
            }
        }
    }
    let mut used = Vec::new();
    let mut count = 0_usize;
    match body {
        EmailCreateBody::Structure(part) => visit(part, &mut used, &mut count),
        EmailCreateBody::Parts {
            text,
            html,
            attachments,
        } => {
            for part in text
                .as_deref()
                .into_iter()
                .chain(html.as_deref())
                .chain(attachments)
            {
                visit(part, &mut used, &mut count);
            }
        }
    }
    let key = match body {
        EmailCreateBody::Structure(_) => "bodyStructure",
        EmailCreateBody::Parts { .. } => "attachments",
    };
    if count > MAX_BODY_PARTS {
        errors.invalid_because(key, format!("more than {MAX_BODY_PARTS} body parts"));
    }
    if let Some(missing) = used.iter().find(|id| !values.contains_key(**id)) {
        errors.invalid_because("bodyValues", format!("no value for partId {missing}"));
    }
    let mut seen = BTreeSet::new();
    if let Some(repeated) = used.iter().find(|id| !seen.insert(**id)) {
        errors.invalid_because(key, format!("partId {repeated} used more than once"));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    fn properties(value: Value) -> Vec<String> {
        let mut list = EmailCreate::parse(map(value))
            .unwrap_err()
            .properties()
            .to_vec();
        list.sort();
        list
    }

    /// The draft the web client saves (compose.ts `draftEmail`).
    #[test]
    fn web_client_draft() {
        let email = EmailCreate::parse(map(json!({
            "htmlBody": [{"partId": "html", "type": "text/html"}],
            "textBody": [{"partId": "text", "type": "text/plain"}],
            "attachments": [{"blobId": "b1", "disposition": "attachment",
                             "name": "x.pdf", "type": "application/pdf"}],
            "bodyValues": {"html": {"value": "<p>Hi</p>"}, "text": {"value": "Hi"}},
            "from": [{"email": "alice@localhost", "name": null}],
            "keywords": {"$draft": true, "$seen": true},
            "mailboxIds": {"m-drafts": true},
            "subject": "Hello",
            "to": [{"email": "bob@localhost", "name": "Bob"}],
            "inReplyTo": ["<a@b>"]
        })))
        .unwrap();
        assert_eq!(email.keywords, [Keyword::draft(), Keyword::seen()].into());
        let EmailCreateBody::Parts {
            text,
            html,
            attachments,
        } = email.body
        else {
            panic!("expected parts")
        };
        assert!(text.is_some() && html.is_some());
        assert_eq!(attachments.len(), 1);
        assert_eq!(email.body_values.len(), 2);
    }

    /// RFC 8621 §4.10: a draft with a mailbox created in the same request.
    #[test]
    fn creation_reference_and_headers() {
        let email = EmailCreate::parse(map(json!({
            "mailboxIds": {"#k1": true},
            "from": [{"name": "Joe Bloggs", "email": "joe@example.com"}],
            "subject": "World domination",
            "receivedAt": "2018-07-10T01:03:11Z",
            "sentAt": "2018-07-10T11:03:11+10:00",
            "header:X-Mailer:asText": "Nebula",
            "bodyStructure": {"partId": "1", "type": "text/plain"},
            "bodyValues": {"1": {"value": "I have the most brilliant plan."}}
        })))
        .unwrap();
        assert!(email.mailbox_ids.contains(&"#k1".parse().unwrap()));
        assert!(matches!(email.body, EmailCreateBody::Structure(_)));
        assert!(!email.header_properties.is_empty());
    }

    #[test]
    fn invalid_properties_are_all_listed() {
        assert_eq!(
            properties(json!({
                "id": "e1",
                "mailboxIds": {},
                "keywords": {"$seen": false},
                "header:Content-Type": " text/plain",
                "header:From:asDate": "x",
                "subject": "a\r\nBcc: evil@example.com",
                "nope": 1
            })),
            [
                "header:Content-Type",
                "header:From:asDate",
                "id",
                "keywords",
                "mailboxIds",
                "nope",
                "subject"
            ]
        );
        assert_eq!(properties(json!({})), ["mailboxIds"]);
        assert_eq!(
            properties(
                json!({"mailboxIds": {"m": true}, "from": [], "header:from:asAddresses": []})
            ),
            ["header:from:asAddresses"]
        );
        assert_eq!(
            properties(json!({
                "mailboxIds": {"m": true},
                "bodyStructure": {"partId": "1"},
                "textBody": [{"partId": "1"}],
                "bodyValues": {"1": {"value": "x"}}
            })),
            ["bodyStructure"]
        );
        assert_eq!(
            properties(json!({
                "mailboxIds": {"m": true},
                "htmlBody": [{"partId": "1"}],
                "bodyValues": {"1": {"value": "x"}}
            })),
            ["htmlBody"]
        );
        assert_eq!(
            properties(json!({
                "mailboxIds": {"m": true},
                "textBody": [{"partId": "1"}],
                "bodyValues": {"2": {"value": "x", "isTruncated": true}}
            })),
            ["bodyValues"]
        );
    }

    #[test]
    fn single_fields_and_mime_version() {
        let base = |extra: Value| {
            let mut object = map(json!({
                "mailboxIds": {"m": true},
                "textBody": [{"partId": "1"}],
                "bodyValues": {"1": {"value": "x"}}
            }));
            object.extend(map(extra));
            object
        };
        let two = json!([[{"email": "a@b"}], [{"email": "c@d"}]]);
        assert_eq!(
            EmailCreate::parse(base(json!({"header:From:asAddresses:all": two})))
                .unwrap_err()
                .properties(),
            ["header:From:asAddresses:all"]
        );
        assert!(
            EmailCreate::parse(base(
                json!({"header:From:asAddresses:all": [[{"email": "a@b"}]]})
            ))
            .is_ok()
        );
        assert!(EmailCreate::parse(base(json!({"header:X-Algo:all": [" 1", " 2"]}))).is_ok());
        assert!(
            EmailCreate::parse(base(json!({"header:subject:asText:all": ["a", "b"]}))).is_err()
        );
        for key in ["header:MIME-Version", "header:mime-version:asRaw"] {
            assert_eq!(
                EmailCreate::parse(base(json!({key: " 1.0"})))
                    .unwrap_err()
                    .properties(),
                [key]
            );
        }
    }

    #[test]
    fn part_ids_once_and_part_count() {
        assert_eq!(
            properties(json!({
                "mailboxIds": {"m": true},
                "bodyStructure": {"type": "multipart/mixed", "subParts": [
                    {"partId": "1"}, {"partId": "1", "type": "text/html"}
                ]},
                "bodyValues": {"1": {"value": "x"}}
            })),
            ["bodyStructure"]
        );
        let attachments = |count: usize| -> Vec<Value> {
            (0..count)
                .map(|_| json!({"blobId": "b1", "type": "image/png"}))
                .collect()
        };
        let email = |count: usize| {
            EmailCreate::parse(map(json!({
                "mailboxIds": {"m": true},
                "textBody": [{"partId": "1"}],
                "bodyValues": {"1": {"value": "x"}},
                "attachments": attachments(count)
            })))
        };
        assert!(email(MAX_BODY_PARTS - 1).is_ok());
        assert_eq!(
            email(MAX_BODY_PARTS).unwrap_err().properties(),
            ["attachments"]
        );
    }
}
