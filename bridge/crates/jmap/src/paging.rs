// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The window of `/query` results (RFC 8620 §5.5): `position` (negative
//! counts from the end) or `anchor` + `anchorOffset`, then `limit`.

use nebula_jmap_proto::MethodErrorType;

use crate::error::CallError;

/// One page of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Page<T> {
    /// Index of the first id in the full results.
    pub(crate) position: u64,
    pub(crate) ids: Vec<T>,
}

/// Cuts `all` to the requested window.
///
/// # Errors
/// `anchorNotFound` if the anchor is not in `all`.
pub(crate) fn page<T: Copy + PartialEq>(
    all: &[T],
    position: i64,
    anchor: Option<(T, i64)>,
    limit: Option<u64>,
) -> Result<Page<T>, CallError> {
    let total = i64::try_from(all.len()).unwrap_or(i64::MAX);
    let start = match anchor {
        Some((anchor, offset)) => {
            let index = all
                .iter()
                .position(|id| *id == anchor)
                .ok_or(CallError::from(MethodErrorType::AnchorNotFound))?;
            i64::try_from(index)
                .unwrap_or(i64::MAX)
                .saturating_add(offset)
                .max(0)
        }
        None if position < 0 => total.saturating_add(position).max(0),
        None => position,
    };
    let start = usize::try_from(start.min(total)).unwrap_or(all.len());
    let limit = limit.map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let ids = all.iter().skip(start).take(limit).copied().collect();
    Ok(Page {
        position: u64::try_from(start).unwrap_or(0),
        ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[u32] = &[1, 2, 3, 4, 5];

    #[test]
    fn positions() {
        assert_eq!(page(ALL, 0, None, Some(2)).unwrap().ids, vec![1, 2]);
        let end = page(ALL, -2, None, None).unwrap();
        assert_eq!((end.position, end.ids), (3, vec![4, 5]));
        assert_eq!(page(ALL, -10, None, Some(1)).unwrap().ids, vec![1]);
        assert!(page(ALL, 10, None, None).unwrap().ids.is_empty());
    }

    #[test]
    fn anchors() {
        assert_eq!(page(ALL, 0, Some((3, 0)), Some(2)).unwrap().ids, vec![3, 4]);
        assert_eq!(page(ALL, 0, Some((3, -5)), Some(1)).unwrap().ids, vec![1]);
        assert!(page(ALL, 0, Some((9, 0)), None).is_err());
    }
}
