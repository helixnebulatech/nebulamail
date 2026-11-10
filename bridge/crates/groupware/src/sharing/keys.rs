// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sharees compared by a normalized key, so that a change with thousands
//! of them is checked in linear time (a hash lookup each), not by
//! comparing everyone with everyone and parsing URLs each time.
//!
//! [`sharee_key`] gives the same answers as the comparisons it replaces:
//! two sharees are the same when they are at the same location (origin
//! and decoded segments, without the trailing slash), or the same
//! `mailto:` address in any case, or else the same text. A principal is
//! its location and each of its addresses ([`principal_keys`]).

use std::collections::{HashMap, HashSet};

use nebula_dav::proto::{CalendarAddress, Href, LocationKey, Sharee as DavSharee};
use nebula_store::{Rights, Sharee};

use super::{ShareUpdate, dav_sharee, grants_access};
use crate::principals::Principal;

/// How two sharees are compared.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ShareeKey {
    /// A principal's location.
    Location(LocationKey),
    /// A `mailto:` address, lowercased (ASCII).
    Address(String),
    /// Anything the bridge does not read, by its text.
    Raw(String),
}

/// The key of a stored sharee (one parse).
pub(crate) fn sharee_key(sharee: &Sharee) -> ShareeKey {
    match dav_sharee(sharee) {
        Some(named) => dav_key(&named),
        None => ShareeKey::Raw(sharee.as_str().to_owned()),
    }
}

/// The key of a sharee as the DAV client names it.
pub(crate) fn dav_key(sharee: &DavSharee) -> ShareeKey {
    match sharee {
        DavSharee::Principal(href) => location_key(href),
        DavSharee::Address(address) => address_key(address),
    }
}

pub(crate) fn location_key(href: &Href) -> ShareeKey {
    ShareeKey::Location(href.location_key())
}

pub(crate) fn address_key(address: &CalendarAddress) -> ShareeKey {
    ShareeKey::Address(address.as_str().to_ascii_lowercase())
}

/// Every key a principal is known by: its location, then each address.
pub(crate) fn principal_keys(principal: &Principal) -> impl Iterator<Item = ShareeKey> + '_ {
    std::iter::once(location_key(&principal.href))
        .chain(principal.addresses.iter().map(address_key))
}

/// The keys and rights of a list of sharees, with the rights of the
/// entries of one key joined (the most they have).
pub(crate) fn rights_by_key(shares: &[(Sharee, Rights)]) -> HashMap<ShareeKey, Rights> {
    let mut out: HashMap<ShareeKey, Rights> = HashMap::with_capacity(shares.len());
    for (sharee, rights) in shares {
        out.entry(sharee_key(sharee))
            .and_modify(|known| *known = known.union(*rights))
            .or_insert(*rights);
    }
    out
}

/// The rights listed for a principal (all its keys joined).
pub(crate) fn principal_rights(
    index: &HashMap<ShareeKey, Rights>,
    principal: &Principal,
) -> Rights {
    principal_keys(principal)
        .filter_map(|key| index.get(&key).copied())
        .fold(Rights::empty(), Rights::union)
}

/// What a [`ShareUpdate`] removes and keeps, by key: built once per
/// change.
#[derive(Debug, Default)]
pub(crate) struct UpdateIndex {
    /// Principals (every key), addresses and listed sharees removed or
    /// set without access.
    removed: HashSet<ShareeKey>,
    /// Those given access.
    kept: HashSet<ShareeKey>,
    replace: bool,
}

impl UpdateIndex {
    pub(crate) fn new(update: &ShareUpdate) -> Self {
        let mut index = Self {
            replace: update.replace,
            ..Self::default()
        };
        for (principal, rights) in &update.set {
            let to = if grants_access(*rights) {
                &mut index.kept
            } else {
                &mut index.removed
            };
            to.extend(principal_keys(principal));
        }
        for principal in &update.remove {
            index.removed.extend(principal_keys(principal));
        }
        for (address, rights) in &update.set_addresses {
            let to = if grants_access(*rights) {
                &mut index.kept
            } else {
                &mut index.removed
            };
            to.insert(address_key(address));
        }
        index
            .removed
            .extend(update.remove_addresses.iter().map(address_key));
        for (sharee, rights) in &update.set_sharees {
            let to = if grants_access(*rights) {
                &mut index.kept
            } else {
                &mut index.removed
            };
            to.insert(sharee_key(sharee));
        }
        index
            .removed
            .extend(update.remove_sharees.iter().map(sharee_key));
        index
    }

    /// Whether the change takes this key away (named, not by `replace`).
    pub(crate) fn removes(&self, key: &ShareeKey) -> bool {
        self.removed.contains(key)
    }

    /// Whether a listed sharee keeps access in a whole `shareWith`.
    pub(crate) fn keeps(&self, key: &ShareeKey) -> bool {
        self.kept.contains(key)
    }

    /// Whether the change takes a current sharee away: named, or not given
    /// access again in a whole `shareWith`.
    pub(crate) fn revokes(&self, key: &ShareeKey) -> bool {
        self.replace && !self.keeps(key) || self.removes(key)
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use crate::principals::PrincipalKind;

    /// The comparisons the keys replace, as they were.
    mod old {
        use super::*;

        pub(super) fn sharee_is_address(sharee: &Sharee, address: &CalendarAddress) -> bool {
            sharee
                .as_str()
                .get(..7)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
                && CalendarAddress::parse(sharee.as_str())
                    .is_some_and(|a| a.as_str().eq_ignore_ascii_case(address.as_str()))
        }

        pub(super) fn sharee_is(sharee: &Sharee, principal: &Principal) -> bool {
            let text = sharee.as_str();
            if text.starts_with("mailto:") {
                return CalendarAddress::parse(text).is_some_and(|a| principal.has_address(&a));
            }
            Url::parse(text)
                .ok()
                .and_then(|url| Href::from_url(&url).ok())
                .is_some_and(|href| href.same_location(&principal.href))
        }

        pub(super) fn same_sharee(a: &Sharee, b: &Sharee) -> bool {
            match (dav_sharee(a), dav_sharee(b)) {
                (Some(DavSharee::Principal(x)), Some(DavSharee::Principal(y))) => {
                    x.same_location(&y)
                }
                (Some(DavSharee::Address(x)), Some(DavSharee::Address(y))) => {
                    x.as_str().eq_ignore_ascii_case(y.as_str())
                }
                _ => a == b,
            }
        }
    }

    fn principal(url: &str, addresses: &[&str]) -> Principal {
        Principal {
            href: Href::from_url(&Url::parse(url).unwrap()).unwrap(),
            name: None,
            addresses: addresses
                .iter()
                .map(|a| CalendarAddress::parse(a).unwrap())
                .collect(),
            kind: PrincipalKind::Individual,
            own: false,
        }
    }

    #[test]
    fn keys_match_the_old_comparisons() {
        let texts = [
            "http://localhost:8080/dav/pal/bob/",
            "http://localhost:8080/dav/pal/bob",
            "http://LOCALHOST:8080/dav/pal/bob/",
            "http://localhost:80/dav/pal/bob/",
            "http://localhost/dav/pal/bob/",
            "http://localhost:8081/dav/pal/bob/",
            "https://localhost:8080/dav/pal/bob/",
            "http://localhost:8080/dav/pal/bob%40x.org/",
            "http://localhost:8080/dav/pal/bob@x.org/",
            "http://localhost:8080/dav/pal/Bob/",
            "mailto:bob@x.org",
            "mailto:BOB@X.org",
            "mailto:Bob@x.ORG",
            "mailto:carol@x.org",
            "urn:x-uid:bob",
            "urn:x-uid:BOB",
            "mailto:not-an-address",
        ];
        let sharees: Vec<Sharee> = texts.iter().filter_map(|t| Sharee::new(t).ok()).collect();
        assert!(sharees.len() >= 12, "{}", sharees.len());
        let principals = [
            principal("http://localhost:8080/dav/pal/bob", &["bob@x.org"]),
            principal("http://localhost:8080/dav/pal/bob%40x.org/", &[]),
            principal(
                "https://localhost:8080/dav/pal/bob/",
                &["carol@x.org", "mailto:BOB@x.org"],
            ),
        ];
        for a in &sharees {
            for b in &sharees {
                assert_eq!(
                    sharee_key(a) == sharee_key(b),
                    old::same_sharee(a, b),
                    "{} {}",
                    a.as_str(),
                    b.as_str()
                );
            }
            for p in &principals {
                let key = sharee_key(a);
                assert_eq!(
                    principal_keys(p).any(|k| k == key),
                    old::sharee_is(a, p),
                    "{} {}",
                    a.as_str(),
                    p.href.url()
                );
                for address in &p.addresses {
                    assert_eq!(
                        key == address_key(address),
                        old::sharee_is_address(a, address),
                        "{} {}",
                        a.as_str(),
                        address.as_str()
                    );
                }
            }
        }
    }
}
