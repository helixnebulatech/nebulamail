// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What a failed calendar write means for one object (`SetError`).
//! Descriptions never carry server texts, hrefs or event content.

use nebula_dav::DavError;
use nebula_dav::proto::{Precondition, Property};
use nebula_groupware::{ConvertError, GroupwareError};
use nebula_jmap_proto::{SetError, SetErrorType};

use crate::error::store_set_error;

pub(crate) fn forbidden(description: &str) -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description(description)
}

/// The JMAP name of a calendar property we set.
const fn property_name(property: Property) -> &'static str {
    match property {
        Property::DisplayName => "name",
        Property::CalendarDescription => "description",
        Property::CalendarOrder => "sortOrder",
        Property::CalendarColor => "color",
        _ => "properties",
    }
}

fn precondition_error(precondition: Option<Precondition>) -> SetError {
    match precondition {
        Some(Precondition::NoUidConflict) => SetError::invalid_properties(["uid"])
            .with_description("another event in this calendar has this uid"),
        Some(
            Precondition::SupportedData
            | Precondition::ValidData
            | Precondition::ValidCalendarObjectResource
            | Precondition::SupportedCalendarComponent,
        ) => SetError::new(SetErrorType::InvalidProperties)
            .with_description("the server refused this event"),
        Some(Precondition::MaxResourceSize) => SetError::new(SetErrorType::TooLarge)
            .with_description("the event is larger than the server allows"),
        Some(Precondition::NeedPrivileges) => forbidden("the server does not allow this"),
        _ => forbidden("the server refused the change"),
    }
}

/// The `SetError` for a failed write of one event or calendar.
pub(crate) fn set_error(error: &GroupwareError) -> SetError {
    match error {
        error if error.is_conflict() => {
            forbidden("it changed on the server meanwhile; reload it and try again")
        }
        GroupwareError::NotFound | GroupwareError::Dav(DavError::NotFound) => SetError::not_found(),
        GroupwareError::Dav(
            DavError::Forbidden { precondition } | DavError::Conflict { precondition },
        ) => precondition_error(*precondition),
        GroupwareError::Dav(DavError::TooLarge) => SetError::new(SetErrorType::TooLarge)
            .with_description("the event is larger than the bridge sends"),
        GroupwareError::Dav(DavError::InsufficientStorage) => {
            SetError::new(SetErrorType::OverQuota)
        }
        GroupwareError::Dav(DavError::MethodNotAllowed) => {
            forbidden("something already exists there")
        }
        GroupwareError::PropertiesRefused(properties) => {
            let mut names: Vec<&str> = properties.iter().map(|p| property_name(*p)).collect();
            names.sort_unstable();
            names.dedup();
            forbidden(&format!("the server does not keep {}", names.join(", ")))
        }
        GroupwareError::Unauthorized => forbidden("the calendar server refused the login"),
        GroupwareError::NoEtag => forbidden(
            "the server gives no version tag for this event, so it can't be changed safely",
        ),
        GroupwareError::NotDiscovered => forbidden("the calendar server is not ready yet"),
        GroupwareError::Unsupported => forbidden("the calendar server does not offer this"),
        GroupwareError::Store(error) => store_set_error(error),
        GroupwareError::Dav(dav) if dav.is_temporary() => {
            forbidden("the calendar server is unavailable; try again later")
        }
        other => {
            tracing::warn!(error = %other, "calendar write failed");
            forbidden("the calendar server failed")
        }
    }
}

/// A conversion failure (JSCalendar ↔ iCalendar) of one event.
pub(crate) fn convert_error(error: &ConvertError) -> SetError {
    match error {
        ConvertError::TooLarge { .. } => {
            SetError::new(SetErrorType::TooLarge).with_description("the event is too large")
        }
        _ => SetError::new(SetErrorType::InvalidProperties)
            .with_description("the event can't be written as iCalendar"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dav_failures_map_to_set_errors() {
        let kind = |error: GroupwareError| set_error(&error).kind();
        assert_eq!(
            kind(GroupwareError::Dav(DavError::PreconditionFailed)),
            SetErrorType::Forbidden
        );
        assert_eq!(kind(GroupwareError::NotFound), SetErrorType::NotFound);
        let uid = set_error(&GroupwareError::Dav(DavError::Forbidden {
            precondition: Some(Precondition::NoUidConflict),
        }));
        assert_eq!(uid.properties(), ["uid"]);
        assert_eq!(
            kind(GroupwareError::Dav(DavError::InsufficientStorage)),
            SetErrorType::OverQuota
        );
        let refused = set_error(&GroupwareError::PropertiesRefused(vec![
            Property::CalendarColor,
            Property::DisplayName,
        ]));
        assert_eq!(
            refused.description(),
            Some("the server does not keep color, name")
        );
        assert_eq!(
            convert_error(&ConvertError::Export {
                format: nebula_groupware::model::convert::Format::JsCalendar
            })
            .kind(),
            SetErrorType::InvalidProperties
        );
    }
}
