// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `emailPush` (Stalwart 1.0's `urn:ietf:params:jmap:emailpush`)
//! a subscription may ask for the delivered emails of
//! the user's account inside the push, an `EmailPush` object sent instead
//! of the delivery's `StateChange`.
//!
//! - **What can be asked for** ([`EmailPush::from_request`]): only the
//!   user's own account (any other is `forbidden`); the `Email` properties
//!   the store answers without the message ([`SUPPORTED_FIELDS`]: ids,
//!   folders, keywords, size, dates, addresses, subject, `hasAttachment`
//!   and `preview`); a filter of the conditions that run on the stored
//!   metadata (folder, keyword, date, size, attachment) with `AND`, `OR`
//!   and `NOT`. Everything else (bodies, `headers`, `header:…`, text
//!   conditions, which the bridge answers with IMAP `SEARCH`, thread
//!   conditions) is `invalidProperties`, never ignored: the web app then
//!   subscribes again without `emailPush`.
//! - **What is sent** ([`build`]): the asked properties of each delivered
//!   email that passes the filter, as `Email/get` gives them; nothing if
//!   none passes. What doesn't fit in one push message is left out
//!   property by property, as Stalwart does (counted exactly here); an
//!   email with nothing left is not sent.

use nebula_jmap_proto::method::{Filter, Operator};
use nebula_jmap_proto::object::email::{EmailField, EmailFilterCondition, EmailProperty};
use nebula_jmap_proto::object::{EmailPushConfig, EmailPushMap, PushUrgency};
use nebula_jmap_proto::push::EmailPush as EmailPushObject;
use nebula_jmap_proto::{
    AccountId as JmapAccountId, Keyword, MailboxId as JmapMailboxId, SetError, SetErrorType, State,
};
use nebula_store::Email as StoredEmail;
use nebula_webpush::Urgency;
use serde_json::{Map, Value, json};

use crate::core::ids;
use crate::email::get::listed;

/// The `Email` properties an `EmailPush` can carry: the ones the store
/// has (`preview` may need the first bytes of the text, as `Email/get`).
pub(crate) const SUPPORTED_FIELDS: &[EmailField] = &[
    EmailField::Id,
    EmailField::BlobId,
    EmailField::ThreadId,
    EmailField::MailboxIds,
    EmailField::Keywords,
    EmailField::Size,
    EmailField::ReceivedAt,
    EmailField::MessageId,
    EmailField::InReplyTo,
    EmailField::References,
    EmailField::Sender,
    EmailField::From,
    EmailField::To,
    EmailField::Cc,
    EmailField::Bcc,
    EmailField::ReplyTo,
    EmailField::Subject,
    EmailField::SentAt,
    EmailField::HasAttachment,
    EmailField::Preview,
];

/// The filter conditions supported (on the stored metadata).
const SUPPORTED_CONDITIONS: &[&str] = &[
    "inMailbox",
    "inMailboxOtherThan",
    "before",
    "after",
    "minSize",
    "maxSize",
    "hasKeyword",
    "notKeyword",
    "hasAttachment",
];

/// One condition of the filter.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Condition {
    InMailbox(JmapMailboxId),
    InMailboxOtherThan(Vec<JmapMailboxId>),
    /// `receivedAt` before (Unix seconds).
    Before(i64),
    /// `receivedAt` at or after.
    After(i64),
    MinSize(u64),
    MaxSize(u64),
    HasKeyword(Keyword),
    NotKeyword(Keyword),
    HasAttachment(bool),
}

impl Condition {
    fn matches(&self, email: &StoredEmail, mailboxes: &[JmapMailboxId]) -> bool {
        match self {
            Self::InMailbox(id) => mailboxes.contains(id),
            Self::InMailboxOtherThan(ids) => mailboxes.iter().any(|m| !ids.contains(m)),
            Self::Before(at) => email.meta.received_at.secs() < *at,
            Self::After(at) => email.meta.received_at.secs() >= *at,
            Self::MinSize(size) => email.meta.size >= *size,
            Self::MaxSize(size) => email.meta.size < *size,
            Self::HasKeyword(keyword) => email.keywords.contains(keyword),
            Self::NotKeyword(keyword) => !email.keywords.contains(keyword),
            Self::HasAttachment(value) => email.meta.has_attachment == *value,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::InMailbox(id) => json!({"inMailbox": id}),
            Self::InMailboxOtherThan(ids) => json!({"inMailboxOtherThan": ids}),
            Self::Before(at) => json!({"before": date(*at)}),
            Self::After(at) => json!({"after": date(*at)}),
            Self::MinSize(size) => json!({"minSize": size}),
            Self::MaxSize(size) => json!({"maxSize": size}),
            Self::HasKeyword(keyword) => json!({"hasKeyword": keyword}),
            Self::NotKeyword(keyword) => json!({"notKeyword": keyword}),
            Self::HasAttachment(value) => json!({"hasAttachment": value}),
        }
    }
}

fn date(secs: i64) -> Value {
    nebula_jmap_proto::UtcDate::from_unix_timestamp(secs)
        .ok()
        .and_then(|date| serde_json::to_value(date).ok())
        .unwrap_or(Value::Null)
}

/// The filter, as a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Is(Condition),
    All(Vec<Node>),
    Any(Vec<Node>),
    None(Vec<Node>),
}

impl Node {
    fn matches(&self, email: &StoredEmail, mailboxes: &[JmapMailboxId]) -> bool {
        match self {
            Self::Is(condition) => condition.matches(email, mailboxes),
            Self::All(nodes) => nodes.iter().all(|n| n.matches(email, mailboxes)),
            Self::Any(nodes) => nodes.iter().any(|n| n.matches(email, mailboxes)),
            Self::None(nodes) => !nodes.iter().any(|n| n.matches(email, mailboxes)),
        }
    }

    /// As `PushSubscription/get` shows it (a condition object with several
    /// members is an `AND` of one-member ones, as Stalwart shows it).
    fn to_json(&self) -> Value {
        let operator = |operator: Operator, nodes: &[Self]| {
            json!({
                "operator": operator,
                "conditions": nodes.iter().map(Self::to_json).collect::<Vec<_>>(),
            })
        };
        match self {
            Self::Is(condition) => condition.to_json(),
            Self::All(nodes) => operator(Operator::And, nodes),
            Self::Any(nodes) => operator(Operator::Or, nodes),
            Self::None(nodes) => operator(Operator::Not, nodes),
        }
    }

    fn from_filter(filter: Filter<EmailFilterCondition>) -> Result<Self, SetError> {
        match filter {
            Filter::Operator(operator) => {
                let nodes = operator
                    .conditions
                    .into_iter()
                    .map(Self::from_filter)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(match operator.operator {
                    Operator::Or => Self::Any(nodes),
                    Operator::Not => Self::None(nodes),
                    _ => Self::All(nodes),
                })
            }
            Filter::Condition(condition) => {
                let mut nodes = from_condition(condition)?;
                Ok(
                    if nodes.len() == 1
                        && let Some(node) = nodes.pop()
                    {
                        node
                    } else {
                        Self::All(nodes)
                    },
                )
            }
            _ => Err(unsupported("this filter is not supported")),
        }
    }
}

fn unsupported(description: &str) -> SetError {
    SetError::invalid_properties(["emailPush"]).with_description(description)
}

/// The members of one condition object, each a node.
fn from_condition(condition: EmailFilterCondition) -> Result<Vec<Node>, SetError> {
    let EmailFilterCondition {
        in_mailbox,
        in_mailbox_other_than,
        before,
        after,
        min_size,
        max_size,
        all_in_thread_have_keyword,
        some_in_thread_have_keyword,
        none_in_thread_have_keyword,
        has_keyword,
        not_keyword,
        has_attachment,
        text,
        from,
        to,
        cc,
        bcc,
        subject,
        body,
        header,
    } = condition;
    let thread = all_in_thread_have_keyword.is_some()
        || some_in_thread_have_keyword.is_some()
        || none_in_thread_have_keyword.is_some();
    let words = [text, from, to, cc, bcc, subject, body]
        .iter()
        .any(Option::is_some)
        || header.is_some();
    if thread || words {
        return Err(unsupported(&format!(
            "only these filter conditions are supported in emailPush: {}",
            SUPPORTED_CONDITIONS.join(", ")
        )));
    }
    let mut nodes = Vec::new();
    let mut add = |condition: Option<Condition>| {
        if let Some(condition) = condition {
            nodes.push(Node::Is(condition));
        }
    };
    add(in_mailbox.map(Condition::InMailbox));
    add(in_mailbox_other_than.map(Condition::InMailboxOtherThan));
    add(before.map(|d| Condition::Before(d.unix_timestamp())));
    add(after.map(|d| Condition::After(d.unix_timestamp())));
    add(min_size.map(|n| Condition::MinSize(n.get())));
    add(max_size.map(|n| Condition::MaxSize(n.get())));
    add(has_keyword.map(Condition::HasKeyword));
    add(not_keyword.map(Condition::NotKeyword));
    add(has_attachment.map(Condition::HasAttachment));
    Ok(nodes)
}

/// What a subscription asked for, checked: the delivered mail of the
/// user's account goes in its pushes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmailPush {
    properties: Vec<EmailField>,
    filter: Option<Node>,
    urgency: PushUrgency,
}

impl EmailPush {
    /// Checks a request's `emailPush` (`None`: none asked).
    ///
    /// # Errors
    /// `forbidden` for an account other than `own`; `invalidProperties`
    /// (naming `emailPush`) for what isn't supported.
    pub(crate) fn from_request(
        own: &JmapAccountId,
        map: Option<EmailPushMap>,
    ) -> Result<Option<Self>, SetError> {
        let Some(map) = map else {
            return Ok(None);
        };
        let mut found = None;
        for (account, config) in map {
            if account != *own {
                return Err(SetError::new(SetErrorType::Forbidden)
                    .with_description("emailPush is only possible for the user's own account"));
            }
            found = Some(Self::from_config(config)?);
        }
        Ok(found)
    }

    fn from_config(config: EmailPushConfig) -> Result<Self, SetError> {
        let mut properties = Vec::with_capacity(config.properties.len());
        for property in config.properties {
            match property {
                EmailProperty::Field(field) if SUPPORTED_FIELDS.contains(&field) => {
                    properties.push(field);
                }
                _ => {
                    return Err(unsupported(
                        "emailPush supports the Email properties of the list (no bodies, \
                         headers or header:… properties)",
                    ));
                }
            }
        }
        if properties.is_empty() {
            // Stalwart would send empty objects; the id at least.
            properties.push(EmailField::Id);
        }
        Ok(Self {
            properties,
            filter: config.filter.map(Node::from_filter).transpose()?,
            urgency: config.urgency,
        })
    }

    /// `emailPush` as `PushSubscription/get` shows it (also what is kept,
    /// sealed).
    pub(crate) fn to_json(&self, account: &JmapAccountId) -> Value {
        let mut config = Map::new();
        config.insert(
            "filter".to_owned(),
            self.filter.as_ref().map_or(Value::Null, Node::to_json),
        );
        config.insert(
            "properties".to_owned(),
            Value::Array(
                self.properties
                    .iter()
                    .map(|p| Value::from(p.as_str()))
                    .collect(),
            ),
        );
        config.insert("urgency".to_owned(), Value::from(self.urgency.as_str()));
        let mut map = Map::new();
        map.insert(account.to_string(), Value::Object(config));
        Value::Object(map)
    }

    /// Reads back what [`EmailPush::to_json`] wrote.
    pub(crate) fn from_json(account: &JmapAccountId, value: &Value) -> Option<Self> {
        let map = EmailPushConfig::parse_map(value).ok()?;
        Self::from_request(account, map).ok().flatten()
    }

    /// Whether a delivered email goes in the push.
    pub(crate) fn matches(&self, email: &StoredEmail) -> bool {
        let Some(filter) = &self.filter else {
            return true;
        };
        let mailboxes: Vec<JmapMailboxId> = email
            .mailboxes
            .iter()
            .filter_map(|m| ids::mailbox_id(*m).ok())
            .collect();
        filter.matches(email, &mailboxes)
    }

    /// Whether `preview` is asked for (it may need the message's text).
    pub(crate) fn wants_preview(&self) -> bool {
        self.properties.contains(&EmailField::Preview)
    }

    pub(crate) fn urgency(&self) -> Urgency {
        match self.urgency {
            PushUrgency::VeryLow => Urgency::VeryLow,
            PushUrgency::Low => Urgency::Low,
            PushUrgency::High => Urgency::High,
            _ => Urgency::Normal,
        }
    }

    /// The asked properties of one email, in the asked order. A preview
    /// that isn't known is left out (not an empty string).
    fn object(&self, email: &StoredEmail) -> Vec<(String, Value)> {
        let Ok(built) = listed(email, &self.properties) else {
            return Vec::new();
        };
        let Ok(Value::Object(mut all)) = serde_json::to_value(built) else {
            return Vec::new();
        };
        self.properties
            .iter()
            .filter(|p| **p != EmailField::Preview || email.meta.preview.is_some())
            .filter_map(|p| {
                let key = p.as_str();
                all.remove(key).map(|value| (key.to_owned(), value))
            })
            .collect()
    }
}

fn json_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// The `EmailPush` of these delivered emails (oldest first), at most
/// `limit` bytes of JSON; `None` if no email passes the filter (nothing is
/// sent) or none fits.
pub(crate) fn build(
    config: &EmailPush,
    account: &JmapAccountId,
    state: Option<State>,
    emails: &[StoredEmail],
    limit: usize,
) -> Option<Vec<u8>> {
    let mut push = EmailPushObject {
        account_id: account.clone(),
        emails: Vec::new(),
        state,
    };
    let mut used = json_len(&push);
    for email in emails.iter().filter(|e| config.matches(e)) {
        // `{}` and the comma before it, if not the first.
        let comma = usize::from(!push.emails.is_empty());
        let mut size = used.saturating_add(2).saturating_add(comma);
        let mut object = Map::new();
        for (key, value) in config.object(email) {
            let entry = json_len(&key)
                .saturating_add(1)
                .saturating_add(json_len(&value))
                .saturating_add(usize::from(!object.is_empty()));
            // As Stalwart: what doesn't fit is left out, the next may fit.
            if size.saturating_add(entry) <= limit {
                size = size.saturating_add(entry);
                object.insert(key, value);
            }
        }
        if !object.is_empty() {
            used = size;
            push.emails.push(object);
        }
    }
    if push.emails.is_empty() {
        return None;
    }
    serde_json::to_vec(&push)
        .ok()
        .filter(|bytes| bytes.len() <= limit)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use nebula_jmap_proto::object::email::EmailAddress;
    use nebula_store::{EmailId, EmailMeta, MailboxId, ThreadId, UnixTime};

    use super::*;

    fn own() -> JmapAccountId {
        "a1".parse().unwrap()
    }

    fn request(value: &Value) -> Result<Option<EmailPush>, SetError> {
        EmailPush::from_request(&own(), EmailPushConfig::parse_map(value)?)
    }

    fn email(id: i64, mailbox: i64, keywords: &[&str], subject: &str) -> StoredEmail {
        StoredEmail {
            id: EmailId::from_raw(id).unwrap(),
            thread: ThreadId::from_raw(id).unwrap(),
            server_id: None,
            mailboxes: BTreeSet::from([MailboxId::from_raw(mailbox).unwrap()]),
            keywords: keywords.iter().map(|k| k.parse().unwrap()).collect(),
            meta: EmailMeta {
                message_id: Some(format!("{id}@example.com")),
                in_reply_to: Vec::new(),
                references: Vec::new(),
                subject: Some(subject.to_owned()),
                from: Some(vec![EmailAddress {
                    name: Some("Alice".to_owned()),
                    email: "alice@example.com".to_owned(),
                }]),
                to: None,
                cc: None,
                bcc: None,
                reply_to: None,
                sender: None,
                sent_at: None,
                received_at: UnixTime::from_secs(1_793_000_000),
                size: 1234,
                has_attachment: false,
                preview: None,
                headers: b"Subject: secret header block\r\n".to_vec(),
            },
        }
    }

    /// What `apps/web/src/mail/email-push.ts` asks for.
    fn web() -> EmailPush {
        request(&json!({"a1": {
            "filter": {"inMailbox": "m1", "notKeyword": "$seen"},
            "properties": ["id", "from", "subject", "receivedAt", "keywords", "mailboxIds"],
            "urgency": "normal"
        }}))
        .unwrap()
        .unwrap()
    }

    #[test]
    fn validation() {
        let config = web();
        assert_eq!(config.urgency(), Urgency::Normal);
        // Shown as Stalwart shows it, and read back the same.
        let shown = config.to_json(&own());
        assert_eq!(
            shown,
            json!({"a1": {
                "filter": {"operator": "AND", "conditions": [
                    {"inMailbox": "m1"}, {"notKeyword": "$seen"}]},
                "properties": ["id", "from", "subject", "receivedAt", "keywords", "mailboxIds"],
                "urgency": "normal"
            }})
        );
        assert_eq!(EmailPush::from_json(&own(), &shown), Some(config));
        assert!(request(&Value::Null).unwrap().is_none());
        assert!(request(&json!({})).unwrap().is_none());
        // Defaults: every delivered email, its id, normal.
        let bare = request(&json!({"a1": {}})).unwrap().unwrap();
        assert_eq!(bare.properties, [EmailField::Id]);
        assert!(bare.filter.is_none());
        let urgent = request(&json!({"a1": {"urgency": "very-low"}}))
            .unwrap()
            .unwrap();
        assert_eq!(urgent.urgency(), Urgency::VeryLow);
        // Operators and every supported condition.
        let tree = json!({"a1": {"properties": ["subject", "preview"], "filter": {
        "operator": "OR", "conditions": [
            {"inMailboxOtherThan": ["m2"], "before": "2027-01-01T00:00:00Z",
             "after": "2026-01-01T00:00:00Z", "minSize": 1, "maxSize": 10,
             "hasKeyword": "$flagged", "hasAttachment": true},
            {"operator": "NOT", "conditions": [{"notKeyword": "$seen"}]}
        ]}}});
        let tree = request(&tree).unwrap().unwrap();
        assert!(tree.wants_preview());
        let again = tree.to_json(&own());
        assert_eq!(EmailPush::from_json(&own(), &again), Some(tree));

        // Another account: forbidden.
        assert_eq!(
            request(&json!({"a2": {}})).unwrap_err().kind(),
            SetErrorType::Forbidden
        );
        // Unsupported: invalidProperties, never ignored.
        for bad in [
            json!({"a1": {"properties": ["textBody"]}}),
            json!({"a1": {"properties": ["bodyValues"]}}),
            json!({"a1": {"properties": ["headers"]}}),
            json!({"a1": {"properties": ["header:Subject:asText"]}}),
            json!({"a1": {"properties": ["id", "attachments"]}}),
            json!({"a1": {"filter": {"text": "x"}}}),
            json!({"a1": {"filter": {"from": "x"}}}),
            json!({"a1": {"filter": {"subject": "x"}}}),
            json!({"a1": {"filter": {"body": "x"}}}),
            json!({"a1": {"filter": {"header": ["X-Spam"]}}}),
            json!({"a1": {"filter": {"someInThreadHaveKeyword": "$flagged"}}}),
            json!({"a1": {"filter": {"operator": "AND", "conditions": [{"cc": "x"}]}}}),
            json!({"a1": {"properties": ["nonsense"]}}),
            json!({"a1": {"urgency": "now"}}),
        ] {
            let error = request(&bad).unwrap_err();
            assert_eq!(error.kind(), SetErrorType::InvalidProperties, "{bad}");
        }
    }

    #[test]
    fn filter() {
        let config = web();
        assert!(config.matches(&email(1, 1, &[], "Hi")));
        assert!(!config.matches(&email(1, 1, &["$seen"], "Hi")));
        assert!(!config.matches(&email(1, 2, &[], "Hi")));
        let not = request(&json!({"a1": {"filter": {"operator": "NOT", "conditions": [
            {"inMailbox": "m1"}, {"hasKeyword": "$junk"}]}}}))
        .unwrap()
        .unwrap();
        assert!(!not.matches(&email(1, 1, &[], "Hi")));
        assert!(!not.matches(&email(1, 2, &["$junk"], "Hi")));
        assert!(not.matches(&email(1, 2, &[], "Hi")));
        let other = request(&json!({"a1": {"filter": {"inMailboxOtherThan": ["m1"],
            "minSize": 1000, "maxSize": 2000, "after": "2026-10-01T00:00:00Z",
            "hasAttachment": false}}}))
        .unwrap()
        .unwrap();
        assert!(other.matches(&email(1, 2, &[], "Hi")));
        assert!(!other.matches(&email(1, 1, &[], "Hi")));
        let small = request(&json!({"a1": {"filter": {"maxSize": 1234}}}))
            .unwrap()
            .unwrap();
        assert!(!small.matches(&email(1, 1, &[], "Hi")));
        // No filter: every email.
        let all = request(&json!({"a1": {}})).unwrap().unwrap();
        assert!(all.matches(&email(1, 9, &["$seen"], "Hi")));
    }

    #[test]
    fn format() {
        let config = web();
        let emails = [
            email(7, 1, &[], "Hola"),
            email(8, 1, &["$seen"], "Read"),
            email(9, 2, &[], "Elsewhere"),
        ];
        let bytes = build(
            &config,
            &own(),
            Some(State::new("s5")),
            &emails,
            nebula_webpush::ece::MAX_PLAINTEXT,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            json!({"@type": "EmailPush", "accountId": "a1", "state": "s5", "emails": [{
                "id": "e7",
                "from": [{"name": "Alice", "email": "alice@example.com"}],
                "subject": "Hola",
                "receivedAt": "2026-10-26T07:33:20Z",
                "keywords": {},
                "mailboxIds": {"m1": true}
            }]})
        );
        // Asked order kept; never the body, the preview or the headers.
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.find("\"id\"") < text.find("\"subject\""));
        assert!(!text.contains("preview") && !text.contains("secret header"));
        // A known preview goes only if asked for.
        let mut with_preview = email(7, 1, &[], "Hola");
        with_preview.meta.preview = Some("the text".to_owned());
        let preview = request(&json!({"a1": {"properties": ["id", "preview"]}}))
            .unwrap()
            .unwrap();
        let bytes = build(&preview, &own(), None, &[with_preview.clone()], 4000).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["emails"],
            json!([{"id": "e7", "preview": "the text"}])
        );
        assert!(value.get("state").is_none());
        let bytes = build(&config, &own(), None, &[with_preview], 4000).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("the text"));
        // Nothing passes the filter: nothing to send.
        assert!(build(&config, &own(), None, &emails[1..], 4000).is_none());
    }

    #[test]
    fn size() {
        let config = web();
        // Many emails with long subjects: never more than the limit, and
        // what doesn't fit is left out property by property.
        let long = "x".repeat(700);
        let emails: Vec<StoredEmail> = (1..=20).map(|i| email(i, 1, &[], &long)).collect();
        let limit = nebula_webpush::ece::MAX_PLAINTEXT;
        let bytes = build(&config, &own(), Some(State::new("s9")), &emails, limit).unwrap();
        assert!(bytes.len() <= limit);
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let sent = value["emails"].as_array().unwrap();
        // The first ones have it all; later ones lose the long subject
        // and keep the rest; once nothing fits, emails are left out.
        assert!((2..20).contains(&sent.len()), "{}", sent.len());
        assert!(sent.iter().all(|e| e.get("id").is_some()));
        assert!(sent.iter().any(|e| e.get("subject").is_none()));
        assert!(sent.first().unwrap().get("subject").is_some());
        // Exactly at the limit, with a tiny one.
        for limit in [60, 90, 120, 200, 300] {
            if let Some(bytes) = build(&config, &own(), None, &emails, limit) {
                assert!(bytes.len() <= limit, "{limit}");
            }
        }
        // The wrapper alone doesn't fit: nothing.
        assert!(build(&config, &own(), None, &emails, 20).is_none());
    }
}
