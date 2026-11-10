// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! A Calendar Server or Nextcloud sharing change in `POST`s of at most
//! `max` entries ([`MAX_SHARE_REQUEST`] for Calendar Server,
//! [`MAX_OC_SHARE_REQUEST`] for Nextcloud).
//!
//! Each name goes **once** ([`merge_names`]): a removal wins over a `set`
//! of the same name, and two accesses for one name become the lesser
//! (read), never the greater. That is what makes a failure half way safe:
//! every sharee is in one `POST` only, so whatever was sent, each one has
//! what it had before or what was asked, never more than the greater of
//! the two.
//!
//! The order ([`share_batches`]) adds a margin: every removal first, then
//! every `read`, then every `read-write`. Nothing is given until all the
//! removals were sent, and a change from read-write to read goes before any
//! read-write. With at most `max` entries there is one
//! `POST`, with the `set`s and `remove`s in the order they came (the same
//! body as before there were batches).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use nebula_dav::proto::ShareAccess;
#[cfg(doc)]
use nebula_dav::proto::{MAX_OC_SHARE_REQUEST, MAX_SHARE_REQUEST};

/// The lesser of two accesses.
const fn lesser(a: ShareAccess, b: ShareAccess) -> ShareAccess {
    match (a, b) {
        (ShareAccess::ReadWrite, ShareAccess::ReadWrite) => ShareAccess::ReadWrite,
        _ => ShareAccess::Read,
    }
}

/// Each name once, in the order first seen: a removal wins over a `set`,
/// and two accesses become the lesser.
pub(crate) fn merge_names<N, K: Eq + Hash>(
    set: Vec<(N, ShareAccess)>,
    remove: Vec<N>,
    key: impl Fn(&N) -> K,
) -> (Vec<(N, ShareAccess)>, Vec<N>) {
    let mut removed: HashSet<K> = HashSet::with_capacity(remove.len());
    let mut remove_out = Vec::with_capacity(remove.len());
    for name in remove {
        if removed.insert(key(&name)) {
            remove_out.push(name);
        }
    }
    let mut at: HashMap<K, usize> = HashMap::with_capacity(set.len());
    let mut set_out: Vec<(N, ShareAccess)> = Vec::with_capacity(set.len());
    for (name, access) in set {
        let k = key(&name);
        if removed.contains(&k) {
            continue;
        }
        if let Some((_, known)) = at.get(&k).and_then(|i| set_out.get_mut(*i)) {
            *known = lesser(*known, access);
        } else {
            at.insert(k, set_out.len());
            set_out.push((name, access));
        }
    }
    (set_out, remove_out)
}

/// One `POST`'s `set` and `remove`.
pub(crate) type Batch<N> = (Vec<(N, ShareAccess)>, Vec<N>);

/// The `POST`s of a merged change ([`merge_names`]): one if it fits,
/// else removals, then reads, then read-writes, each at most `max`
/// entries (at least 1). Nothing to do is no `POST`.
pub(crate) fn share_batches<N>(
    set: Vec<(N, ShareAccess)>,
    remove: Vec<N>,
    max: usize,
) -> Vec<Batch<N>> {
    let max = max.max(1);
    if set.is_empty() && remove.is_empty() {
        return Vec::new();
    }
    if set.len() + remove.len() <= max {
        return vec![(set, remove)];
    }
    let mut out: Vec<Batch<N>> = Vec::new();
    let mut remove = remove.into_iter().peekable();
    while remove.peek().is_some() {
        out.push((Vec::new(), remove.by_ref().take(max).collect()));
    }
    let (read, write): (Vec<_>, Vec<_>) = set
        .into_iter()
        .partition(|(_, access)| *access == ShareAccess::Read);
    for group in [read, write] {
        let mut group = group.into_iter().peekable();
        while group.peek().is_some() {
            out.push((group.by_ref().take(max).collect(), Vec::new()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nebula_dav::proto::{MAX_OC_SHARE_REQUEST, MAX_SHARE_REQUEST};

    fn names(prefix: &str, count: usize) -> Vec<String> {
        (0..count).map(|n| format!("{prefix}{n}")).collect()
    }

    #[test]
    fn share_batches_order_and_size() {
        let remove = names("gone", 300);
        let mut set: Vec<(String, ShareAccess)> = names("read", 300)
            .into_iter()
            .map(|n| (n, ShareAccess::Read))
            .collect();
        set.extend(
            names("write", 300)
                .into_iter()
                .map(|n| (n, ShareAccess::ReadWrite)),
        );
        let batches = share_batches(set.clone(), remove.clone(), MAX_SHARE_REQUEST);
        assert!(
            batches
                .iter()
                .all(|(s, r)| s.len() + r.len() <= MAX_SHARE_REQUEST)
        );
        // Removals, then reads, then read-writes: no group starts before
        // the one before it ended.
        let stage = |(s, r): &Batch<String>| match (s.first(), r.is_empty()) {
            (None, false) => 0,
            (Some((_, ShareAccess::Read)), true) => 1,
            (Some((_, ShareAccess::ReadWrite)), true) => 2,
            _ => panic!("mixed batch"),
        };
        let stages: Vec<u8> = batches.iter().map(stage).collect();
        assert!(stages.windows(2).all(|w| w[0] <= w[1]), "{stages:?}");
        assert!(
            batches
                .iter()
                .all(|(s, _)| { s.iter().all(|(_, a)| *a == s[0].1) })
        );
        let entries: usize = batches.iter().map(|(s, r)| s.len() + r.len()).sum();
        assert_eq!(entries, 900);
        assert_eq!(batches.len(), 6);
        // Up to the limit: one, as it came.
        let small: Vec<(String, ShareAccess)> = names("a", 200)
            .into_iter()
            .map(|n| (n, ShareAccess::ReadWrite))
            .collect();
        let one = share_batches(small.clone(), names("b", 56), MAX_SHARE_REQUEST);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].0, small);
        assert!(share_batches::<String>(Vec::new(), Vec::new(), MAX_SHARE_REQUEST).is_empty());
        // Nextcloud's 10: the same 900 entries in 90 POSTs, same order.
        let small = share_batches(set, remove, MAX_OC_SHARE_REQUEST);
        assert_eq!(small.len(), 90);
        assert!(
            small
                .iter()
                .all(|(s, r)| s.len() + r.len() <= MAX_OC_SHARE_REQUEST)
        );
        let stages: Vec<u8> = small.iter().map(stage).collect();
        assert!(stages.windows(2).all(|w| w[0] <= w[1]), "{stages:?}");
    }

    #[test]
    fn each_name_once_removal_wins_lesser_access() {
        let set = vec![
            ("bob".to_owned(), ShareAccess::ReadWrite),
            ("carol".to_owned(), ShareAccess::ReadWrite),
            ("BOB".to_owned(), ShareAccess::Read),
            ("dave".to_owned(), ShareAccess::Read),
            ("dave".to_owned(), ShareAccess::ReadWrite),
            ("carol".to_owned(), ShareAccess::ReadWrite),
        ];
        let remove = vec!["dave".to_owned(), "eve".to_owned(), "DAVE".to_owned()];
        let (set, remove) = merge_names(set, remove, |n: &String| n.to_ascii_lowercase());
        assert_eq!(
            set,
            [
                ("bob".to_owned(), ShareAccess::Read),
                ("carol".to_owned(), ShareAccess::ReadWrite)
            ]
        );
        assert_eq!(remove, ["dave".to_owned(), "eve".to_owned()]);
    }
}
