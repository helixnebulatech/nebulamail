// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Syncing one mailbox.
//!
//! 1. **Plan** (one SELECT, under the connection lock):
//!    - UIDVALIDITY differs from the stored one → reset the mailbox in the
//!      store (its copies go; emails with a server id come back with the
//!      same id when found again).
//!    - QRESYNC with a stored HIGHESTMODSEQ: `SELECT … (QRESYNC …)` gives
//!      the expunged UIDs (VANISHED) and the changed flags directly.
//!    - CONDSTORE with a stored HIGHESTMODSEQ: `UID FETCH 1:* (FLAGS)
//!      (CHANGEDSINCE n)` for flags, and `UID SEARCH ALL` to see expunges.
//!    - Otherwise (first sync, interrupted sync, no CONDSTORE): `UID FETCH
//!      1:* (UID FLAGS)` and compare everything.
//!    - New messages: UIDs the store didn't know (from the comparisons) and,
//!      on the quick paths, `UID SEARCH UID <stored UIDNEXT>:*`.
//! 2. **Fetch** the metadata of new messages in batches, newest first, so
//!    the inbox is usable before old mail arrives. The lock is taken per
//!    batch: user actions don't wait for a whole first sync. Each batch is
//!    stored (one transaction) and broadcast at once.
//! 3. **Save** UIDVALIDITY, UIDNEXT and HIGHESTMODSEQ *as seen at SELECT*.
//!    Changes made during the sync have a higher MODSEQ/UID, so the next
//!    sync still sees them; and an interrupted sync saves nothing, so the
//!    next one does the full comparison.

use crate::op::report::Candidate;
use std::collections::BTreeSet;

use nebula_imap::proto::command::{
    FetchAttribute, FetchModifiers, QResyncParams, SearchKey, Section,
};
use nebula_imap::proto::response::FetchData;
use nebula_imap::proto::types::{
    Capabilities, Capability, SeqBound, SeqItem, Uid, UidSet, UidValidity,
};
use nebula_imap::{
    Client, ModSeqState, ObjectIdSupport, Resync, SelectOptions, SelectResult, Selected, SyncLevel,
};
use nebula_store::{
    FlagUpdate, MailboxId, MailboxSyncState, NewMessage, Recorded, StoredUid, UnixTime,
};

use crate::MailError;
use crate::core::account::MailAccount;
use crate::core::slot::{Reselect, Slot};
use crate::model::fetch::{self, Fetched, FlagChange};
use crate::model::headers;

/// How much a metadata FETCH asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataMode {
    /// Everything the server supports.
    Full,
    /// For messages whose full answer we could not read: no BODYSTRUCTURE
    /// (it may be nested too deep) and no PREVIEW (a quoted string with
    /// many `(` counts as too deep). The preview is then
    /// made from the text as for servers without PREVIEW.
    Minimal,
}

/// The attributes of a metadata FETCH, as far as the server supports them.
pub(crate) fn metadata_attributes(
    client: &Client<Selected>,
    mode: MetadataMode,
) -> Vec<FetchAttribute> {
    attribute_list(
        client.capabilities(),
        client.sync_level(),
        client.object_id_support(),
        mode,
    )
}

fn attribute_list(
    capabilities: &Capabilities,
    sync_level: SyncLevel,
    object_ids: ObjectIdSupport,
    mode: MetadataMode,
) -> Vec<FetchAttribute> {
    let full = mode == MetadataMode::Full;
    let mut list = vec![
        FetchAttribute::Uid,
        FetchAttribute::Flags,
        FetchAttribute::InternalDate,
        FetchAttribute::Rfc822Size,
    ];
    if full {
        list.push(FetchAttribute::BodyStructure);
    }
    list.push(FetchAttribute::BodyPeek {
        section: Section::HeaderFields(headers::list_fields()),
        partial: None,
    });
    if sync_level >= SyncLevel::CondStore {
        list.push(FetchAttribute::ModSeq);
    }
    match object_ids {
        ObjectIdSupport::Rfc8474 => {
            list.extend([FetchAttribute::EmailId, FetchAttribute::ThreadId]);
        }
        ObjectIdSupport::ObjectIdPlus => list.push(FetchAttribute::ObjectIds),
        _ => {}
    }
    if capabilities.has(&Capability::GmailExt1) {
        list.extend([FetchAttribute::GmailMsgId, FetchAttribute::GmailThreadId]);
    }
    if full && capabilities.has(&Capability::Preview) {
        list.push(FetchAttribute::Preview);
    }
    list
}

/// What the plan step found.
struct Plan {
    uid_validity: UidValidity,
    /// The mailbox was never synced completely.
    first: bool,
    /// New UIDs, newest first.
    new_uids: Vec<Uid>,
    /// The state to save when everything is fetched.
    finished: MailboxSyncState,
}

/// What the server said changed, before it is applied.
#[derive(Default)]
struct Delta {
    gone: BTreeSet<Uid>,
    updates: Vec<FlagUpdate>,
    /// UIDs on the server, when the plan listed them all.
    server: Option<BTreeSet<Uid>>,
}

impl Delta {
    fn take_flags(&mut self, data: &FetchData) {
        match fetch::flag_change(data) {
            Some(FlagChange::Update(update)) => self.updates.push(update),
            Some(FlagChange::Deleted(uid)) => {
                self.gone.insert(uid);
            }
            None => {}
        }
    }
}

/// Syncs one mailbox. Takes the account's sync lock.
pub(crate) async fn sync_mailbox(
    account: &MailAccount,
    id: MailboxId,
) -> Result<Recorded, MailError> {
    let _one_sync = account.sync_lock.lock().await;
    sync_locked(account, id).await
}

/// [`sync_mailbox`] for callers that already hold the sync lock.
pub(crate) async fn sync_locked(
    account: &MailAccount,
    id: MailboxId,
) -> Result<Recorded, MailError> {
    account.ensure_running()?;
    let mut recorded = Recorded::default();
    let Some(plan) = plan(account, id, &mut recorded).await? else {
        return Ok(recorded);
    };
    let Some(omitted) = fetch_new(account, id, &plan, &mut recorded).await? else {
        return Ok(recorded);
    };
    let mut finished = plan.finished;
    finished.uid_next = resume_uid_next(finished.uid_next, &omitted);
    account
        .store()
        .set_mailbox_sync_state(account.id, id, finished)
        .await?;
    Ok(recorded)
}

/// `None` for a mailbox that can't hold messages.
async fn plan(
    account: &MailAccount,
    id: MailboxId,
    recorded: &mut Recorded,
) -> Result<Option<Plan>, MailError> {
    let store = account.store();
    let now = UnixTime::now()?;
    let mut slot = account.slot().await;
    // Read with the connection ours, never before: an operation that held
    // it meanwhile may have written the state. A MOVE into a mailbox never
    // synced stores its UIDVALIDITY from COPYUID (`adopt_validity`) and the
    // moved copy; a state read before that still has no UIDVALIDITY, so
    // the mailbox was reset and the moved email imported again under a new
    // id (seen in CI with Dovecot: the live sync of the new mailbox, after
    // NOTIFY, waited for the connection while the MOVE ran).
    let mailbox = store.mailbox(account.id, id).await?;
    if !mailbox.selectable {
        return Ok(None);
    }
    let known = mailbox.sync;

    let params = match (known.uid_validity, known.highest_mod_seq) {
        (Some(uid_validity), Some(mod_seq)) => Some(QResyncParams {
            uid_validity,
            mod_seq,
            known_uids: None,
        }),
        _ => None,
    };
    let options = SelectOptions {
        resync: params.clone().map_or(Resync::Auto, Resync::QResync),
        ..SelectOptions::default()
    };
    // Leave the mailbox first if it is selected. A QRESYNC SELECT from a
    // selected mailbox gets `[CLOSED]`, and Stalwart sends the changes of
    // the new mailbox *before* it, so they were taken for the old one's and
    // dropped (`Client<Selected>::select` now unselects first for any
    // mailbox; this keeps it explicit for the same one).
    slot.leave(&mailbox.imap_name).await?;
    let (client, selected) = slot
        .select(account, &mailbox.imap_name, options, Reselect::Always)
        .await?;
    // Unsolicited news from before is about to be superseded.
    client.take_events();
    let state = client.mailbox().clone();
    let selected = selected.unwrap_or_default();

    let known = check_validity(account, id, known, state.uid_validity, now, recorded).await?;
    // From the state after the UIDVALIDITY check: a reset mailbox is copied
    // again from scratch.
    let first = is_first_copy(&known);
    let stored = store.mailbox_uids(account.id, id).await?;
    let stored_uids: BTreeSet<Uid> = stored.iter().map(|s| s.uid).collect();

    let quick = known.highest_mod_seq.is_some() && known.uid_next.is_some();
    let used = selected.used;
    let delta = if quick && used == Some(SyncLevel::QResync) && params.is_some() {
        Ok(from_qresync(&selected, &stored))
    } else if let (true, Some(since)) = (quick && used.is_some(), known.highest_mod_seq) {
        from_condstore(&mut slot, since, state.exists, &stored_uids).await
    } else {
        full_compare(&mut slot, state.exists, &stored_uids).await
    }?;

    let mut new_uids: BTreeSet<Uid> = match &delta.server {
        Some(server) => server.difference(&stored_uids).copied().collect(),
        None => BTreeSet::new(),
    };
    if quick && let Some(uid_next) = known.uid_next {
        new_uids.extend(uids_from(&mut slot, uid_next, state.exists, &stored_uids).await?);
    }

    // Applied with the connection still ours: an operation (move, flags…)
    // in between would write its result first and then have it undone by
    // what we read before it. Operations keep the connection until their
    // store write too, so reads and writes of both never interleave.
    let Delta { gone, updates, .. } = delta;
    if !gone.is_empty() {
        let removed = store
            .remove_messages(account.id, id, gone.iter().copied().collect(), now)
            .await?;
        account.emit(&removed);
        recorded.merge(removed);
    }
    if !updates.is_empty() {
        let applied = store.update_flags(account.id, id, updates, now).await?;
        account.emit(&applied.recorded);
        recorded.merge(applied.recorded);
        new_uids.extend(applied.unknown);
    }
    for uid in &gone {
        new_uids.remove(uid);
    }
    drop(slot);

    let highest_mod_seq = match state.mod_seq {
        ModSeqState::Highest(mod_seq) => Some(mod_seq),
        _ => None,
    };
    Ok(Some(Plan {
        uid_validity: state.uid_validity,
        first,
        new_uids: new_uids.into_iter().rev().collect(),
        finished: MailboxSyncState {
            uid_validity: Some(state.uid_validity),
            uid_next: state.uid_next,
            highest_mod_seq,
            messages: Some(state.exists),
            synced_at: Some(now),
        },
    }))
}

/// A mailbox never synced completely is being copied, not receiving mail:
/// no `EmailDelivery` push for that (Web Push would wake every browser
/// once per batch).
fn is_first_copy(state: &MailboxSyncState) -> bool {
    state.synced_at.is_none()
}

/// Most FETCH responses taken from one listing of a mailbox, whatever
/// EXISTS says (the server sets EXISTS too).
const MAX_MESSAGES_PER_MAILBOX: usize = 4_000_000;

/// Whether a listing that got `count` FETCH responses for a mailbox of
/// `exists` messages is more than a real server sends (each message once,
/// plus some unsolicited updates): a hostile server repeating FETCH.
fn over_limit(count: usize, exists: u32) -> bool {
    let expected = usize::try_from(exists)
        .unwrap_or(usize::MAX)
        .saturating_mul(2)
        .saturating_add(1024)
        .min(MAX_MESSAGES_PER_MAILBOX);
    count > expected
}

/// Too many FETCH responses (see [`over_limit`]).
const TOO_MANY_FETCH: MailError =
    MailError::Invalid("server sent more FETCH responses than messages");

/// A UIDVALIDITY other than the stored one (or the first one) resets the
/// mailbox in the store; returns the state to sync from.
async fn check_validity(
    account: &MailAccount,
    id: MailboxId,
    known: MailboxSyncState,
    uid_validity: UidValidity,
    now: UnixTime,
    recorded: &mut Recorded,
) -> Result<MailboxSyncState, MailError> {
    if known.uid_validity == Some(uid_validity) {
        return Ok(known);
    }
    if known.uid_validity.is_some() {
        tracing::info!(mailbox = %id, "UIDVALIDITY changed: resyncing the mailbox");
    }
    let reset = account
        .store()
        .reset_mailbox(account.id, id, uid_validity, now)
        .await?;
    account.emit(&reset);
    recorded.merge(reset);
    Ok(MailboxSyncState {
        uid_validity: Some(uid_validity),
        ..MailboxSyncState::default()
    })
}

/// QRESYNC: the SELECT already said what vanished and what changed.
fn from_qresync(selected: &SelectResult, stored: &[StoredUid]) -> Delta {
    let mut delta = Delta {
        gone: vanished_among(&selected.vanished, stored.iter().map(|copy| copy.uid)),
        ..Delta::default()
    };
    for data in &selected.changed {
        delta.take_flags(data);
    }
    delta
}

/// The stored UIDs inside the VANISHED sets. The ranges are sorted and
/// merged once and each UID found by binary search: O((S + R) log R), not
/// S × R (a server can send thousands of disjoint ranges). Items with `*`
/// match nothing, as in [`UidSet::contains`].
fn vanished_among(sets: &[UidSet], stored: impl Iterator<Item = Uid>) -> BTreeSet<Uid> {
    let mut ranges: Vec<(Uid, Uid)> = sets
        .iter()
        .flat_map(UidSet::items)
        .filter_map(|item| match *item {
            SeqItem::Single(SeqBound::Value(uid)) => Some((uid, uid)),
            SeqItem::Range(SeqBound::Value(a), SeqBound::Value(b)) => Some((a.min(b), a.max(b))),
            _ => None,
        })
        .collect();
    ranges.sort_unstable();
    let mut merged: Vec<(Uid, Uid)> = Vec::with_capacity(ranges.len());
    for (low, high) in ranges {
        match merged.last_mut() {
            Some((_, last)) if low <= *last || last.get().checked_add(1) == Some(low.get()) => {
                *last = (*last).max(high);
            }
            _ => merged.push((low, high)),
        }
    }
    stored
        .filter(|uid| {
            // The last range starting at or before `uid`.
            let after = merged.partition_point(|(low, _)| low <= uid);
            after
                .checked_sub(1)
                .and_then(|index| merged.get(index))
                .is_some_and(|(_, high)| uid <= high)
        })
        .collect()
}

/// CONDSTORE: changed flags since the stored mod-sequence, and the whole
/// UID list to find expunged messages.
async fn from_condstore(
    slot: &mut Slot,
    since: nebula_imap::proto::types::ModSeq,
    exists: u32,
    stored: &BTreeSet<Uid>,
) -> Result<Delta, MailError> {
    let mut delta = Delta::default();
    let server = all_uids(slot, exists).await?;
    if exists > 0 {
        let client = slot
            .selected()
            .ok_or(MailError::NotFound("selected mailbox"))?;
        let attributes = [
            FetchAttribute::Uid,
            FetchAttribute::Flags,
            FetchAttribute::ModSeq,
        ];
        // Each response is used and dropped at once; past the limit
        // nothing more is kept.
        let mut count = 0_usize;
        let result = client
            .uid_fetch_each(
                &UidSet::all(),
                &attributes,
                FetchModifiers::ChangedSince(since),
                |data| {
                    count = count.saturating_add(1);
                    if !over_limit(count, exists) {
                        delta.take_flags(&data);
                    }
                },
            )
            .await;
        let summary = slot.check(result)?;
        if over_limit(count, exists) {
            return Err(TOO_MANY_FETCH);
        }
        complete(summary.skipped)?;
    }
    delta.gone.extend(stored.difference(&server).copied());
    delta.server = Some(server);
    Ok(delta)
}

/// No usable mod-sequence: every UID and its flags.
async fn full_compare(
    slot: &mut Slot,
    exists: u32,
    stored: &BTreeSet<Uid>,
) -> Result<Delta, MailError> {
    let mut delta = Delta::default();
    let mut server = BTreeSet::new();
    if exists > 0 {
        let client = slot
            .selected()
            .ok_or(MailError::NotFound("selected mailbox"))?;
        let mut attributes = vec![FetchAttribute::Uid, FetchAttribute::Flags];
        if client.sync_level() >= SyncLevel::CondStore {
            attributes.push(FetchAttribute::ModSeq);
        }
        // Only the UID of each response is kept; past the limit not even
        // that.
        let mut count = 0_usize;
        let result = client
            .uid_fetch_each(&UidSet::all(), &attributes, FetchModifiers::None, |data| {
                count = count.saturating_add(1);
                if over_limit(count, exists) {
                    return;
                }
                if let Some(uid) = data.uid {
                    server.insert(uid);
                    // Only known UIDs: the new ones get a full FETCH anyway.
                    if stored.contains(&uid) {
                        delta.take_flags(&data);
                    }
                }
            })
            .await;
        let end = slot.check(result)?;
        if over_limit(count, exists) {
            return Err(TOO_MANY_FETCH);
        }
        complete(end.skipped)?;
        if end.unreadable > 0 {
            // Some FETCH answers could not be read: their UIDs are missing
            // from `server`, and would be taken as expunged. Ask the list
            // of UIDs instead (UID SEARCH ALL fails rather than come back
            // partial).
            tracing::warn!(
                count = end.unreadable,
                "unreadable FETCH responses: listing UIDs with SEARCH"
            );
            server = all_uids(slot, exists).await?;
        }
    }
    delta.gone.extend(stored.difference(&server).copied());
    delta.server = Some(server);
    Ok(delta)
}

/// The plan's FETCH of UIDs and flags must be whole: a dropped response
/// would read as an expunged message (or lose a flag change). Only UID,
/// FLAGS and MODSEQ are asked, which a sender cannot make large, so this
/// only fails with a hostile or broken server.
fn complete(skipped: usize) -> Result<(), MailError> {
    if skipped == 0 {
        Ok(())
    } else {
        Err(MailError::from(nebula_imap::Error::ResponseTooLarge(
            "UID FETCH",
        )))
    }
}

async fn all_uids(slot: &mut Slot, exists: u32) -> Result<BTreeSet<Uid>, MailError> {
    if exists == 0 {
        return Ok(BTreeSet::new());
    }
    let client = slot
        .selected()
        .ok_or(MailError::NotFound("selected mailbox"))?;
    let result = client.uid_search(&SearchKey::All).await;
    Ok(slot.check(result)?.uids().into_iter().collect())
}

/// UIDs at or above `first` that the store doesn't have. (`n:*` also
/// matches the last message when `n` is above every UID: filtered out.)
async fn uids_from(
    slot: &mut Slot,
    first: Uid,
    exists: u32,
    stored: &BTreeSet<Uid>,
) -> Result<Vec<Uid>, MailError> {
    if exists == 0 {
        return Ok(Vec::new());
    }
    let client = slot
        .selected()
        .ok_or(MailError::NotFound("selected mailbox"))?;
    let result = client
        .uid_search(&SearchKey::Uid(UidSet::from_value(first)))
        .await;
    Ok(slot
        .check(result)?
        .uids()
        .into_iter()
        .filter(|uid| *uid >= first && !stored.contains(uid))
        .collect())
}

/// The UIDNEXT to save: the one seen at SELECT, or the lowest UID that
/// could not be fetched, if lower. The quick paths only look for new
/// messages from the saved UIDNEXT on (`uids_from`, which skips the stored
/// ones), so a message left out would otherwise never be fetched again;
/// this way the next sync asks again for it, and only for it.
fn resume_uid_next(seen: Option<Uid>, omitted: &[Uid]) -> Option<Uid> {
    match (seen, omitted.iter().min()) {
        (Some(seen), Some(&lowest)) => Some(seen.min(lowest)),
        (None, Some(&lowest)) => Some(lowest),
        (seen, None) => seen,
    }
}

/// Fetches and stores the new messages. Returns the UIDs that could not be
/// fetched (see [`fetch_metadata`]), or `None` if the mailbox changed under
/// us (UIDVALIDITY): the next sync starts over.
async fn fetch_new(
    account: &MailAccount,
    id: MailboxId,
    plan: &Plan,
    recorded: &mut Recorded,
) -> Result<Option<Vec<Uid>>, MailError> {
    let mailbox = account.store().mailbox(account.id, id).await?;
    let batch = account.config().batch_size.max(1);
    let mut reports = Vec::new();
    let mut omitted = Vec::new();
    for chunk in plan.new_uids.chunks(batch) {
        account.ensure_running()?;
        let mut slot = account.slot().await;
        let (client, _) = slot
            .select(
                account,
                &mailbox.imap_name,
                SelectOptions::default(),
                Reselect::IfOther,
            )
            .await?;
        if client.mailbox().uid_validity != plan.uid_validity {
            return Ok(None);
        }
        let Metadata {
            messages,
            omitted: left_out,
        } = fetch_metadata(&mut slot, chunk).await?;
        omitted.extend(left_out);
        if messages.is_empty() {
            continue;
        }
        let now = UnixTime::now()?;
        reports.extend(messages.iter().filter_map(Candidate::of));
        // Stored before letting go of the connection, as in `plan`.
        let mut added = account
            .store()
            .add_messages(account.id, id, messages, now)
            .await?;
        if plan.first {
            added.forget(nebula_store::DataType::EmailDelivery);
        }
        drop(slot);
        account.emit(&added);
        recorded.merge(added);
    }
    // Delivery reports and read receipts for our submissions.
    if !reports.is_empty() {
        account.apply_reports(id, reports).await;
    }
    Ok(Some(omitted))
}

/// What [`fetch_metadata`] got.
pub(crate) struct Metadata {
    /// The messages, ready to store.
    pub(crate) messages: Vec<NewMessage>,
    /// Asked for, but not in any usable answer: expunged meanwhile, or
    /// still over the limits without BODYSTRUCTURE.
    pub(crate) omitted: Vec<Uid>,
}

/// Metadata of these UIDs. Messages the server answered without a
/// readable FETCH (a BODYSTRUCTURE nested too deep for the parser, or so
/// large that the response was dropped, a PREVIEW with many `(`) are asked
/// again in [`MetadataMode::Minimal`]; they get `hasAttachment: false`. A
/// message still missing after that is left out with a warning, so it does
/// not block the rest (an accepted risk).
pub(crate) async fn fetch_metadata(slot: &mut Slot, uids: &[Uid]) -> Result<Metadata, MailError> {
    let wanted: BTreeSet<Uid> = uids.iter().copied().collect();
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    for mode in [MetadataMode::Full, MetadataMode::Minimal] {
        let missing: Vec<Uid> = wanted.difference(&seen).copied().collect();
        let Some(set) = UidSet::from_values(missing) else {
            break;
        };
        let client = slot
            .selected()
            .ok_or(MailError::NotFound("selected mailbox"))?;
        let attributes = metadata_attributes(client, mode);
        let result = client
            .uid_fetch(&set, &attributes, FetchModifiers::None)
            .await;
        let now = UnixTime::now()?;
        let mut without_header = Vec::new();
        for data in slot.check(result)?.messages {
            // Unsolicited FETCH responses about other messages: skip.
            let Some(uid) = data.uid.filter(|uid| wanted.contains(uid)) else {
                continue;
            };
            // An unsolicited FLAGS update for this message may come before
            // the answer: wait for the answer.
            if !fetch::is_metadata_answer(&data) {
                if mode == MetadataMode::Minimal {
                    without_header.push(data);
                }
                continue;
            }
            if !seen.insert(uid) {
                continue;
            }
            if let Some(Fetched::Message(message)) = fetch::message(&data, now) {
                found.push(*message);
            }
        }
        // A broken server that never sends the header: take what it sent
        // (as before), or the message would be asked for at every sync.
        for data in without_header {
            let Some(uid) = data.uid.filter(|uid| seen.insert(*uid)) else {
                continue;
            };
            tracing::warn!(%uid, "metadata FETCH answered without the header");
            if let Some(Fetched::Message(message)) = fetch::message(&data, now) {
                found.push(*message);
            }
        }
    }
    let omitted: Vec<Uid> = wanted.difference(&seen).copied().collect();
    for uid in &omitted {
        tracing::warn!(
            uid = uid.get(),
            "IMAP message without usable metadata: left out"
        );
    }
    Ok(Metadata {
        messages: found,
        omitted,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code: a panic is a failed test")]

    use nebula_imap::proto::types::ModSeq;

    use super::*;
    use crate::test_support::{NOW, closed_port, fixture};

    #[test]
    fn fetch_limit() {
        assert!(!over_limit(10, 10));
        assert!(!over_limit(1044, 10));
        assert!(over_limit(1045, 10));
        assert!(!over_limit(0, 0));
        // EXISTS comes from the server: it can't lift the cap.
        assert!(!over_limit(MAX_MESSAGES_PER_MAILBOX, u32::MAX));
        assert!(over_limit(MAX_MESSAGES_PER_MAILBOX + 1, u32::MAX));
        assert!(over_limit(usize::MAX, u32::MAX));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_uidvalidity_is_a_first_copy() {
        let f = fixture(closed_port()).await;
        let account = f.account();
        let synced = MailboxSyncState {
            uid_validity: UidValidity::new(7),
            uid_next: Uid::new(10),
            highest_mod_seq: ModSeq::new(5),
            messages: Some(9),
            synced_at: Some(NOW),
        };
        f.store()
            .set_mailbox_sync_state(f.account, f.inbox, synced)
            .await
            .unwrap();
        let mut recorded = Recorded::default();
        let same = check_validity(
            &account,
            f.inbox,
            synced,
            UidValidity::new(7).unwrap(),
            NOW,
            &mut recorded,
        )
        .await
        .unwrap();
        assert!(!is_first_copy(&same));
        let other = check_validity(
            &account,
            f.inbox,
            synced,
            UidValidity::new(8).unwrap(),
            NOW,
            &mut recorded,
        )
        .await
        .unwrap();
        assert!(is_first_copy(&other));
        let stored = f.store().mailbox(f.account, f.inbox).await.unwrap().sync;
        assert!(is_first_copy(&stored));
    }

    fn uid(value: u32) -> Uid {
        Uid::new(value).unwrap()
    }

    #[test]
    fn vanished_ranges_are_merged_and_searched() {
        let sets = vec![
            UidSet::range(uid(5), uid(9)),
            UidSet::from_values([uid(20), uid(21), uid(30)]).unwrap(),
            UidSet::range(uid(8), uid(12)),
            UidSet::from_value(uid(100)),
        ];
        let stored = [1, 5, 10, 12, 13, 21, 22, 30, 150].map(uid);
        let gone = vanished_among(&sets, stored.into_iter());
        assert_eq!(
            gone.into_iter().map(Uid::get).collect::<Vec<_>>(),
            vec![5, 10, 12, 21, 30]
        );
    }

    #[test]
    fn many_disjoint_vanished_ranges() {
        // 10,000 disjoint ranges (2k+1 .. 2k+1, every odd UID) and 10,000
        // stored UIDs (1..=10,000): exactly the odd ones vanished.
        let sets: Vec<UidSet> = (0..10_000u32)
            .map(|k| UidSet::single(uid(2 * k + 1)))
            .collect();
        let gone = vanished_among(&sets, (1..=10_000).map(uid));
        assert_eq!(gone.len(), 5_000);
        assert!(gone.iter().all(|uid| uid.get() % 2 == 1));
    }

    #[test]
    fn minimal_metadata_has_no_structure_nor_preview() {
        let capabilities = Capabilities::parse_atoms(["IMAP4rev1", "PREVIEW", "CONDSTORE"]);
        let full = attribute_list(
            &capabilities,
            SyncLevel::CondStore,
            ObjectIdSupport::None,
            MetadataMode::Full,
        );
        assert!(full.contains(&FetchAttribute::BodyStructure));
        assert!(full.contains(&FetchAttribute::Preview));
        let minimal = attribute_list(
            &capabilities,
            SyncLevel::CondStore,
            ObjectIdSupport::None,
            MetadataMode::Minimal,
        );
        assert!(!minimal.contains(&FetchAttribute::BodyStructure));
        assert!(!minimal.contains(&FetchAttribute::Preview));
        assert!(minimal.contains(&FetchAttribute::Uid));
        assert!(minimal.contains(&FetchAttribute::ModSeq));
    }

    #[test]
    fn omitted_messages_are_asked_again() {
        // Nothing left out: the UIDNEXT seen at SELECT.
        assert_eq!(resume_uid_next(Some(uid(50)), &[]), Some(uid(50)));
        assert_eq!(resume_uid_next(None, &[]), None);
        // Left out: the next quick sync starts its search from the lowest.
        assert_eq!(
            resume_uid_next(Some(uid(50)), &[uid(42), uid(40)]),
            Some(uid(40))
        );
        assert_eq!(resume_uid_next(None, &[uid(7)]), Some(uid(7)));
        // A UID at or above UIDNEXT (should not happen) changes nothing.
        assert_eq!(resume_uid_next(Some(uid(50)), &[uid(60)]), Some(uid(50)));
    }
}
