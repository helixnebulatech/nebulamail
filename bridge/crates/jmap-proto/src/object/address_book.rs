// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `AddressBook` (RFC 9610 §2).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::calendar::{check_description, check_name};
use super::map_patch::{MapMembers, MapPatch};
use crate::error::{MethodError, SetError};
use crate::macros::string_enum;
use crate::method::parse::{PropertyErrors, take_argument};
use crate::method::patch::{Patch, Target};
use crate::method::{PatchObject, SetArguments};
use crate::references::IdOrRef;
use crate::types::{AddressBookId, PrincipalId, UnsignedInt};

/// What a user may do with an address book (RFC 9610 §2,
/// `AddressBookRights`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wire format is exactly these four booleans"
)]
pub struct AddressBookRights {
    /// May read its cards.
    pub may_read: bool,
    /// May create, change and destroy its cards.
    pub may_write: bool,
    /// May share it.
    pub may_share: bool,
    /// May delete the book itself.
    pub may_delete: bool,
}

impl AddressBookRights {
    /// Every right (the owner's).
    pub const ALL: Self = Self {
        may_read: true,
        may_write: true,
        may_share: true,
        may_delete: true,
    };
}

string_enum! {
    /// `AddressBook` properties (for `properties`).
    pub enum AddressBookProperty ("AddressBook property") {
        /// `id`.
        Id = "id",
        /// `name`.
        Name = "name",
        /// `description`.
        Description = "description",
        /// `sortOrder`.
        SortOrder = "sortOrder",
        /// `isDefault`.
        IsDefault = "isDefault",
        /// `isSubscribed`.
        IsSubscribed = "isSubscribed",
        /// `shareWith`.
        ShareWith = "shareWith",
        /// `myRights`.
        MyRights = "myRights",
    }
}

/// An `AddressBook` as returned by `/get` (and in `/set` `created`).
/// `None`: not sent; `Some(None)`: `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressBook {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<AddressBookId>,
    /// `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `description`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    /// `sortOrder`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<UnsignedInt>,
    /// `isDefault`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// `isSubscribed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_subscribed: Option<bool>,
    /// `shareWith` (`null` in other people's books).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_with: Option<Option<BTreeMap<PrincipalId, AddressBookRights>>>,
    /// `myRights`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_rights: Option<AddressBookRights>,
}

/// Properties only the server sets.
const SERVER_SET: &[&str] = &["id", "isDefault", "myRights"];

/// An address book to create (`AddressBook/set` create).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressBookCreate {
    /// The name (not blank, no control characters).
    pub name: String,
    /// A description.
    pub description: Option<String>,
    /// Sort order (default 0).
    pub sort_order: UnsignedInt,
    /// Whether subscribed; `None` lets the server choose.
    pub is_subscribed: Option<bool>,
    /// Who else may use it.
    pub share_with: Option<BTreeMap<PrincipalId, AddressBookRights>>,
}

fn parse_member(errors: &mut PropertyErrors, out: &mut AddressBook, key: &str, value: Value) {
    match key {
        "name" => {
            let name = errors.parse(key, value);
            out.name = check_name(errors, key, name);
        }
        "description" => {
            let description = errors.parse(key, value);
            out.description = check_description(errors, key, description);
        }
        "sortOrder" => {
            out.sort_order = errors
                .parse::<Option<UnsignedInt>>(key, value)
                .map(Option::unwrap_or_default);
        }
        "isSubscribed" => out.is_subscribed = errors.parse(key, value),
        "shareWith" => out.share_with = errors.parse(key, value),
        _ => errors.invalid(key),
    }
}

impl AddressBookCreate {
    /// Parses one object of `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every invalid, unknown or server-set
    /// property.
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut out = AddressBook::default();
        for (key, value) in object {
            parse_member(&mut errors, &mut out, &key, value);
        }
        let name = errors.required("name", out.name);
        errors.finish()?;
        Ok(Self {
            name: name.unwrap_or_default(),
            description: out.description.flatten(),
            sort_order: out.sort_order.unwrap_or_default(),
            is_subscribed: out.is_subscribed,
            share_with: out.share_with.flatten(),
        })
    }
}

/// An `AddressBook/set` update. `None` fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressBookPatch {
    /// New name.
    pub name: Option<String>,
    /// New description (`Some(None)`: remove it).
    pub description: Option<Option<String>>,
    /// New sort order (`null` resets to 0).
    pub sort_order: Option<UnsignedInt>,
    /// Subscribe or unsubscribe.
    pub is_subscribed: Option<bool>,
    /// Sharing changes (whole map or `shareWith/<principal>`).
    pub share_with: Option<MapPatch<PrincipalId, AddressBookRights>>,
}

impl AddressBookPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidPatch` for bad paths; `invalidProperties` for unknown,
    /// server-set or badly typed properties.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut whole = AddressBook::default();
        let mut members = MapMembers::<PrincipalId, AddressBookRights>::default();
        let mut share_whole = None;
        for Patch { key, path, value } in patch.patches()? {
            match Target::of(&path) {
                Target::Property("shareWith") => {
                    share_whole = errors.parse(&key, value).map(MapPatch::Replace);
                }
                Target::Property(property) if SERVER_SET.contains(&property) => {
                    errors.invalid_because(&key, "server-set");
                }
                Target::Property(property) => {
                    parse_member(&mut errors, &mut whole, property, value);
                }
                Target::Member("shareWith", member) => {
                    members.record(&mut errors, &key, member, value)?;
                }
                Target::Member(property, _) | Target::Deeper(property) => {
                    if AddressBookProperty::ALL
                        .iter()
                        .any(|known| known.as_str() == property)
                    {
                        return Err(SetError::invalid_patch(format!(
                            "{key:?}: cannot patch inside {property}"
                        )));
                    }
                    errors.invalid(key);
                }
            }
        }
        errors.finish()?;
        Ok(Self {
            name: whole.name,
            description: whole.description,
            sort_order: whole.sort_order,
            is_subscribed: whole.is_subscribed,
            share_with: share_whole.or_else(|| members.into_patch()),
        })
    }
}

/// `AddressBook/set` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressBookSetArguments {
    /// The standard arguments (creates: [`AddressBookCreate::parse`];
    /// updates: [`AddressBookPatch::parse`]).
    pub set: SetArguments<AddressBookId>,
    /// Destroy books that have cards, and the cards only in them, instead
    /// of failing with `addressBookHasContents`.
    pub on_destroy_remove_contents: bool,
    /// After everything succeeds, make this the default book.
    pub on_success_set_is_default: Option<IdOrRef<AddressBookId>>,
}

impl AddressBookSetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` or `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        let on_destroy_remove_contents =
            take_argument(&mut arguments, "onDestroyRemoveContents")?.unwrap_or(false);
        let on_success_set_is_default = take_argument(&mut arguments, "onSuccessSetIsDefault")?;
        Ok(Self {
            set: SetArguments::parse(arguments, max_objects)?,
            on_destroy_remove_contents,
            on_success_set_is_default,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::{MethodErrorType, SetErrorType};
    use crate::method::GetArguments;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    /// Stalwart 1.0's `AddressBook/get` (2026-10-27).
    #[test]
    fn stalwart_get_shape() {
        let book = AddressBook {
            id: Some("b".parse().unwrap()),
            name: Some("Stalwart Address Book (alice@nebulamail.test)".to_owned()),
            description: Some(None),
            sort_order: Some(UnsignedInt::ZERO),
            is_default: Some(true),
            is_subscribed: Some(true),
            share_with: Some(Some(BTreeMap::new())),
            my_rights: Some(AddressBookRights::ALL),
        };
        assert_eq!(
            serde_json::to_value(&book).unwrap(),
            json!({
                "id": "b",
                "name": "Stalwart Address Book (alice@nebulamail.test)",
                "description": null,
                "sortOrder": 0,
                "isDefault": true,
                "isSubscribed": true,
                "shareWith": {},
                "myRights": {"mayRead": true, "mayWrite": true, "mayDelete": true, "mayShare": true}
            })
        );
    }

    /// apps/web/src/contacts/api.ts and sharing.ts.
    #[test]
    fn web_requests() {
        let list = GetArguments::<AddressBookId, AddressBookProperty>::parse(
            map(json!({"accountId": "c", "properties": ["name", "isDefault", "myRights", "sortOrder"]})),
            500,
        )
        .unwrap();
        assert_eq!(list.properties.unwrap().len(), 4);
        let share = GetArguments::<AddressBookId, AddressBookProperty>::parse(
            map(json!({"accountId": "c", "ids": ["b"], "properties": ["shareWith"]})),
            500,
        )
        .unwrap();
        assert_eq!(share.properties.unwrap(), [AddressBookProperty::ShareWith]);

        let set = AddressBookSetArguments::parse(
            map(json!({"accountId": "c", "update": {"b": {
                "shareWith/d": {"mayRead": true, "mayWrite": false, "mayShare": false, "mayDelete": false}
            }}})),
            10,
        )
        .unwrap();
        let patch = AddressBookPatch::parse(set.set.update.values().next().unwrap()).unwrap();
        let Some(MapPatch::Change { set, remove }) = patch.share_with else {
            panic!("expected a change")
        };
        assert_eq!(set.len(), 1);
        assert!(remove.is_empty());
        let unshare =
            AddressBookPatch::parse(&serde_json::from_value(json!({"shareWith/d": null})).unwrap())
                .unwrap();
        assert!(matches!(unshare.share_with, Some(MapPatch::Change { .. })));
    }

    #[test]
    fn create_and_errors() {
        let created = AddressBookCreate::parse(map(json!({
            "name": "Trabajo", "description": "Clientes", "sortOrder": 2, "isSubscribed": true
        })))
        .unwrap();
        assert_eq!(created.name, "Trabajo");
        assert_eq!(created.sort_order, UnsignedInt::from(2_u32));
        let error = AddressBookCreate::parse(map(json!({
            "id": "x", "isDefault": true, "myRights": {}, "name": "a\u{7}",
            "shareWith": {"d": {"mayRead": true}}, "sortOrder": -1, "other": 1
        })))
        .unwrap_err();
        let mut properties = error.properties().to_vec();
        properties.sort();
        assert_eq!(
            properties,
            [
                "id",
                "isDefault",
                "myRights",
                "name",
                "other",
                "shareWith",
                "sortOrder"
            ]
        );
        let kind = |value: Value| {
            AddressBookPatch::parse(&serde_json::from_value(value).unwrap())
                .unwrap_err()
                .kind()
        };
        assert_eq!(
            kind(json!({"isDefault": false})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(kind(json!({"name": ""})), SetErrorType::InvalidProperties);
        assert_eq!(kind(json!({"name/x": 1})), SetErrorType::InvalidPatch);
        assert_eq!(
            kind(json!({"shareWith/d/mayRead": true})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(kind(json!({"x/y": 1})), SetErrorType::InvalidProperties);

        let destroy = AddressBookSetArguments::parse(
            map(
                json!({"accountId": "c", "destroy": ["b"], "onDestroyRemoveContents": true,
                       "onSuccessSetIsDefault": "c2"}),
            ),
            10,
        )
        .unwrap();
        assert!(destroy.on_destroy_remove_contents);
        assert_eq!(
            destroy.on_success_set_is_default,
            Some(IdOrRef::Id("c2".parse().unwrap()))
        );
        assert_eq!(
            AddressBookSetArguments::parse(
                map(json!({"accountId": "c", "onDestroyRemoveEvents": true})),
                10
            )
            .unwrap_err()
            .kind(),
            MethodErrorType::InvalidArguments
        );
    }
}
