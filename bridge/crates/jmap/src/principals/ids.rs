// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Principal ids: `r{n}`, where `n` is the stable number of the
//! principal's DAV location (`nebula_groupware::principals::
//! principal_number`). `r` is not used by any other object (`m e t i s`
//! mail, `a k` contacts, `c v p` calendars, `q` quotas). Parsed strictly,
//! like every other id.

use nebula_jmap_proto::PrincipalId;

use crate::core::ids::parse_number;
use crate::error::Bug;

/// The JMAP id of a principal number.
pub(crate) fn principal_id(number: i64) -> Result<PrincipalId, Bug> {
    format!("r{number}")
        .parse()
        .map_err(|_| Bug("principal id"))
}

/// The number of a principal id, if it is one of ours.
pub(crate) fn principal_number(id: &PrincipalId) -> Option<i64> {
    parse_number(id.as_str(), 'r')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_strict() {
        let id = principal_id(4_611_686_018_427_387_904).unwrap();
        assert_eq!(id.as_str(), "r4611686018427387904");
        assert_eq!(principal_number(&id), Some(4_611_686_018_427_387_904));
        for bad in ["r0", "r01", "r-1", "p1", "r", "r1a", "R1"] {
            assert_eq!(principal_number(&bad.parse().unwrap()), None, "{bad}");
        }
    }
}
