// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! SELECT / EXAMINE, with CONDSTORE and QRESYNC (RFC 7162).

use nebula_imap_proto::command::{Access, QResyncParams, Select, SelectSync};
use nebula_imap_proto::response::{FetchData, NewKeywords, ResponseCode, Untagged, VanishedKind};
use nebula_imap_proto::types::{Capability, Flag, MailboxName, Uid, UidSet, UidValidity};

use crate::core::client::{
    Authenticated, Client, MailboxState, ModSeqState, PermanentFlags, Selected, SyncLevel,
    sealed::State,
};
use crate::core::connection::{StrayContinuation, expect_ok};
use crate::{Error, StateError};

/// How to resynchronise when selecting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Resync {
    /// `(CONDSTORE)` when available, to learn HIGHESTMODSEQ.
    #[default]
    Auto,
    /// No extension.
    Plain,
    /// `(QRESYNC (...))`: report what changed since the given state. Falls
    /// back to CONDSTORE if QRESYNC is not enabled (see
    /// [`SelectResult::used`]).
    QResync(QResyncParams),
}

/// Options of [`Client::select`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOptions {
    /// SELECT (read-write) or EXAMINE (read-only).
    pub access: Access,
    /// Resynchronisation.
    pub resync: Resync,
}

impl Default for SelectOptions {
    fn default() -> Self {
        Self {
            access: Access::ReadWrite,
            resync: Resync::Auto,
        }
    }
}

/// What a QRESYNC select reported, besides the mailbox state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct SelectResult {
    /// The extension actually used.
    pub used: Option<SyncLevel>,
    /// UIDs expunged since the known state (`VANISHED (EARLIER)`).
    pub vanished: Vec<UidSet>,
    /// Messages changed since the known mod-sequence (flags, MODSEQ).
    pub changed: Vec<FetchData>,
}

/// Collects the untagged data of a SELECT.
#[derive(Default)]
struct Collected {
    /// Whether this SELECT uses QRESYNC: only then are FETCH and `VANISHED
    /// (EARLIER)` its own answer; otherwise they are events.
    qresync: bool,
    flags: Vec<Flag>,
    permanent_flags: Option<(Vec<Flag>, NewKeywords)>,
    /// `None` until the server sends EXISTS (required, RFC 3501 §6.3.1).
    exists: Option<u32>,
    uid_validity: Option<UidValidity>,
    uid_next: Option<Uid>,
    mod_seq: Option<ModSeqState>,
    access: Option<Access>,
    vanished: Vec<UidSet>,
    changed: Vec<FetchData>,
}

impl Collected {
    /// Takes what belongs to the SELECT; gives back the rest.
    fn take(&mut self, data: Untagged) -> Option<Untagged> {
        match data {
            Untagged::Flags(flags) => self.flags = flags,
            Untagged::Exists(count) => self.exists = Some(count),
            Untagged::Recent(_) => {}
            Untagged::Vanished(vanished)
                if self.qresync && vanished.kind == VanishedKind::Earlier =>
            {
                self.vanished.push(vanished.uids);
            }
            Untagged::Fetch(fetch) if self.qresync => self.changed.push(*fetch),
            Untagged::Status(status) => match status.code {
                // QRESYNC: what came before belonged to the old mailbox.
                Some(ResponseCode::Closed) => {
                    *self = Self {
                        qresync: self.qresync,
                        ..Self::default()
                    };
                }
                Some(code) if is_select_code(&code) => self.take_code(code),
                code => {
                    return Some(Untagged::Status(
                        nebula_imap_proto::response::StatusResponse { code, ..status },
                    ));
                }
            },
            other => return Some(other),
        }
        None
    }

    fn take_code(&mut self, code: ResponseCode) {
        match code {
            ResponseCode::UidValidity(value) => self.uid_validity = Some(value),
            ResponseCode::UidNext(value) => self.uid_next = Some(value),
            ResponseCode::HighestModSeq(value) => self.mod_seq = Some(ModSeqState::Highest(value)),
            ResponseCode::NoModSeq => self.mod_seq = Some(ModSeqState::NoModSeq),
            ResponseCode::PermanentFlags {
                flags,
                new_keywords,
            } => {
                self.permanent_flags = Some((flags, new_keywords));
            }
            ResponseCode::ReadOnly => self.access = Some(Access::ReadOnly),
            ResponseCode::ReadWrite => self.access = Some(Access::ReadWrite),
            _ => {}
        }
    }
}

/// Codes that describe the mailbox being selected.
fn is_select_code(code: &ResponseCode) -> bool {
    matches!(
        code,
        ResponseCode::UidValidity(_)
            | ResponseCode::UidNext(_)
            | ResponseCode::HighestModSeq(_)
            | ResponseCode::NoModSeq
            | ResponseCode::PermanentFlags { .. }
            | ResponseCode::ReadOnly
            | ResponseCode::ReadWrite
            | ResponseCode::Unseen(_)
    )
}

impl Client<Authenticated> {
    /// Selects a mailbox (SELECT, or EXAMINE for read-only).
    ///
    /// # Errors
    ///
    /// On NO (for example, the mailbox does not exist) the client comes back
    /// in `Authenticated` state inside the error.
    pub async fn select(
        self,
        mailbox: &MailboxName,
        options: SelectOptions,
    ) -> Result<(Client<Selected>, SelectResult), StateError<Client<Authenticated>>> {
        select_mailbox(self, mailbox, options).await
    }
}

impl Client<Selected> {
    /// Selects another mailbox. With QRESYNC enabled the server sends
    /// `[CLOSED]` for the current one.
    ///
    /// A QRESYNC SELECT (with a known state) first leaves the current
    /// mailbox with UNSELECT, so the answer has no `[CLOSED]` at all.
    /// RFC 7162 §3.2.11 puts `[CLOSED]` before everything about the new
    /// mailbox, and the collector drops what came before it (it belonged to
    /// the old mailbox). Stalwart 0.16 sends the `VANISHED (EARLIER)` and
    /// changed `FETCH` of the new mailbox *before* `[CLOSED]`, so they were
    /// dropped and the caller saved the new HIGHESTMODSEQ without them:
    /// flag changes and expunges lost for good. Without
    /// UNSELECT (and read-write), it selects directly as before.
    ///
    /// # Errors
    ///
    /// On NO the previous mailbox is no longer selected (RFC 3501 §6.3.1),
    /// so the client comes back in `Authenticated` state.
    pub async fn select(
        self,
        mailbox: &MailboxName,
        options: SelectOptions,
    ) -> Result<(Client<Selected>, SelectResult), StateError<Client<Authenticated>>> {
        let (sync, _) = choose_sync(&self, options.resync.clone());
        if !matches!(sync, SelectSync::QResync(_)) {
            return select_mailbox(self, mailbox, options).await;
        }
        match self.unselect().await {
            Ok(client) => select_mailbox(client, mailbox, options).await,
            Err(error) => match error.into_parts() {
                (_, Some(client)) => select_mailbox(client, mailbox, options).await,
                (error, None) => Err(StateError::fatal(error)),
            },
        }
    }
}

fn choose_sync<S>(client: &Client<S>, resync: Resync) -> (SelectSync, Option<SyncLevel>) {
    let session = &client.session;
    let condstore = session.sync >= SyncLevel::CondStore || session.has(&Capability::CondStore);
    match resync {
        Resync::QResync(params) if session.sync == SyncLevel::QResync => {
            (SelectSync::QResync(params), Some(SyncLevel::QResync))
        }
        Resync::QResync(_) | Resync::Auto if condstore => {
            (SelectSync::CondStore, Some(SyncLevel::CondStore))
        }
        Resync::QResync(_) | Resync::Auto | Resync::Plain => (SelectSync::Plain, None),
    }
}

async fn select_mailbox<S: State>(
    mut client: Client<S>,
    mailbox: &MailboxName,
    options: SelectOptions,
) -> Result<(Client<Selected>, SelectResult), StateError<Client<Authenticated>>> {
    let (sync, used) = choose_sync(&client, options.resync);
    let command = Select {
        mailbox,
        access: options.access,
        sync,
    };
    let mut collected = Collected {
        qresync: used == Some(SyncLevel::QResync),
        ..Collected::default()
    };
    let result = client
        .run_with(&command, StrayContinuation::Reject, |data| {
            collected.take(data)
        })
        .await;
    let tagged = match result {
        Ok(tagged) => tagged,
        Err(error) => return Err(StateError::fatal(error)),
    };
    let tagged = match expect_ok(command_name(options.access), tagged) {
        Ok(tagged) => tagged,
        Err(error) => return Err(StateError::new(error, client.into_state(Authenticated))),
    };
    if let Some(code) = tagged.code {
        collected.take_code(code);
    }
    let Some(uid_validity) = collected.uid_validity else {
        return Err(StateError::fatal(Error::MissingUidValidity));
    };
    // Without EXISTS we would report an empty mailbox, and a sync would
    // take every stored message as expunged.
    let Some(exists) = collected.exists else {
        return Err(StateError::fatal(Error::Protocol("SELECT without EXISTS")));
    };
    let state = MailboxState {
        name: mailbox.clone(),
        access: collected.access.unwrap_or(options.access),
        uid_validity,
        uid_next: collected.uid_next,
        exists,
        mod_seq: collected.mod_seq.unwrap_or(ModSeqState::Unknown),
        flags: collected.flags,
        permanent_flags: match collected.permanent_flags {
            Some((flags, new_keywords)) => PermanentFlags::Listed {
                flags,
                new_keywords,
            },
            None => PermanentFlags::NotAnnounced,
        },
    };
    let result = SelectResult {
        used,
        vanished: collected.vanished,
        changed: collected.changed,
    };
    Ok((client.into_state(Selected { mailbox: state }), result))
}

fn command_name(access: Access) -> &'static str {
    match access {
        Access::ReadOnly => "EXAMINE",
        _ => "SELECT",
    }
}
