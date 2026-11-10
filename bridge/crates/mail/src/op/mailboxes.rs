// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Changing the mailboxes of an email (`Email/set` with `mailboxIds`):
//! moves, copies and removals of its copies.
//!
//! - One mailbox out and one in: `UID MOVE`. With `COPYUID` (UIDPLUS) the
//!   email keeps its id; without it, the copy is removed and the target
//!   synced, so it shows up as a new email.
//! - Only new mailboxes: `UID COPY` from an existing copy (with `COPYUID`
//!   the email simply gains a mailbox; this also works without OBJECTID).
//! - Only mailboxes out: the copy there is expunged (`\Deleted` +
//!   `UID EXPUNGE`, which needs UIDPLUS: a plain EXPUNGE would also remove
//!   other messages marked `\Deleted`).

use std::collections::BTreeSet;

use nebula_imap::CopyUidMap;
use nebula_imap::proto::command::{StoreOperation, StoreResponse};
use nebula_imap::proto::types::{Capability, Flag, Uid};
use nebula_store::{EmailId, Location, Mailbox, MailboxId, MailboxSyncState, Recorded, UnixTime};

use super::{select_location, single};
use crate::MailError;
use crate::core::account::MailAccount;

/// The new UID of `uid` in a COPYUID answer.
fn copied_uid(
    map: Option<&CopyUidMap>,
    uid: Uid,
) -> Option<(nebula_imap::proto::types::UidValidity, Uid)> {
    let map = map?;
    map.pairs
        .iter()
        .find(|(source, _)| *source == uid)
        .map(|(_, destination)| (map.uid_validity, *destination))
}

/// How an email gets to its new mailboxes, in this order.
#[derive(Debug, PartialEq, Eq)]
struct Steps {
    /// `UID COPY` from `source` into these, first: `source` is still there.
    copies: Vec<Mailbox>,
    /// `UID MOVE` of a copy that leaves into a new mailbox.
    moves: Vec<(Location, Mailbox)>,
    /// Copies that leave with nowhere to go.
    expunges: Vec<Location>,
    /// Where the copies come from: a copy that stays if there is one.
    source: Location,
}

/// The steps from `locations` to `target` (`destinations`: the target
/// mailboxes the email is not in yet). Copies go first, from a location
/// still in place: copying from a copy already moved away would fail.
/// `None` without locations.
fn plan(
    locations: &[Location],
    target: &BTreeSet<MailboxId>,
    destinations: Vec<Mailbox>,
) -> Option<Steps> {
    let (staying, mut leaving): (Vec<&Location>, Vec<&Location>) = locations
        .iter()
        .partition(|location| target.contains(&location.mailbox));
    let source = staying.first().copied().or(locations.first())?.clone();
    let move_count = destinations.len().min(leaving.len());
    let copy_count = destinations.len().saturating_sub(move_count);
    let mut destinations = destinations.into_iter();
    let copies: Vec<Mailbox> = destinations.by_ref().take(copy_count).collect();
    let moves = leaving
        .drain(..move_count)
        .cloned()
        .zip(destinations)
        .collect();
    Some(Steps {
        copies,
        moves,
        expunges: leaving.into_iter().cloned().collect(),
        source,
    })
}

impl MailAccount {
    /// Puts an email in exactly these mailboxes.
    ///
    /// # Errors
    /// [`MailError::Invalid`] for an empty set or a mailbox that can't hold
    /// messages, [`MailError::NotFound`] for unknown ids,
    /// [`MailError::Unsupported`] when removing a copy needs UIDPLUS, and
    /// connection errors.
    pub async fn set_mailboxes(
        &self,
        email: EmailId,
        target: BTreeSet<MailboxId>,
    ) -> Result<Recorded, MailError> {
        if target.is_empty() {
            return Err(MailError::Invalid("an email needs at least one mailbox"));
        }
        // Held to the end: a second change waits and reads the locations
        // this one leaves (the last one wins, the email is never lost).
        let _membership = self.membership.lock().await;
        let locations = self.email_locations(email).await?;
        let current: BTreeSet<MailboxId> = locations.iter().map(|l| l.mailbox).collect();
        let mut destinations = Vec::new();
        for id in target.difference(&current) {
            let mailbox = self.store().mailbox(self.id, *id).await?;
            if !mailbox.selectable {
                return Err(MailError::Invalid("mailbox can't hold messages"));
            }
            destinations.push(mailbox);
        }
        let Some(steps) = plan(&locations, &target, destinations) else {
            return Err(MailError::NotFound("email"));
        };

        let mut recorded = Recorded::default();
        for destination in &steps.copies {
            recorded.merge(self.copy_to(email, &steps.source, destination).await?);
        }
        for (from, destination) in &steps.moves {
            recorded.merge(self.move_copy(email, from, destination).await?);
        }
        for location in &steps.expunges {
            recorded.merge(self.expunge_copy(location).await?);
        }
        Ok(recorded)
    }

    /// A mailbox never synced has no UIDVALIDITY yet: take it from COPYUID,
    /// so the first sync doesn't reset (and lose) the copy we add.
    ///
    /// Called with the connection held, and reads the state again: a sync
    /// may have stored it since `destination` was read (writing over it
    /// would drop its UIDNEXT and HIGHESTMODSEQ).
    async fn adopt_validity(
        &self,
        destination: &Mailbox,
        validity: nebula_imap::proto::types::UidValidity,
    ) -> Result<bool, MailError> {
        let current = self.store().mailbox(self.id, destination.id).await?;
        if let Some(known) = current.sync.uid_validity {
            return Ok(known == validity);
        }
        let state = MailboxSyncState {
            uid_validity: Some(validity),
            ..MailboxSyncState::default()
        };
        self.store()
            .set_mailbox_sync_state(self.id, destination.id, state)
            .await?;
        Ok(true)
    }

    async fn move_copy(
        &self,
        email: EmailId,
        from: &Location,
        destination: &Mailbox,
    ) -> Result<Recorded, MailError> {
        let mut slot = self.slot().await;
        let client = select_location(self, &mut slot, from).await?;
        let result = client.uid_move(&single(from), &destination.imap_name).await;
        let moved = slot.check(result)?;
        // The connection stays ours until the store agrees with the server:
        // a sync in between would see the copy gone from `from` and drop it
        // (then `move_message` fails with NotFound, seen in CI), or find the
        // new UID first and import it as another email.
        let now = UnixTime::now()?;
        let recorded = match copied_uid(moved.copy_uid.as_ref(), from.uid) {
            Some((validity, uid)) if self.adopt_validity(destination, validity).await? => {
                let recorded = self
                    .store()
                    .move_message(
                        self.id,
                        (from.mailbox, from.uid),
                        (destination.id, uid),
                        now,
                    )
                    .await?;
                drop(slot);
                recorded
            }
            _ => {
                // No COPYUID: we don't know the new UID. The copy leaves
                // here and comes back from the sync as a new email.
                let mut recorded = self
                    .store()
                    .remove_messages(self.id, from.mailbox, vec![from.uid], now)
                    .await?;
                // The sync takes the connection itself.
                drop(slot);
                self.emit(&recorded);
                recorded.merge(self.sync_mailbox(destination.id).await?);
                tracing::debug!(%email, "moved without COPYUID: the email gets a new id");
                recorded
            }
        };
        self.emit(&recorded);
        Ok(recorded)
    }

    async fn copy_to(
        &self,
        email: EmailId,
        source: &Location,
        destination: &Mailbox,
    ) -> Result<Recorded, MailError> {
        let mut slot = self.slot().await;
        let client = select_location(self, &mut slot, source).await?;
        let result = client
            .uid_copy(&single(source), &destination.imap_name)
            .await;
        let copied = slot.check(result)?;
        // As in `move_copy`: no sync between the COPY and the store write.
        let recorded = match copied_uid(copied.copy_uid.as_ref(), source.uid) {
            Some((validity, uid)) if self.adopt_validity(destination, validity).await? => {
                let recorded = self
                    .store()
                    .add_location(self.id, email, (destination.id, uid), UnixTime::now()?)
                    .await?;
                drop(slot);
                recorded
            }
            _ => {
                drop(slot);
                self.sync_mailbox(destination.id).await?
            }
        };
        self.emit(&recorded);
        Ok(recorded)
    }

    /// Removes one copy from the server: `\Deleted` + `UID EXPUNGE`.
    pub(crate) async fn expunge_copy(&self, location: &Location) -> Result<Recorded, MailError> {
        let mut slot = self.slot().await;
        let client = select_location(self, &mut slot, location).await?;
        if !client.capabilities().has(&Capability::UidPlus) {
            return Err(MailError::Unsupported("removing a message needs UIDPLUS"));
        }
        let set = single(location);
        let result = client
            .uid_store(
                &set,
                StoreOperation::Add,
                &[Flag::Deleted],
                StoreResponse::Silent,
                None,
            )
            .await;
        slot.check(result)?;
        let client = slot
            .selected()
            .ok_or(MailError::NotFound("selected mailbox"))?;
        let result = client.uid_expunge(&set).await;
        slot.check(result)?;
        // As in `move_copy`: no sync between the EXPUNGE and the store write.
        let recorded = self
            .store()
            .remove_messages(
                self.id,
                location.mailbox,
                vec![location.uid],
                UnixTime::now()?,
            )
            .await?;
        drop(slot);
        self.emit(&recorded);
        Ok(recorded)
    }
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::types::UidValidity;

    use super::*;

    fn location(mailbox: i64, uid: u32) -> Location {
        Location {
            mailbox: MailboxId::from_raw(mailbox).unwrap(),
            imap_name: nebula_imap::proto::types::MailboxName::new(&format!("M{mailbox}")).unwrap(),
            uid_validity: UidValidity::new(1),
            uid: Uid::new(uid).unwrap(),
            mod_seq: None,
        }
    }

    fn mailbox(id: i64) -> Mailbox {
        Mailbox {
            id: MailboxId::from_raw(id).unwrap(),
            imap_name: nebula_imap::proto::types::MailboxName::new(&format!("M{id}")).unwrap(),
            name: format!("M{id}"),
            delimiter: Some('/'),
            parent: None,
            role: None,
            subscribed: true,
            selectable: true,
            no_inferiors: false,
            sort_order: 0,
            sync: MailboxSyncState::default(),
        }
    }

    fn ids(list: &[i64]) -> BTreeSet<MailboxId> {
        list.iter()
            .map(|id| MailboxId::from_raw(*id).unwrap())
            .collect()
    }

    #[test]
    fn copies_come_from_a_copy_still_in_place() {
        // In L (1), to {D1 (2), D2 (3)}: copy L→D, then move L→the other.
        let steps = plan(
            &[location(1, 5)],
            &ids(&[2, 3]),
            vec![mailbox(2), mailbox(3)],
        )
        .unwrap();
        assert_eq!(steps.source, location(1, 5));
        assert_eq!(steps.copies, vec![mailbox(2)]);
        assert_eq!(steps.moves, vec![(location(1, 5), mailbox(3))]);
        assert!(steps.expunges.is_empty());

        // In {A (1), B (2)}, to {B, C (3), D (4)}: B stays and is the source.
        let steps = plan(
            &[location(1, 5), location(2, 6)],
            &ids(&[2, 3, 4]),
            vec![mailbox(3), mailbox(4)],
        )
        .unwrap();
        assert_eq!(steps.source, location(2, 6));
        assert_eq!(steps.copies, vec![mailbox(3)]);
        assert_eq!(steps.moves, vec![(location(1, 5), mailbox(4))]);
        assert!(steps.expunges.is_empty());

        // In {A, B}, to {C}: one move, one expunge, no copy.
        let steps = plan(
            &[location(1, 5), location(2, 6)],
            &ids(&[3]),
            vec![mailbox(3)],
        )
        .unwrap();
        assert!(steps.copies.is_empty());
        assert_eq!(steps.moves, vec![(location(1, 5), mailbox(3))]);
        assert_eq!(steps.expunges, vec![location(2, 6)]);

        assert_eq!(plan(&[], &ids(&[1]), vec![mailbox(1)]), None);
    }

    #[test]
    fn copyuid_lookup() {
        let uid = |n| Uid::new(n).unwrap();
        let map = CopyUidMap {
            uid_validity: UidValidity::new(9).unwrap(),
            pairs: vec![(uid(1), uid(100)), (uid(2), uid(101))],
        };
        assert_eq!(
            copied_uid(Some(&map), uid(2)),
            Some((UidValidity::new(9).unwrap(), uid(101)))
        );
        assert_eq!(copied_uid(Some(&map), uid(3)), None);
        assert_eq!(copied_uid(None, uid(1)), None);
    }
}
