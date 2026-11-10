// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sieve scripts (RFC 9661): `SieveScript/get`, `/changes`, `/query`,
//! `/set` and `/validate`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{MethodError, SetError};
use crate::macros::string_enum;
use crate::method::parse::{PropertyErrors, from_arguments, take_argument};
use crate::method::patch::Target;
use crate::method::{
    FilterCondition, Patch, PatchObject, QueryArguments, SetArguments, SortProperty,
};
use crate::references::IdOrRef;
use crate::types::{AccountId, BlobId, SieveScriptId};

string_enum! {
    /// `SieveScript` properties.
    pub enum SieveScriptProperty ("SieveScript property") {
        /// `id`.
        Id = "id",
        /// `name`.
        Name = "name",
        /// `blobId`: the script's content.
        BlobId = "blobId",
        /// `isActive`.
        IsActive = "isActive",
    }
}

/// A `SieveScript` as returned by `/get` (only the asked properties).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SieveScript {
    /// `id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<SieveScriptId>,
    /// `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `blobId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<BlobId>,
    /// `isActive`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_active: Option<bool>,
}

/// `SieveScript/set` arguments (RFC 9661 §2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SieveScriptSetArguments {
    /// The standard arguments.
    pub set: SetArguments<SieveScriptId>,
    /// After success, make this script (or `#creationId`) the active one.
    /// A string that is not a valid id is `None`: RFC 9661 §2.4 says an
    /// invalid or nonexistent id "MUST be ignored".
    pub on_success_activate_script: Option<IdOrRef<SieveScriptId>>,
    /// After success, deactivate the active script; with both arguments
    /// this runs first (RFC 9661 §2.4).
    pub on_success_deactivate_script: bool,
}

impl SieveScriptSetArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments` (also for an `onSuccessActivateScript` that is
    /// not a string) or `requestTooLarge`.
    pub fn parse(
        mut arguments: Map<String, Value>,
        max_objects: usize,
    ) -> Result<Self, MethodError> {
        // An `Id` is a string (RFC 8620 §1.2): another type is a malformed
        // request; a string with bad syntax is an invalid id, ignored.
        let on_success_activate_script =
            take_argument::<String>(&mut arguments, "onSuccessActivateScript")?
                .and_then(|id| id.parse().ok());
        let on_success_deactivate_script =
            take_argument(&mut arguments, "onSuccessDeactivateScript")?.unwrap_or(false);
        Ok(Self {
            set: SetArguments::parse(arguments, max_objects)?,
            on_success_activate_script,
            on_success_deactivate_script,
        })
    }
}

/// A script to create: its content and, optionally, its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SieveScriptCreate {
    /// `name` (`None`: the server picks one).
    pub name: Option<String>,
    /// `blobId` of the content (an upload or another script's blob).
    pub blob_id: BlobId,
}

impl SieveScriptCreate {
    /// Parses one object of `create`.
    ///
    /// # Errors
    /// `invalidProperties` listing every problem (`isActive` and `id` are
    /// server-set).
    pub fn parse(object: Map<String, Value>) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut name = None;
        let mut blob_id = None;
        for (key, value) in object {
            match key.as_str() {
                "name" => name = errors.parse::<Option<String>>(&key, value).flatten(),
                "blobId" => blob_id = errors.parse::<BlobId>(&key, value),
                _ => errors.invalid(key),
            }
        }
        let blob_id = errors.required("blobId", blob_id);
        errors.finish()?;
        let blob_id =
            blob_id.ok_or_else(|| SetError::invalid_properties(vec!["blobId".to_owned()]))?;
        Ok(Self { name, blob_id })
    }
}

/// An update: a new name and/or new content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SieveScriptPatch {
    /// New name.
    pub name: Option<String>,
    /// New content.
    pub blob_id: Option<BlobId>,
}

impl SieveScriptPatch {
    /// Parses one patch of `update`.
    ///
    /// # Errors
    /// `invalidPatch` for paths inside properties; `invalidProperties` for
    /// other or badly typed properties.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut out = Self::default();
        for Patch { key, path, value } in patch.patches()? {
            match Target::of(&path) {
                Target::Property("name") => match errors.parse::<Option<String>>(&key, value) {
                    Some(Some(name)) => out.name = Some(name),
                    Some(None) => errors.invalid_because("name", "cannot be null"),
                    None => {}
                },
                Target::Property("blobId") => out.blob_id = errors.parse::<BlobId>(&key, value),
                Target::Property(_) => errors.invalid(key),
                Target::Member(..) | Target::Deeper(..) => {
                    return Err(SetError::invalid_patch(format!(
                        "{key:?}: SieveScript properties have no members to patch"
                    )));
                }
            }
        }
        errors.finish()?;
        Ok(out)
    }
}

/// `SieveScript` `FilterCondition` (RFC 9661 §2.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SieveScriptFilterCondition {
    /// The name contains this (case-insensitive).
    #[serde(default)]
    pub name: Option<String>,
    /// `isActive` is this.
    #[serde(default)]
    pub is_active: Option<bool>,
}

impl FilterCondition for SieveScriptFilterCondition {
    const PROPERTIES: &'static [&'static str] = &["name", "isActive"];
}

/// `SieveScript` sort properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SieveScriptSort {
    /// `name`.
    Name,
    /// `isActive`.
    IsActive,
}

impl SortProperty for SieveScriptSort {
    fn parse(property: &str, _extra: &mut Map<String, Value>) -> Result<Self, MethodError> {
        match property {
            "name" => Ok(Self::Name),
            "isActive" => Ok(Self::IsActive),
            other => Err(MethodError::unsupported_sort(format!(
                "cannot sort scripts by {other:?}"
            ))),
        }
    }
}

/// `SieveScript/query` arguments.
pub type SieveScriptQueryArguments =
    QueryArguments<SieveScriptFilterCondition, SieveScriptSort, SieveScriptId>;

/// `SieveScript/validate` arguments (RFC 9661 §2.5).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SieveScriptValidateArguments {
    /// The account.
    pub account_id: AccountId,
    /// The script to check.
    pub blob_id: BlobId,
}

impl SieveScriptValidateArguments {
    /// Parses the arguments.
    ///
    /// # Errors
    /// `invalidArguments`.
    pub fn parse(arguments: Map<String, Value>) -> Result<Self, MethodError> {
        from_arguments(arguments)
    }
}

/// `SieveScript/validate` response: `error` is `null` for a valid script,
/// else an `invalidSieve` `SetError` (RFC 9661 §2.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SieveScriptValidateResponse {
    /// The account.
    pub account_id: AccountId,
    /// Why the script is invalid, or `None`.
    pub error: Option<SetError>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    fn map(value: Value) -> Map<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn set_arguments() {
        let args = SieveScriptSetArguments::parse(
            map(json!({
                "accountId": "a",
                "create": {"script": {"name": "x", "blobId": "bu1"}},
                "onSuccessActivateScript": "#script"
            })),
            10,
        )
        .unwrap();
        assert_eq!(
            args.on_success_activate_script,
            Some(IdOrRef::Ref("script".parse().unwrap()))
        );
        assert!(!args.on_success_deactivate_script);
        let create =
            SieveScriptCreate::parse(args.set.create.into_values().next().unwrap()).unwrap();
        assert_eq!(create.name.as_deref(), Some("x"));
        let deactivate = SieveScriptSetArguments::parse(
            map(json!({"accountId": "a", "onSuccessDeactivateScript": true})),
            10,
        )
        .unwrap();
        assert!(deactivate.on_success_deactivate_script);
    }

    /// RFC 9661 §2.4: an invalid id "MUST be ignored", so it parses as no
    /// activation; a value that isn't a string is still a bad request.
    #[test]
    fn invalid_activate_id_is_ignored() {
        for bad in ["", "#", "a b", "#a/b", "x\u{0}"] {
            let args = SieveScriptSetArguments::parse(
                map(json!({"accountId": "a", "onSuccessActivateScript": bad})),
                10,
            )
            .unwrap();
            assert_eq!(args.on_success_activate_script, None, "{bad:?}");
        }
        assert!(
            SieveScriptSetArguments::parse(
                map(json!({"accountId": "a", "onSuccessActivateScript": 1})),
                10,
            )
            .is_err()
        );
    }

    #[test]
    fn objects() {
        assert_eq!(
            SieveScriptCreate::parse(map(json!({"name": "x"})))
                .unwrap_err()
                .kind(),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            SieveScriptCreate::parse(map(json!({"blobId": "b1", "isActive": true})))
                .unwrap_err()
                .kind(),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            SieveScriptCreate::parse(map(json!({"blobId": "b1", "name": null})))
                .unwrap()
                .name,
            None
        );
        let patch: PatchObject = serde_json::from_value(json!({"name": "y"})).unwrap();
        assert_eq!(
            SieveScriptPatch::parse(&patch).unwrap().name.as_deref(),
            Some("y")
        );
        for bad in [
            json!({"name": null}),
            json!({"isActive": true}),
            json!({"name": 1}),
        ] {
            let patch: PatchObject = serde_json::from_value(bad).unwrap();
            assert!(SieveScriptPatch::parse(&patch).is_err());
        }
    }

    #[test]
    fn shapes() {
        let script = SieveScript {
            id: Some("z1".parse().unwrap()),
            name: Some("a".to_owned()),
            blob_id: Some("bz1_00".parse().unwrap()),
            is_active: Some(true),
        };
        assert_eq!(
            serde_json::to_value(&script).unwrap(),
            json!({"id": "z1", "name": "a", "blobId": "bz1_00", "isActive": true})
        );
        assert_eq!(
            serde_json::to_value(SieveScriptValidateResponse {
                account_id: "a".parse().unwrap(),
                error: None
            })
            .unwrap(),
            json!({"accountId": "a", "error": null})
        );
    }
}
