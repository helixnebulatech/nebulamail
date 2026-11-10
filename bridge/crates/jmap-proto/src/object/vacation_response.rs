// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `VacationResponse` (RFC 8621 §8): one object per account, id
//! `singleton`.

use serde::Serialize;
use serde_json::Value;

use crate::error::SetError;
use crate::macros::string_enum;
use crate::method::parse::PropertyErrors;
use crate::method::patch::Target;
use crate::method::{Patch, PatchObject};
use crate::types::{UtcDate, VacationResponseId};

/// The only id a `VacationResponse` has.
pub const VACATION_SINGLETON: &str = "singleton";

string_enum! {
    /// `VacationResponse` properties.
    pub enum VacationResponseProperty ("VacationResponse property") {
        /// `id`.
        Id = "id",
        /// `isEnabled`.
        IsEnabled = "isEnabled",
        /// `fromDate`.
        FromDate = "fromDate",
        /// `toDate`.
        ToDate = "toDate",
        /// `subject`.
        Subject = "subject",
        /// `textBody`.
        TextBody = "textBody",
        /// `htmlBody`.
        HtmlBody = "htmlBody",
    }
}

/// A `VacationResponse` as returned by `/get` (only the asked
/// properties; the nullable ones as `Some(None)` when null).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VacationResponse {
    /// `id` (`singleton`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<VacationResponseId>,
    /// `isEnabled`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_enabled: Option<bool>,
    /// `fromDate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_date: Option<Option<UtcDate>>,
    /// `toDate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_date: Option<Option<UtcDate>>,
    /// `subject`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<Option<String>>,
    /// `textBody`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_body: Option<Option<String>>,
    /// `htmlBody`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_body: Option<Option<String>>,
}

/// An update; `None` leaves a property as it is, `Some(None)` clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VacationResponsePatch {
    /// New `isEnabled`.
    pub is_enabled: Option<bool>,
    /// New `fromDate`.
    pub from_date: Option<Option<UtcDate>>,
    /// New `toDate`.
    pub to_date: Option<Option<UtcDate>>,
    /// New `subject`.
    pub subject: Option<Option<String>>,
    /// New `textBody`.
    pub text_body: Option<Option<String>>,
    /// New `htmlBody`.
    pub html_body: Option<Option<String>>,
}

impl VacationResponsePatch {
    /// Parses the patch of `singleton`.
    ///
    /// # Errors
    /// `invalidPatch` for paths inside properties; `invalidProperties` for
    /// `id`, unknown or badly typed properties.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut out = Self::default();
        for Patch { key, path, value } in patch.patches()? {
            let text = |errors: &mut PropertyErrors, value: Value| {
                errors.parse::<Option<String>>(&key, value)
            };
            match Target::of(&path) {
                Target::Property("isEnabled") => out.is_enabled = errors.parse(&key, value),
                Target::Property("fromDate") => out.from_date = errors.parse(&key, value),
                Target::Property("toDate") => out.to_date = errors.parse(&key, value),
                Target::Property("subject") => out.subject = text(&mut errors, value),
                Target::Property("textBody") => out.text_body = text(&mut errors, value),
                Target::Property("htmlBody") => out.html_body = text(&mut errors, value),
                Target::Property(_) => errors.invalid(key),
                Target::Member(..) | Target::Deeper(..) => {
                    return Err(SetError::invalid_patch(format!(
                        "{key:?}: VacationResponse properties have no members to patch"
                    )));
                }
            }
        }
        errors.finish()?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn patch_and_shape() {
        let patch: PatchObject = serde_json::from_value(json!({
            "isEnabled": true, "fromDate": "2026-10-30T00:00:00Z", "toDate": null,
            "subject": "Away", "textBody": null
        }))
        .unwrap();
        let parsed = VacationResponsePatch::parse(&patch).unwrap();
        assert_eq!(parsed.is_enabled, Some(true));
        assert!(matches!(parsed.from_date, Some(Some(_))));
        assert_eq!(parsed.to_date, Some(None));
        assert_eq!(parsed.html_body, None);
        for bad in [
            json!({"id": "x"}),
            json!({"isEnabled": "yes"}),
            json!({"fromDate": "today"}),
        ] {
            let patch: PatchObject = serde_json::from_value(bad).unwrap();
            assert!(VacationResponsePatch::parse(&patch).is_err());
        }
        let object = VacationResponse {
            id: Some(VACATION_SINGLETON.parse().unwrap()),
            is_enabled: Some(false),
            subject: Some(None),
            ..VacationResponse::default()
        };
        assert_eq!(
            serde_json::to_value(object).unwrap(),
            json!({"id": "singleton", "isEnabled": false, "subject": null})
        );
    }
}
