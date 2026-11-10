// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `ContactCard` (RFC 9610 §3): a JSContact `Card` (kept as a bounded
//! [`JsBody`]) plus `id` and `addressBookIds`.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::email::SetPatch;
use super::email::patch::Members;
use super::js_body::{BodyPatch, JsBody, JsContact, JsPropertyName, JsPropertyNameError};
use crate::error::{MethodError, SetError, SetErrorType};
use crate::macros::serde_via_str;
use crate::method::parse::{PropertyErrors, from_arguments};
use crate::method::patch::Patch;
use crate::method::{
    FilterCondition, PatchObject, QueryArguments, QueryChangesArguments, SortProperty,
};
use crate::references::IdOrRef;
use crate::types::serde_util::{empty_map_as_null, empty_vec_as_null};
use crate::types::{AccountId, AddressBookId, BlobId, ContactCardId, TrueSet, UtcDate};

/// A `ContactCard` property name: `id`, `addressBookIds` or any JSContact
/// member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ContactCardProperty {
    /// `id`.
    Id,
    /// `addressBookIds`.
    AddressBookIds,
    /// A JSContact member (`name`, `emails`, `uid`…).
    Body(JsPropertyName),
}

impl ContactCardProperty {
    /// The property name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Id => "id",
            Self::AddressBookIds => "addressBookIds",
            Self::Body(name) => name.as_str(),
        }
    }
}

impl FromStr for ContactCardProperty {
    type Err = JsPropertyNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "id" => Ok(Self::Id),
            "addressBookIds" => Ok(Self::AddressBookIds),
            other => other.parse().map(Self::Body),
        }
    }
}

impl fmt::Display for ContactCardProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

serde_via_str!(ContactCardProperty);

/// A `ContactCard` as returned by `/get`, `/parse` and `/set` (`created`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCard {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<ContactCardId>,
    /// `addressBookIds`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address_book_ids: Option<TrueSet<AddressBookId>>,
    /// The JSContact members.
    #[serde(flatten)]
    pub body: JsBody<JsContact>,
}

fn non_empty_books(
    errors: &mut PropertyErrors,
    key: &str,
    value: Value,
) -> Option<TrueSet<IdOrRef<AddressBookId>>> {
    let set = errors.parse::<TrueSet<IdOrRef<AddressBookId>>>(key, value)?;
    if set.is_empty() {
        errors.invalid_because(key, "must not be empty");
        return None;
    }
    Some(set)
}

/// A card to create (`ContactCard/set` create).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactCardCreate {
    /// The books it goes in (at least one; `#creationId` allowed).
    pub address_book_ids: TrueSet<IdOrRef<AddressBookId>>,
    /// The JSContact card.
    pub body: JsBody<JsContact>,
}

impl ContactCardCreate {
    /// Parses one object of `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every invalid property, `id`, and
    /// `addressBookIds` if missing or empty.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut address_book_ids = None;
        let mut body = Map::new();
        for (key, value) in object {
            match key.as_str() {
                "addressBookIds" => address_book_ids = non_empty_books(&mut errors, &key, value),
                "id" => errors.invalid_because(&key, "server-set"),
                _ => {
                    body.insert(key, value);
                }
            }
        }
        let body = match JsBody::new(body) {
            Ok(body) => Some(body),
            Err(error) => {
                for (name, why) in error.problems() {
                    errors.invalid_because(name.as_str(), why.as_str());
                }
                None
            }
        };
        let address_book_ids = errors.required("addressBookIds", address_book_ids);
        errors.finish()?;
        Ok(Self {
            address_book_ids: address_book_ids.unwrap_or_default(),
            body: body.unwrap_or_default(),
        })
    }
}

/// A `ContactCard/set` update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContactCardPatch {
    /// Move to other books (never to none).
    pub address_book_ids: Option<SetPatch<IdOrRef<AddressBookId>>>,
    /// Changes to the JSContact card (apply with [`JsBody::apply`]).
    pub body: Vec<BodyPatch>,
}

impl ContactCardPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidPatch` for bad or overlapping paths; `invalidProperties`
    /// for `id` and bad values.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut out = Self::default();
        let mut members = Members::<IdOrRef<AddressBookId>>::default();
        for Patch { key, path, value } in patch.patches()? {
            match path.tokens() {
                [one] if one == "addressBookIds" => {
                    out.address_book_ids = non_empty_books(&mut errors, &key, value)
                        .map(|set| SetPatch::Replace(set.into_set()));
                }
                [one, member] if one == "addressBookIds" => members.record(&key, member, &value)?,
                [one, ..] if one == "addressBookIds" => {
                    return Err(SetError::invalid_patch(format!("{key:?} goes too deep")));
                }
                [one, ..] if one == "id" => errors.invalid_because(&key, "server-set"),
                _ => match BodyPatch::parse::<JsContact>(&key, path, value) {
                    Ok(body) => out.body.push(body),
                    Err(error) if error.kind() == SetErrorType::InvalidPatch => return Err(error),
                    Err(error) => {
                        errors.invalid_because(&key, error.description().unwrap_or("invalid"));
                    }
                },
            }
        }
        errors.finish()?;
        if out.address_book_ids.is_none() {
            out.address_book_ids = members.into_patch();
        }
        Ok(out)
    }
}

/// `ContactCard` `FilterCondition` (RFC 9610 §3.3). Which ones are
/// supported is up to the implementation (`unsupportedFilter`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactCardFilterCondition {
    /// In this address book.
    #[serde(default)]
    pub in_address_book: Option<AddressBookId>,
    /// The `uid` is exactly this.
    #[serde(default)]
    pub uid: Option<String>,
    /// A group with this member (uid).
    #[serde(default)]
    pub has_member: Option<String>,
    /// `kind` is this.
    #[serde(default)]
    pub kind: Option<String>,
    /// Created before.
    #[serde(default)]
    pub created_before: Option<UtcDate>,
    /// Created at or after.
    #[serde(default)]
    pub created_after: Option<UtcDate>,
    /// Updated before.
    #[serde(default)]
    pub updated_before: Option<UtcDate>,
    /// Updated at or after.
    #[serde(default)]
    pub updated_after: Option<UtcDate>,
    /// Any text property contains this.
    #[serde(default)]
    pub text: Option<String>,
    /// The name contains this.
    #[serde(default)]
    pub name: Option<String>,
    /// A given name contains this.
    #[serde(default, rename = "name/given")]
    pub name_given: Option<String>,
    /// A surname contains this.
    #[serde(default, rename = "name/surname")]
    pub name_surname: Option<String>,
    /// A second surname contains this.
    #[serde(default, rename = "name/surname2")]
    pub name_surname2: Option<String>,
    /// A nickname contains this.
    #[serde(default)]
    pub nickname: Option<String>,
    /// An organization contains this.
    #[serde(default)]
    pub organization: Option<String>,
    /// An email address contains this.
    #[serde(default)]
    pub email: Option<String>,
    /// A phone number contains this.
    #[serde(default)]
    pub phone: Option<String>,
    /// An online service contains this.
    #[serde(default)]
    pub online_service: Option<String>,
    /// An address contains this.
    #[serde(default)]
    pub address: Option<String>,
    /// A note contains this.
    #[serde(default)]
    pub note: Option<String>,
}

impl FilterCondition for ContactCardFilterCondition {
    const PROPERTIES: &'static [&'static str] = &[
        "inAddressBook",
        "uid",
        "hasMember",
        "kind",
        "createdBefore",
        "createdAfter",
        "updatedBefore",
        "updatedAfter",
        "text",
        "name",
        "name/given",
        "name/surname",
        "name/surname2",
        "nickname",
        "organization",
        "email",
        "phone",
        "onlineService",
        "address",
        "note",
    ];
}

/// `ContactCard` sort properties (RFC 9610 §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContactCardSort {
    /// `created`.
    Created,
    /// `updated`.
    Updated,
    /// `name/given`.
    NameGiven,
    /// `name/surname`.
    NameSurname,
    /// `name/surname2`.
    NameSurname2,
}

impl SortProperty for ContactCardSort {
    fn parse(property: &str, _extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        Ok(match property {
            "created" => Self::Created,
            "updated" => Self::Updated,
            "name/given" => Self::NameGiven,
            "name/surname" => Self::NameSurname,
            "name/surname2" => Self::NameSurname2,
            other => {
                return Err(MethodError::unsupported_sort(format!(
                    "cannot sort cards by {other:?}"
                )));
            }
        })
    }
}

/// `ContactCard/query` arguments (no extras).
pub type ContactCardQueryArguments =
    QueryArguments<ContactCardFilterCondition, ContactCardSort, ContactCardId>;

/// `ContactCard/queryChanges` arguments (no extras).
pub type ContactCardQueryChangesArguments =
    QueryChangesArguments<ContactCardFilterCondition, ContactCardSort, ContactCardId>;

/// `ContactCard/parse` arguments (`urn:ietf:params:jmap:contacts:parse`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContactCardParseArguments {
    /// The account.
    pub account_id: AccountId,
    /// The vCard blobs to read.
    pub blob_ids: Vec<BlobId>,
    /// Properties to return; `None`: all.
    #[serde(default)]
    pub properties: Option<Vec<ContactCardProperty>>,
}

impl ContactCardParseArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`, or `requestTooLarge` beyond `max_objects`
    /// blobs.
    pub fn parse(arguments: Map<String, Value>, max_objects: usize) -> Result<Self, MethodError> {
        let parsed: Self = from_arguments(arguments)?;
        if parsed.blob_ids.len() > max_objects {
            return Err(MethodError::request_too_large("too many blobs"));
        }
        Ok(parsed)
    }
}

/// `ContactCard/parse` response: one card per blob (the first vCard, as
/// Stalwart does).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactCardParseResponse {
    /// The account.
    pub account_id: AccountId,
    /// Blob → its card.
    #[serde(serialize_with = "empty_map_as_null")]
    pub parsed: BTreeMap<BlobId, ContactCard>,
    /// Blobs that are not vCards.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub not_parsable: Vec<BlobId>,
    /// Blobs that don't exist.
    #[serde(serialize_with = "empty_vec_as_null")]
    pub not_found: Vec<BlobId>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;
    use crate::method::{Filter, GetArguments};

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// `listContacts` (contacts/api.ts and mail/contacts.ts) and
    /// `getContact`.
    #[test]
    fn web_query_and_get() {
        let query = ContactCardQueryArguments::parse(map(json!({
            "accountId": "c", "calculateTotal": true, "limit": 200, "position": 0
        })))
        .unwrap();
        assert!(query.calculate_total && query.filter.is_none());
        let get = GetArguments::<ContactCardId, ContactCardProperty>::parse(
            map(json!({"accountId": "c", "ids": ["b"],
                "properties": ["uid", "kind", "name", "emails", "phones", "organizations",
                               "addressBookIds", "members"]})),
            500,
        )
        .unwrap();
        assert!(
            get.properties
                .unwrap()
                .contains(&ContactCardProperty::AddressBookIds)
        );
        let filtered = ContactCardQueryArguments::parse(map(json!({
            "accountId": "c",
            "filter": {"inAddressBook": "b", "text": "bob", "name/given": "Bob",
                       "createdAfter": "2026-01-01T00:00:00Z"},
            "sort": [{"property": "name/surname"}, {"property": "updated", "isAscending": false}]
        })))
        .unwrap();
        let Some(Filter::Condition(condition)) = filtered.filter else {
            panic!("expected a condition")
        };
        assert_eq!(condition.name_given.as_deref(), Some("Bob"));
        assert_eq!(filtered.sort[0].property, ContactCardSort::NameSurname);

        let kind = |value: Value| {
            ContactCardQueryArguments::parse(map(value))
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"accountId": "c", "filter": {"fuzzy": 1}})),
            MethodErrorType::UnsupportedFilter
        );
        assert_eq!(
            kind(json!({"accountId": "c", "sort": [{"property": "name"}]})),
            MethodErrorType::UnsupportedSort
        );
        assert_eq!(
            kind(json!({"accountId": "c", "filter": {"createdAfter": "yesterday"}})),
            MethodErrorType::InvalidArguments
        );
        assert_eq!(
            kind(json!({"accountId": "c", "expandRecurrences": true})),
            MethodErrorType::InvalidArguments
        );
    }

    /// Stalwart 1.0's `ContactCard/get` (2026-10-27).
    #[test]
    fn get_response_round_trip() {
        let stalwart = json!({
            "@type": "Card", "version": "1.0",
            "emails": {"e1": {"address": "bob@nebulamail.test", "@type": "EmailAddress"}},
            "kind": "individual",
            "name": {"full": "Bob Pérez", "components": [{"kind": "given", "value": "Bob"},
                {"kind": "surname", "value": "Pérez"}], "isOrdered": true, "@type": "Name"},
            "id": "c", "addressBookIds": {"b": true}
        });
        let mut members = map(stalwart.clone());
        let card = ContactCard {
            id: Some("c".parse().unwrap()),
            address_book_ids: Some(
                serde_json::from_value(members.remove("addressBookIds").unwrap()).unwrap(),
            ),
            body: {
                members.remove("id");
                JsBody::new(members).unwrap()
            },
        };
        assert_eq!(serde_json::to_value(&card).unwrap(), stalwart);
    }

    /// `newCard` (contacts/card.ts) and the import of parsed cards.
    #[test]
    fn web_create() {
        let created = ContactCardCreate::parse(map(json!({
            "@type": "Card", "addressBookIds": {"b": true}, "kind": "individual",
            "uid": "urn:uuid:5c1e7a3e-0b55-4bb4-a1a4-0a6c2d1f8b11", "version": "1.0",
            "name": {"@type": "Name", "full": "Carla Gómez",
                     "components": [{"kind": "given", "value": "Carla"}]},
            "emails": {"e1": {"@type": "EmailAddress", "address": "carla@example.com",
                              "contexts": {"work": true}}},
            "phones": {"p1": {"number": "+34 600 000 000", "features": {"mobile": true}}},
            "notes": {"n1": {"note": "Línea 1\nLínea 2"}},
            "anniversaries": {"a1": {"kind": "birth", "date": {"@type": "PartialDate", "month": 3, "day": 14}}},
            "media": {"m1": {"kind": "photo", "uri": "data:image/png;base64,iVBORw0KGgo="}}
        })))
        .unwrap();
        assert_eq!(
            created.body.uid(),
            Some("urn:uuid:5c1e7a3e-0b55-4bb4-a1a4-0a6c2d1f8b11")
        );
        assert!(created.body.get("addressBookIds").is_none());
        let in_new_book =
            ContactCardCreate::parse(map(json!({"addressBookIds": {"#book": true}}))).unwrap();
        assert_eq!(in_new_book.address_book_ids.len(), 1);

        let error = ContactCardCreate::parse(map(json!({
            "id": "x", "addressBookIds": {}, "@type": "Event", "uid": 3, "created": "today"
        })))
        .unwrap_err();
        let mut properties = error.properties().to_vec();
        properties.sort();
        assert_eq!(
            properties,
            ["@type", "addressBookIds", "created", "id", "uid"]
        );
        assert_eq!(
            ContactCardCreate::parse(map(json!({"name": {"full": "x"}})))
                .unwrap_err()
                .properties(),
            ["addressBookIds"]
        );
    }

    /// `updateContact` (`formProperties`) and `setGroupMembers`.
    #[test]
    fn web_patches() {
        let parse = |value: Value| ContactCardPatch::parse(&serde_json::from_value(value).unwrap());
        let form = parse(json!({
            "addresses": null, "anniversaries": null,
            "emails": {"e1": {"@type": "EmailAddress", "address": "new@example.com"}},
            "kind": "individual", "media": null, "members": null,
            "name": {"@type": "Name", "full": "Carla"},
            "notes": {"n1": {"note": "x"}}, "organizations": null,
            "phones": null, "titles": {"t1": {"kind": "title", "name": "CTO"}}
        }))
        .unwrap();
        assert_eq!(form.body.len(), 11);
        assert!(form.address_book_ids.is_none());
        let group = parse(json!({"members": {"urn:uuid:1": true, "urn:uuid:2": true}})).unwrap();
        assert_eq!(group.body.len(), 1);
        let moved = parse(json!({"addressBookIds/b": null, "addressBookIds/b2": true, "emails/e1/address": "x@y"}))
            .unwrap();
        assert!(matches!(
            moved.address_book_ids,
            Some(SetPatch::Change { .. })
        ));

        let kind = |value: Value| parse(value).unwrap_err().kind();
        assert_eq!(kind(json!({"id": "x"})), SetErrorType::InvalidProperties);
        assert_eq!(
            kind(json!({"addressBookIds": {}})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"addressBookIds/b/x": true})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(
            kind(json!({"@type": "Group"})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"version": null})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"emails": {}, "emails/e1": null})),
            SetErrorType::InvalidPatch
        );
    }

    /// `parseVcf` in contacts/api.ts, and the response.
    #[test]
    fn parse_arguments_and_response() {
        let args = ContactCardParseArguments::parse(
            map(json!({"accountId": "c", "blobIds": ["G1", "G2"]})),
            10,
        )
        .unwrap();
        assert_eq!(args.blob_ids.len(), 2);
        assert!(
            ContactCardParseArguments::parse(
                map(json!({"accountId": "c", "blobIds": ["G1"], "x": 1})),
                10
            )
            .is_err()
        );
        assert_eq!(
            ContactCardParseArguments::parse(
                map(json!({"accountId": "c", "blobIds": ["G1", "G2"]})),
                1
            )
            .unwrap_err()
            .kind(),
            MethodErrorType::RequestTooLarge
        );
        let response = ContactCardParseResponse {
            account_id: "c".parse().unwrap(),
            parsed: BTreeMap::from([(
                "G1".parse().unwrap(),
                ContactCard {
                    body: JsBody::new(map(
                        json!({"@type": "Card", "version": "1.0", "name": {"full": "X"}}),
                    ))
                    .unwrap(),
                    ..ContactCard::default()
                },
            )]),
            not_parsable: Vec::new(),
            not_found: vec!["G2".parse().unwrap()],
        };
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            json!({
                "accountId": "c",
                "parsed": {"G1": {"@type": "Card", "version": "1.0", "name": {"full": "X"}}},
                "notParsable": null,
                "notFound": ["G2"]
            })
        );
    }
}
