// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mailbox (RFC 8621 §2).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{MethodError, SetError};
use crate::macros::string_enum;
use crate::method::parse::{PropertyErrors, take_argument};
use crate::method::patch::{Patch, Target};
use crate::method::{FilterCondition, PatchObject, QueryArguments, SetArguments, SortProperty};
use crate::references::IdOrRef;
use crate::types::serde_util::double_option;
use crate::types::{MailboxId, UnsignedInt};

string_enum! {
    /// A mailbox role (IANA "IMAP Mailbox Name Attributes", lowercased).
    pub enum MailboxRole ("mailbox role") {
        /// All messages (`\All`).
        All = "all",
        /// Archive (`\Archive`).
        Archive = "archive",
        /// Drafts (`\Drafts`).
        Drafts = "drafts",
        /// Flagged messages (`\Flagged`).
        Flagged = "flagged",
        /// Important messages (`\Important`, RFC 8457).
        Important = "important",
        /// The inbox.
        Inbox = "inbox",
        /// Spam (`\Junk`).
        Junk = "junk",
        /// Sent messages (`\Sent`).
        Sent = "sent",
        /// Subscribed mailboxes (`\Subscribed`).
        Subscribed = "subscribed",
        /// Trash (`\Trash`).
        Trash = "trash",
        /// Snoozed messages (`\Snoozed`).
        Snoozed = "snoozed",
        /// Scheduled to send later (`\Scheduled`).
        Scheduled = "scheduled",
        /// Notes to self (`\Memos`).
        Memos = "memos",
    }
}

/// What the user may do in a mailbox (RFC 8621 §2 `myRights`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wire format is exactly these nine booleans"
)]
pub struct MailboxRights {
    /// May read the emails in it.
    pub may_read_items: bool,
    /// May add emails to it.
    pub may_add_items: bool,
    /// May remove emails from it.
    pub may_remove_items: bool,
    /// May set or clear `$seen`.
    pub may_set_seen: bool,
    /// May set or clear other keywords.
    pub may_set_keywords: bool,
    /// May create child mailboxes.
    pub may_create_child: bool,
    /// May rename it.
    pub may_rename: bool,
    /// May delete it.
    pub may_delete: bool,
    /// May send email from it.
    pub may_submit: bool,
}

impl MailboxRights {
    /// Every right.
    pub const ALL: Self = Self {
        may_read_items: true,
        may_add_items: true,
        may_remove_items: true,
        may_set_seen: true,
        may_set_keywords: true,
        may_create_child: true,
        may_rename: true,
        may_delete: true,
        may_submit: true,
    };
}

string_enum! {
    /// Mailbox properties (for `properties` and `updatedProperties`).
    pub enum MailboxProperty ("Mailbox property") {
        /// `id`.
        Id = "id",
        /// `name`.
        Name = "name",
        /// `parentId`.
        ParentId = "parentId",
        /// `role`.
        Role = "role",
        /// `sortOrder`.
        SortOrder = "sortOrder",
        /// `totalEmails`.
        TotalEmails = "totalEmails",
        /// `unreadEmails`.
        UnreadEmails = "unreadEmails",
        /// `totalThreads`.
        TotalThreads = "totalThreads",
        /// `unreadThreads`.
        UnreadThreads = "unreadThreads",
        /// `myRights`.
        MyRights = "myRights",
        /// `isSubscribed`.
        IsSubscribed = "isSubscribed",
        /// The bridge's vendor property: see [`IMAP_NAME_PROPERTY`].
        ImapName = "https://helixnebula.tech/jmap/bridge/imapName",
    }
}

/// The folder's full name on the IMAP server, decoded to UTF-8, with the
/// server's own separator and namespace prefix (`INBOX.Lists.Rust`). Sieve
/// on that server names folders this way (`fileinto`), so it is the only
/// form that is right on every server. Only on the bridge,
/// read-only.
pub const IMAP_NAME_PROPERTY: &str = "https://helixnebula.tech/jmap/bridge/imapName";

/// A Mailbox as returned by `Mailbox/get` (and in `/set` `created`).
///
/// Every property is optional because only the requested ones are sent;
/// `None` means "not sent". For nullable properties, `Some(None)` is sent
/// as `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<MailboxId>,
    /// `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `parentId` (`null` for top-level mailboxes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Option<MailboxId>>,
    /// `role` (`null` if none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Option<MailboxRole>>,
    /// `sortOrder`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<UnsignedInt>,
    /// `totalEmails`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_emails: Option<UnsignedInt>,
    /// `unreadEmails`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_emails: Option<UnsignedInt>,
    /// `totalThreads`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_threads: Option<UnsignedInt>,
    /// `unreadThreads`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_threads: Option<UnsignedInt>,
    /// `myRights`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<MailboxRights>,
    /// `isSubscribed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// [`IMAP_NAME_PROPERTY`].
    #[serde(
        rename = "https://helixnebula.tech/jmap/bridge/imapName",
        skip_serializing_if = "Option::is_none"
    )]
    pub imap_name: Option<String>,
}

/// Properties only the server sets.
const SERVER_SET: &[&str] = &[
    "id",
    IMAP_NAME_PROPERTY,
    "totalEmails",
    "unreadEmails",
    "totalThreads",
    "unreadThreads",
    "myRights",
];

/// A mailbox to create (`Mailbox/set` create).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxCreate {
    /// The name (not empty, no control characters).
    pub name: String,
    /// The parent, or `None` for a top-level mailbox.
    pub parent_id: Option<IdOrRef<MailboxId>>,
    /// The role, if any.
    pub role: Option<MailboxRole>,
    /// Sort order (default 0).
    pub sort_order: UnsignedInt,
    /// Whether subscribed; `None` lets the server choose.
    pub is_subscribed: Option<bool>,
}

fn check_name(errors: &mut PropertyErrors, name: Option<String>) -> Option<String> {
    let name = name?;
    if name.is_empty() || name.chars().any(char::is_control) {
        errors.invalid_because("name", "must not be empty or contain control characters");
        return None;
    }
    Some(name)
}

impl MailboxCreate {
    /// Parses one object of `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every invalid, unknown or server-set
    /// property.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut name = None;
        let mut parent_id = None;
        let mut role = None;
        let mut sort_order = None;
        let mut is_subscribed = None;
        for (key, value) in object {
            match key.as_str() {
                "name" => name = errors.parse::<String>(&key, value),
                "parentId" => parent_id = errors.parse(&key, value),
                "role" => role = errors.parse(&key, value),
                "sortOrder" => sort_order = errors.parse(&key, value),
                "isSubscribed" => is_subscribed = errors.parse(&key, value),
                _ => errors.invalid(key),
            }
        }
        let name = check_name(&mut errors, name);
        let name = errors.required("name", name);
        errors.finish()?;
        Ok(Self {
            name: name.unwrap_or_default(),
            parent_id: parent_id.flatten(),
            role: role.flatten(),
            sort_order: sort_order.flatten().unwrap_or_default(),
            is_subscribed: is_subscribed.flatten(),
        })
    }
}

/// A `Mailbox/set` update. `None` fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailboxPatch {
    /// New name.
    pub name: Option<String>,
    /// New parent (`Some(None)`: move to the top level).
    pub parent_id: Option<Option<IdOrRef<MailboxId>>>,
    /// New role (`Some(None)`: remove the role).
    pub role: Option<Option<MailboxRole>>,
    /// New sort order (`null` resets to 0).
    pub sort_order: Option<UnsignedInt>,
    /// Subscribe or unsubscribe.
    pub is_subscribed: Option<bool>,
}

impl MailboxPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidPatch` for bad paths; `invalidProperties` for unknown,
    /// server-set or badly typed properties.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut out = Self::default();
        let mut errors = PropertyErrors::new();
        for Patch { key, path, value } in patch.patches()? {
            let property = match Target::of(&path) {
                Target::Property(property) => property,
                Target::Member(property, _) | Target::Deeper(property) => {
                    if is_known(property) {
                        return Err(SetError::invalid_patch(format!(
                            "{key:?}: {property} has no members to patch"
                        )));
                    }
                    errors.invalid(key);
                    continue;
                }
            };
            match property {
                "name" => {
                    let name = errors.parse(property, value);
                    out.name = check_name(&mut errors, name);
                }
                "parentId" => out.parent_id = errors.parse(property, value),
                "role" => out.role = errors.parse(property, value),
                "sortOrder" => {
                    out.sort_order = errors
                        .parse::<Option<UnsignedInt>>(property, value)
                        .map(Option::unwrap_or_default);
                }
                // Not nullable and without a default: `null` is invalid.
                "isSubscribed" => out.is_subscribed = errors.parse(property, value),
                _ => errors.invalid(property),
            }
        }
        errors.finish()?;
        Ok(out)
    }
}

fn is_known(property: &str) -> bool {
    MailboxProperty::ALL
        .iter()
        .any(|known| known.as_str() == property)
        || SERVER_SET.contains(&property)
}

/// Mailbox `FilterCondition` (RFC 8621 §2.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailboxFilterCondition {
    /// Parent id (`Some(None)`: top-level mailboxes only).
    #[serde(default, deserialize_with = "double_option")]
    pub parent_id: Option<Option<MailboxId>>,
    /// Name contains this.
    #[serde(default)]
    pub name: Option<String>,
    /// Has this role (`Some(None)`: has no role).
    #[serde(default, deserialize_with = "double_option")]
    pub role: Option<Option<MailboxRole>>,
    /// Has any role / has none.
    #[serde(default)]
    pub has_any_role: Option<bool>,
    /// Is subscribed / is not.
    #[serde(default)]
    pub is_subscribed: Option<bool>,
}

impl FilterCondition for MailboxFilterCondition {
    const PROPERTIES: &'static [&'static str] =
        &["parentId", "name", "role", "hasAnyRole", "isSubscribed"];
}

/// Mailbox sort properties (RFC 8621 §2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MailboxSort {
    /// `sortOrder`.
    SortOrder,
    /// `name`.
    Name,
}

impl SortProperty for MailboxSort {
    fn parse(property: &str, _extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        match property {
            "sortOrder" => Ok(Self::SortOrder),
            "name" => Ok(Self::Name),
            other => Err(MethodError::unsupported_sort(format!(
                "cannot sort mailboxes by {other:?}"
            ))),
        }
    }
}

/// `Mailbox/query` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxQueryArguments {
    /// The standard arguments.
    pub query: QueryArguments<MailboxFilterCondition, MailboxSort, MailboxId>,
    /// Return parents before children.
    pub sort_as_tree: bool,
    /// A mailbox matches only if its parents match too.
    pub filter_as_tree: bool,
}

impl MailboxQueryArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let sort_as_tree = take_argument(&mut arguments, "sortAsTree")?.unwrap_or(false);
        let filter_as_tree = take_argument(&mut arguments, "filterAsTree")?.unwrap_or(false);
        Ok(Self {
            query: QueryArguments::parse(arguments)?,
            sort_as_tree,
            filter_as_tree,
        })
    }
}

/// `Mailbox/queryChanges` arguments (RFC 8621 §2.4: the standard ones
/// plus the original query's `sortAsTree` and `filterAsTree`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxQueryChangesArguments {
    /// The standard arguments.
    pub query_changes:
        crate::method::QueryChangesArguments<MailboxFilterCondition, MailboxSort, MailboxId>,
    /// The original query's `sortAsTree`.
    pub sort_as_tree: bool,
    /// The original query's `filterAsTree`.
    pub filter_as_tree: bool,
}

impl MailboxQueryChangesArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, `unsupportedFilter` or `unsupportedSort`.
    pub fn parse(mut arguments: Map<String, Value>) -> Result<Self, MethodError> {
        let sort_as_tree = take_argument(&mut arguments, "sortAsTree")?.unwrap_or(false);
        let filter_as_tree = take_argument(&mut arguments, "filterAsTree")?.unwrap_or(false);
        Ok(Self {
            query_changes: crate::method::QueryChangesArguments::parse(arguments)?,
            sort_as_tree,
            filter_as_tree,
        })
    }
}

/// `Mailbox/set` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxSetArguments {
    /// The standard arguments.
    pub set: SetArguments<MailboxId>,
    /// Destroy mailboxes that still have emails (removing the emails from
    /// them), instead of failing with `mailboxHasEmail`.
    pub on_destroy_remove_emails: bool,
}

impl MailboxSetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` or `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let on_destroy_remove_emails =
            take_argument(&mut arguments, "onDestroyRemoveEmails")?.unwrap_or(false);
        Ok(Self {
            set: SetArguments::parse(arguments, max_objects)?,
            on_destroy_remove_emails,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;
    use crate::method::Filter;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// RFC 8621 §2.6 (the Mailbox/get response).
    #[test]
    fn rfc_get_response_shape() {
        let inbox = Mailbox {
            id: Some("MB23cfa8094c0f41e6".parse().unwrap()),
            name: Some("Inbox".to_owned()),
            parent_id: Some(None),
            role: Some(Some(MailboxRole::Inbox)),
            sort_order: Some(UnsignedInt::from(10_u32)),
            total_emails: Some(UnsignedInt::from(16_307_u32)),
            unread_emails: Some(UnsignedInt::from(13_u32)),
            total_threads: Some(UnsignedInt::from(7_778_u32)),
            unread_threads: Some(UnsignedInt::from(7_u32)),
            my_rights: Some(MailboxRights {
                may_create_child: true,
                may_rename: false,
                may_delete: false,
                ..MailboxRights::ALL
            }),
            is_subscribed: Some(true),
            imap_name: None,
        };
        assert_eq!(
            serde_json::to_value(&inbox).unwrap(),
            json!({
                "id": "MB23cfa8094c0f41e6",
                "name": "Inbox",
                "parentId": null,
                "role": "inbox",
                "sortOrder": 10,
                "totalEmails": 16307,
                "unreadEmails": 13,
                "totalThreads": 7778,
                "unreadThreads": 7,
                "myRights": {
                    "mayReadItems": true, "mayAddItems": true, "mayRemoveItems": true,
                    "maySetSeen": true, "maySetKeywords": true, "mayCreateChild": true,
                    "mayRename": false, "mayDelete": false, "maySubmit": true
                },
                "isSubscribed": true
            })
        );
        let only_name = Mailbox {
            id: Some("m1".parse().unwrap()),
            name: Some("x".to_owned()),
            ..Mailbox::default()
        };
        assert_eq!(
            serde_json::to_value(&only_name).unwrap(),
            json!({"id": "m1", "name": "x"})
        );
    }

    #[test]
    fn create() {
        let created = MailboxCreate::parse(map(json!({
            "name": "Projects", "parentId": "#parent", "isSubscribed": true, "role": null
        })))
        .unwrap();
        assert_eq!(
            created.parent_id,
            Some(IdOrRef::Ref("parent".parse().unwrap()))
        );
        assert_eq!(created.sort_order, UnsignedInt::ZERO);
        let error = MailboxCreate::parse(map(json!({
            "parentId": 3, "totalEmails": 0, "role": "nope", "shareWith": {}
        })))
        .unwrap_err();
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        let mut properties = error.properties().to_vec();
        properties.sort();
        assert_eq!(
            properties,
            ["name", "parentId", "role", "shareWith", "totalEmails"]
        );
        assert!(MailboxCreate::parse(map(json!({"name": ""}))).is_err());
        assert!(MailboxCreate::parse(map(json!({"name": "a\nb"}))).is_err());
    }

    #[test]
    fn patch() {
        let patch: PatchObject =
            serde_json::from_value(json!({"name": "New", "parentId": null, "sortOrder": null}))
                .unwrap();
        let parsed = MailboxPatch::parse(&patch).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("New"));
        assert_eq!(parsed.parent_id, Some(None));
        assert_eq!(parsed.sort_order, Some(UnsignedInt::ZERO));
        assert_eq!(parsed.role, None);
        let kind = |value: Value| {
            MailboxPatch::parse(&serde_json::from_value(value).unwrap())
                .unwrap_err()
                .kind()
        };
        assert_eq!(kind(json!({"name": null})), SetErrorType::InvalidProperties);
        assert_eq!(
            kind(json!({"isSubscribed": null})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"unreadEmails": 3})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"shareWith/p1": null})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(kind(json!({"name/x": "y"})), SetErrorType::InvalidPatch);
    }

    #[test]
    fn query() {
        let args = MailboxQueryArguments::parse(map(json!({
            "accountId": "a",
            "filter": {"parentId": null, "hasAnyRole": false},
            "sort": [{"property": "sortOrder"}, {"property": "name"}],
            "sortAsTree": true
        })))
        .unwrap();
        assert!(args.sort_as_tree && !args.filter_as_tree);
        let Some(Filter::Condition(condition)) = args.query.filter else {
            panic!("expected a condition")
        };
        assert_eq!(condition.parent_id, Some(None));
        assert_eq!(condition.has_any_role, Some(false));
        assert_eq!(args.query.sort.len(), 2);
    }

    #[test]
    fn set_arguments() {
        let args = MailboxSetArguments::parse(
            map(json!({"accountId": "a", "destroy": ["m1"], "onDestroyRemoveEmails": true})),
            10,
        )
        .unwrap();
        assert!(args.on_destroy_remove_emails);
        assert_eq!(args.set.destroy.len(), 1);
    }
}
