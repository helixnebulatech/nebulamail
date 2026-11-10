// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What a DAV failure means for one object (`SetError`) or for a whole
//! call. Descriptions never carry server texts, hrefs or card content.

use nebula_dav::DavError;
use nebula_dav::proto::{Precondition, Property};
use nebula_groupware::GroupwareError;
use nebula_jmap_proto::{MethodError, MethodErrorType, SetError, SetErrorType};

use crate::error::{CallError, store_set_error};

fn forbidden(description: &str) -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description(description)
}

/// The JMAP name of a collection property we set.
const fn property_name(property: Property) -> &'static str {
    match property {
        Property::DisplayName => "name",
        Property::AddressbookDescription | Property::CalendarDescription => "description",
        Property::CalendarOrder => "sortOrder",
        _ => "properties",
    }
}

fn precondition_error(precondition: Option<Precondition>) -> SetError {
    match precondition {
        Some(Precondition::NoUidConflict) => SetError::invalid_properties(["uid"])
            .with_description("another card in this address book has this uid"),
        Some(Precondition::SupportedData | Precondition::ValidData) => {
            SetError::new(SetErrorType::InvalidProperties)
                .with_description("the server refused this vCard")
        }
        Some(Precondition::MaxResourceSize) => SetError::new(SetErrorType::TooLarge)
            .with_description("the card is larger than the server allows"),
        Some(Precondition::NeedPrivileges) => forbidden("the server does not allow this"),
        _ => forbidden("the server refused the change"),
    }
}

/// The `SetError` for a failed write of one card or book.
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
            .with_description("the card is larger than the bridge sends"),
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
        GroupwareError::Unauthorized => forbidden("the contacts server refused the login"),
        GroupwareError::NoEtag => forbidden(
            "the server gives no version tag for this card, so it can't be changed safely",
        ),
        GroupwareError::NotDiscovered => forbidden("the contacts server is not ready yet"),
        GroupwareError::Store(error) => store_set_error(error),
        GroupwareError::Dav(dav) if dav.is_temporary() => {
            forbidden("the contacts server is unavailable; try again later")
        }
        other => {
            tracing::warn!(error = %other, "contacts write failed");
            forbidden("the contacts server failed")
        }
    }
}

/// A failure that stops a whole read (`/get`): the DAV account can't be
/// read now.
pub(crate) fn call_error(error: GroupwareError) -> CallError {
    match error {
        GroupwareError::Store(error) => CallError::Store(error),
        other => {
            tracing::warn!(error = %other, "reading contacts failed");
            MethodError::new(MethodErrorType::ServerUnavailable)
                .with_description("the contacts can't be read now")
                .into()
        }
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
        assert!(
            set_error(&GroupwareError::Dav(DavError::PreconditionFailed))
                .description()
                .unwrap()
                .contains("changed on the server")
        );
        assert_eq!(kind(GroupwareError::NotFound), SetErrorType::NotFound);
        assert_eq!(
            kind(GroupwareError::Dav(DavError::NotFound)),
            SetErrorType::NotFound
        );
        let uid = set_error(&GroupwareError::Dav(DavError::Forbidden {
            precondition: Some(Precondition::NoUidConflict),
        }));
        assert_eq!(uid.kind(), SetErrorType::InvalidProperties);
        assert_eq!(uid.properties(), ["uid"]);
        assert_eq!(
            kind(GroupwareError::Dav(DavError::Conflict {
                precondition: Some(Precondition::MaxResourceSize)
            })),
            SetErrorType::TooLarge
        );
        assert_eq!(
            kind(GroupwareError::Dav(DavError::InsufficientStorage)),
            SetErrorType::OverQuota
        );
        assert_eq!(
            kind(GroupwareError::Dav(DavError::TooLarge)),
            SetErrorType::TooLarge
        );
        let refused = set_error(&GroupwareError::PropertiesRefused(vec![
            Property::CalendarOrder,
            Property::DisplayName,
        ]));
        assert_eq!(refused.kind(), SetErrorType::Forbidden);
        assert_eq!(
            refused.description(),
            Some("the server does not keep name, sortOrder")
        );
        assert_eq!(kind(GroupwareError::Unauthorized), SetErrorType::Forbidden);
        assert_eq!(
            kind(GroupwareError::Dav(DavError::Timeout)),
            SetErrorType::Forbidden
        );
    }

    #[test]
    fn read_failures_are_unavailable() {
        let error = call_error(GroupwareError::Unreadable).into_method_error();
        assert_eq!(error.kind(), MethodErrorType::ServerUnavailable);
    }
}
