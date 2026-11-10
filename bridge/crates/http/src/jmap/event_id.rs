// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Event ids of `EventSource` (RFC 8620 §7.3): after each `state` event
//! the server SHOULD send "a new event id that encodes the entire server
//! state visible to the user"; a client that reconnects sends the last one
//! in `Last-Event-ID` and the server SHOULD send at once what it missed.
//!
//! The id is opaque to the client, bounded, and plain ASCII (the SSE
//! `id` field can't hold LF, CR or NUL): `1.<own>.<digest>`, where
//!
//! - `own` is the unpadded base64url JSON of the connection's account
//!   states (`{"Email":"s7",…}`), or empty if that JSON is longer than
//!   [`MAX_OWN_JSON`];
//! - `digest` is the first 16 bytes of the SHA-256 of the JSON of every
//!   state the connection sees (its shared accounts too), unpadded
//!   base64url.
//!
//! On reconnection: the same digest, nothing missed. Otherwise the types
//! whose own state differs go (in every account that has them); if none
//! differs, or the id can't be read, every state goes. A client that gets
//! a state it already has does nothing (RFC 8620 §7.1: it compares the
//! state with its own). States of shared accounts are not in `own`: in
//! the bridge they change with the account's own groupware states, which
//! are.

use std::collections::BTreeMap;

use data_encoding::BASE64URL_NOPAD;
use nebula_jmap_proto::push::StateChange;
use nebula_jmap_proto::types::DataType;
use nebula_jmap_proto::{AccountId as JmapAccountId, State};
use sha2::{Digest as _, Sha256};

/// The id's version, its first part.
const VERSION: &str = "1";
/// Longest JSON of the own states kept in an id (the id is then at most
/// about 1.4 KB).
const MAX_OWN_JSON: usize = 1024;
/// Longest `Last-Event-ID` read; a longer one is not one of ours.
pub(crate) const MAX_LAST_EVENT_ID: usize = 2048;
/// Bytes of the digest kept.
const DIGEST_LEN: usize = 16;

type States = BTreeMap<DataType, State>;

/// The states a connection has seen: the base of its event ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Seen(StateChange);

/// A `Last-Event-ID` of ours, read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EventId {
    /// `None`: they didn't fit in the id.
    own: Option<States>,
    digest: [u8; DIGEST_LEN],
}

fn digest(change: &StateChange) -> [u8; DIGEST_LEN] {
    // `BTreeMap`s: the same states always give the same JSON.
    let json = serde_json::to_vec(&change.changed).unwrap_or_default();
    let full = Sha256::digest(&json);
    let mut out = [0; DIGEST_LEN];
    out.copy_from_slice(full.get(..DIGEST_LEN).unwrap_or(&[0; DIGEST_LEN]));
    out
}

impl EventId {
    fn parse(text: &str) -> Option<Self> {
        if text.len() > MAX_LAST_EVENT_ID {
            return None;
        }
        let mut parts = text.split('.');
        if parts.next()? != VERSION {
            return None;
        }
        let own = parts.next()?;
        let digest = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let own = if own.is_empty() {
            None
        } else {
            let json = BASE64URL_NOPAD.decode(own.as_bytes()).ok()?;
            Some(serde_json::from_slice(&json).ok()?)
        };
        let digest = BASE64URL_NOPAD.decode(digest.as_bytes()).ok()?;
        Some(Self {
            own,
            digest: digest.try_into().ok()?,
        })
    }
}

impl Seen {
    /// The states the connection starts with.
    pub(crate) const fn new(current: StateChange) -> Self {
        Self(current)
    }

    /// Takes in the new states of a `state` event.
    pub(crate) fn merge(&mut self, change: &StateChange) {
        for (account, states) in &change.changed {
            let known = self.0.changed.entry(account.clone()).or_default();
            for (data_type, state) in states {
                known.insert(data_type.clone(), state.clone());
            }
        }
    }

    /// The event id of these states.
    pub(crate) fn id(&self, own: &JmapAccountId) -> String {
        let own_json = self
            .0
            .changed
            .get(own)
            .and_then(|states| serde_json::to_vec(states).ok())
            .filter(|json| json.len() <= MAX_OWN_JSON)
            .map(|json| BASE64URL_NOPAD.encode(&json))
            .unwrap_or_default();
        format!(
            "{VERSION}.{own_json}.{}",
            BASE64URL_NOPAD.encode(&digest(&self.0))
        )
    }

    /// What a client whose last event id was `last` missed, if anything.
    pub(crate) fn missed(&self, own: &JmapAccountId, last: &str) -> Option<StateChange> {
        let everything = || (!self.0.changed.is_empty()).then(|| self.0.clone());
        let Some(last) = EventId::parse(last) else {
            return everything();
        };
        if last.digest == digest(&self.0) {
            return None;
        }
        let Some(old) = last.own else {
            return everything();
        };
        let differ: Vec<&DataType> = self
            .0
            .changed
            .get(own)
            .into_iter()
            .flatten()
            .filter(|(data_type, state)| old.get(*data_type) != Some(*state))
            .map(|(data_type, _)| data_type)
            .collect();
        if differ.is_empty() {
            return everything();
        }
        let changed: BTreeMap<_, _> = self
            .0
            .changed
            .iter()
            .filter_map(|(account, states)| {
                let states: States = states
                    .iter()
                    .filter(|(data_type, _)| differ.contains(data_type))
                    .map(|(data_type, state)| (data_type.clone(), state.clone()))
                    .collect();
                (!states.is_empty()).then(|| (account.clone(), states))
            })
            .collect();
        Some(StateChange { changed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: &str) -> JmapAccountId {
        id.parse().unwrap()
    }

    fn change(entries: &[(&str, DataType, &str)]) -> StateChange {
        let mut change = StateChange::default();
        for (id, data_type, state) in entries {
            change
                .changed
                .entry(account(id))
                .or_default()
                .insert(data_type.clone(), State::new(*state));
        }
        change
    }

    fn start() -> Seen {
        Seen::new(change(&[
            ("a1", DataType::Email, "e1"),
            ("a1", DataType::Mailbox, "m1"),
            ("a1", DataType::Calendar, "c1"),
            ("a1-r7", DataType::Calendar, "c1"),
        ]))
    }

    /// Short, ASCII, nothing the SSE `id` field can't hold.
    #[test]
    fn ids_are_compact_and_opaque() {
        let id = start().id(&account("a1"));
        assert!(id.starts_with("1."));
        assert!(id.len() < 120, "{id}");
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        );
        assert_eq!(start().id(&account("a1")), id);
    }

    #[test]
    fn nothing_missed_with_the_same_states() {
        let seen = start();
        let me = account("a1");
        assert_eq!(seen.missed(&me, &seen.id(&me)), None);
    }

    /// Only the types that changed, in every account that has them.
    #[test]
    fn only_what_changed() {
        let me = account("a1");
        let old = start().id(&me);
        let mut seen = start();
        seen.merge(&change(&[
            ("a1", DataType::Calendar, "c2"),
            ("a1-r7", DataType::Calendar, "c2"),
        ]));
        assert_eq!(
            seen.missed(&me, &old),
            Some(change(&[
                ("a1", DataType::Calendar, "c2"),
                ("a1-r7", DataType::Calendar, "c2"),
            ]))
        );
        // A type the old id didn't have counts as changed.
        let mut more = start();
        more.merge(&change(&[("a1", DataType::Identity, "i1")]));
        assert_eq!(
            more.missed(&me, &old),
            Some(change(&[("a1", DataType::Identity, "i1")]))
        );
    }

    /// An id that isn't ours, or one that doesn't say which own state
    /// changed: everything.
    #[test]
    fn unreadable_ids_get_everything() {
        let me = account("a1");
        let seen = start();
        let all = Some(seen.0.clone());
        for bad in [
            "",
            "garbage",
            "2.e30.AAAAAAAAAAAAAAAAAAAAAA",
            "1.e30",
            "1.e30.AAAA",
            "1.!!.AAAAAAAAAAAAAAAAAAAAAA",
            "1.e30.AAAAAAAAAAAAAAAAAAAAAA.x",
            &"1".repeat(MAX_LAST_EVENT_ID + 1),
        ] {
            assert_eq!(seen.missed(&me, bad), all, "{bad:?}");
        }
        // Readable, own states the same, a shared account different.
        let old = seen.id(&me);
        let mut shared = start();
        shared.merge(&change(&[("a1-r9", DataType::Calendar, "c1")]));
        assert_eq!(shared.missed(&me, &old), Some(shared.0.clone()));
        // Nothing to send when there are no states at all.
        assert_eq!(Seen::default().missed(&me, "garbage"), None);
    }

    /// Own states too long for the id: left out, and a different digest
    /// sends everything.
    #[test]
    fn long_own_states_are_left_out() {
        let me = account("a1");
        let long = "x".repeat(MAX_OWN_JSON);
        let seen = Seen::new(change(&[("a1", DataType::Email, &long)]));
        let id = seen.id(&me);
        assert!(id.starts_with("1.."), "{id}");
        assert!(id.len() < 40);
        assert_eq!(seen.missed(&me, &id), None);
        let mut newer = seen.clone();
        newer.merge(&change(&[("a1", DataType::Email, "short")]));
        assert_eq!(newer.missed(&me, &id), Some(newer.0.clone()));
    }
}
