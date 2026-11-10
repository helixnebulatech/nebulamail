// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The typestate client: `NotAuthenticated` → `Authenticated` → `Selected`.

use std::sync::Arc;

use nebula_imap_proto::command::{Access, Command, EncodeOptions};
use nebula_imap_proto::response::{
    Completion, Condition, NewKeywords, ResponseCode, Tagged, Unknown, UnknownReason, Untagged,
    VanishedKind,
};
use nebula_imap_proto::types::{
    Capabilities, Capability, Flag, MailboxName, ModSeq, NameEncoding, Uid, UidValidity,
};

use super::alert::{AlertSink, Alerts};
use super::connection::{Connection, SecureStream, StrayContinuation, expect_ok};
use crate::Error;

/// State: connected over TLS, not logged in.
#[derive(Debug)]
pub struct NotAuthenticated;

/// State: logged in, no mailbox selected.
#[derive(Debug)]
pub struct Authenticated;

/// State: a mailbox is selected.
#[derive(Debug)]
pub struct Selected {
    pub(crate) mailbox: MailboxState,
}

/// What we know about the mailbox's mod-sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ModSeqState {
    /// HIGHESTMODSEQ, kept up to date with the changes we see.
    Highest(ModSeq),
    /// `NOMODSEQ`: this mailbox has no mod-sequences.
    NoModSeq,
    /// CONDSTORE was not used for this SELECT.
    Unknown,
}

/// The selected mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MailboxState {
    /// Its name.
    pub name: MailboxName,
    /// Read-write or read-only (the server may force read-only).
    pub access: Access,
    /// UIDVALIDITY: if it differs from the stored one, resync everything.
    pub uid_validity: UidValidity,
    /// UIDNEXT, if the server sent it.
    pub uid_next: Option<Uid>,
    /// Number of messages, updated by EXISTS/EXPUNGE/VANISHED.
    pub exists: u32,
    /// HIGHESTMODSEQ or NOMODSEQ.
    pub mod_seq: ModSeqState,
    /// Flags defined in the mailbox (`FLAGS`).
    pub flags: Vec<Flag>,
    /// Flags that can be stored (`PERMANENTFLAGS`).
    pub permanent_flags: PermanentFlags,
}

/// The `PERMANENTFLAGS` of a SELECT. `()` (nothing can be stored) and no
/// PERMANENTFLAGS at all (anything can, RFC 3501 §7.1) are different.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermanentFlags {
    /// The server did not send PERMANENTFLAGS: every flag can be stored.
    NotAnnounced,
    /// The server listed what can be stored.
    Listed {
        /// The flags (maybe none).
        flags: Vec<Flag>,
        /// Whether new keywords can be created (`\*`).
        new_keywords: NewKeywords,
    },
}

impl PermanentFlags {
    /// Whether `flag` can be stored in the mailbox. Keywords compare
    /// without case ([`Flag::matches`]).
    #[must_use]
    pub fn can_store(&self, flag: &Flag) -> bool {
        match self {
            Self::NotAnnounced => true,
            Self::Listed {
                flags,
                new_keywords,
            } => {
                flags.iter().any(|listed| listed.matches(flag))
                    || (matches!(flag, Flag::Keyword(_)) && *new_keywords == NewKeywords::Allowed)
            }
        }
    }
}

impl MailboxState {
    /// Updates counters from an unsolicited response.
    pub(crate) fn apply(&mut self, data: &Untagged) {
        match data {
            Untagged::Exists(count) => self.exists = *count,
            Untagged::Expunge(_) => self.exists = self.exists.saturating_sub(1),
            Untagged::Vanished(vanished) if vanished.kind == VanishedKind::Now => {
                let count = vanished.uids.count().unwrap_or(0);
                self.exists = self
                    .exists
                    .saturating_sub(u32::try_from(count).unwrap_or(u32::MAX));
            }
            Untagged::Fetch(fetch) => {
                if let Some(mod_seq) = fetch.mod_seq {
                    self.raise_mod_seq(mod_seq);
                }
            }
            Untagged::Status(status) => match &status.code {
                Some(ResponseCode::HighestModSeq(mod_seq)) => self.raise_mod_seq(*mod_seq),
                Some(ResponseCode::UidNext(uid)) => self.uid_next = Some(*uid),
                _ => {}
            },
            _ => {}
        }
    }

    fn raise_mod_seq(&mut self, mod_seq: ModSeq) {
        match self.mod_seq {
            ModSeqState::Highest(current) if current >= mod_seq => {}
            ModSeqState::Highest(_) | ModSeqState::Unknown => {
                self.mod_seq = ModSeqState::Highest(mod_seq);
            }
            ModSeqState::NoModSeq => {}
        }
    }
}

/// Which synchronisation extension is active (RFC 7162).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SyncLevel {
    /// Neither: compare UIDs and flags.
    None,
    /// `CONDSTORE` enabled: MODSEQ and CHANGEDSINCE.
    CondStore,
    /// `QRESYNC` enabled: also VANISHED and SELECT ... QRESYNC.
    QResync,
}

/// How the server gives stable message ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ObjectIdSupport {
    /// No server ids: match messages by Message-ID or content.
    None,
    /// RFC 8474: fetch `EMAILID` and `THREADID` (Dovecot…).
    Rfc8474,
    /// `OBJECTID+` draft, enabled: fetch `OBJECTID` (Stalwart).
    ObjectIdPlus,
}

/// Data of the connection shared by all states.
#[derive(Debug)]
pub(crate) struct Session {
    pub(crate) capabilities: Capabilities,
    pub(crate) sync: SyncLevel,
    pub(crate) names: NameEncoding,
    /// `ENABLE UTF8=ACCEPT` succeeded (RFC 6855 §3).
    pub(crate) utf8_accept: bool,
    pub(crate) object_ids: ObjectIdSupport,
    pub(crate) events: Vec<Untagged>,
    /// More than `max_events` arrived since the last `take_events`: the
    /// kept ones were dropped (see [`Events::overflowed`]).
    pub(crate) events_overflowed: bool,
    pub(crate) max_events: usize,
    /// Where `[ALERT]` texts go (RFC 3501 §7.1, RFC 9051 §7.1).
    pub(crate) alerts: Alerts,
}

/// The unsolicited responses received since the last
/// [`Client::take_events`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Events {
    /// EXISTS, EXPUNGE, VANISHED, FETCH with new flags, alerts…, oldest
    /// first. Empty if [`Self::overflowed`].
    pub items: Vec<Untagged>,
    /// More than [`Limits::max_events`](crate::Limits) arrived (a big
    /// UID MOVE without QRESYNC, a flood from the server): they were
    /// dropped to bound memory, so the caller must resynchronise
    /// everything instead of looking at `items`.
    pub overflowed: bool,
}

impl Session {
    pub(crate) fn new(capabilities: Capabilities, max_events: usize) -> Self {
        Self {
            capabilities,
            sync: SyncLevel::None,
            names: NameEncoding::ModifiedUtf7,
            utf8_accept: false,
            object_ids: ObjectIdSupport::None,
            events: Vec::new(),
            events_overflowed: false,
            max_events,
            alerts: Alerts::default(),
        }
    }

    /// Sends the text of an `[ALERT]` status response to the sink: the
    /// user must see it (RFC 3501 §7.1, RFC 9051 §7.1). Called for every
    /// untagged response, before any command handler can take it.
    pub(crate) fn observe(&mut self, data: &Untagged) {
        if let Untagged::Status(status) = data
            && status.code == Some(ResponseCode::Alert)
        {
            self.alerts.push(&status.text);
        }
    }

    /// As [`Self::observe`], for a tagged response.
    pub(crate) fn observe_tagged(&mut self, tagged: &Tagged) {
        if tagged.code == Some(ResponseCode::Alert) {
            self.alerts.push(&tagged.text);
        }
    }

    /// Keeps one event, unless the buffer is full: then everything kept is
    /// dropped and nothing more is kept until the caller takes them.
    fn push_event(&mut self, data: Untagged) {
        if self.events_overflowed {
            return;
        }
        if self.events.len() >= self.max_events {
            tracing::info!(
                max = self.max_events,
                "too many unsolicited IMAP responses: dropped, full sync needed"
            );
            self.events = Vec::new();
            self.events_overflowed = true;
            return;
        }
        self.events.push(data);
    }

    pub(crate) fn take_events(&mut self) -> Events {
        Events {
            items: std::mem::take(&mut self.events),
            overflowed: std::mem::take(&mut self.events_overflowed),
        }
    }

    pub(crate) fn has(&self, capability: &Capability) -> bool {
        self.capabilities.has(capability)
    }

    pub(crate) fn encode_options(&self) -> EncodeOptions {
        EncodeOptions::for_capabilities(&self.capabilities, self.names)
    }

    /// Keeps an unsolicited response for the caller. Unknown responses are
    /// only logged (by size), and new capabilities are applied at once.
    pub(crate) fn record(&mut self, data: Untagged) {
        match data {
            Untagged::Unknown(unknown) => {
                tracing::debug!(
                    len = unknown.len,
                    reason = ?unknown.reason,
                    about = ?unknown.about,
                    "ignored IMAP response"
                );
            }
            Untagged::Capabilities(capabilities) => self.capabilities = capabilities,
            Untagged::Status(ref status) => {
                if let Some(ResponseCode::Capabilities(capabilities)) = &status.code {
                    self.capabilities = capabilities.clone();
                }
                if status.condition == Condition::Bye {
                    tracing::info!(condition = ?status.condition, "IMAP server notice");
                }
                self.push_event(data);
            }
            other => self.push_event(other),
        }
    }
}

/// An IMAP connection in state `S`.
pub struct Client<S> {
    pub(crate) connection: Box<Connection<SecureStream>>,
    pub(crate) session: Session,
    pub(crate) state: S,
}

impl<S: std::fmt::Debug> std::fmt::Debug for Client<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("state", &self.state)
            .field("sync", &self.session.sync)
            .field("broken", &self.connection.is_broken())
            .finish_non_exhaustive()
    }
}

/// The result of connecting: usually not authenticated; `PREAUTH` greetings
/// (accepted only over TLS) skip the login.
#[derive(Debug)]
#[non_exhaustive]
pub enum Connected {
    /// The normal case: log in next.
    NotAuthenticated(Client<NotAuthenticated>),
    /// The server greeted with PREAUTH.
    PreAuthenticated(Client<Authenticated>),
}

impl<S> Client<S> {
    /// The server's current capabilities.
    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.session.capabilities
    }

    /// The active synchronisation extension.
    #[must_use]
    pub fn sync_level(&self) -> SyncLevel {
        self.session.sync
    }

    /// How mailbox names are encoded (UTF-8 after `ENABLE UTF8=ACCEPT` or
    /// with `IMAP4rev2`, RFC 9051 §5.1).
    #[must_use]
    pub fn name_encoding(&self) -> NameEncoding {
        self.session.names
    }

    /// Which FETCH items give stable ids: `EMAILID`/`THREADID` (RFC 8474)
    /// or `OBJECTID` (`OBJECTID+`).
    #[must_use]
    pub fn object_id_support(&self) -> ObjectIdSupport {
        self.session.object_ids
    }

    /// Sends the `[ALERT]`s of this connection to `sink` from now on, and
    /// those received before (the greeting's) right away. The caller must
    /// show them to the user (RFC 3501 §7.1, RFC 9051 §7.1).
    pub fn set_alert_sink(&mut self, sink: Arc<dyn AlertSink>) {
        self.session.alerts.set_sink(sink);
    }

    /// Takes the unsolicited responses received so far (EXISTS, EXPUNGE,
    /// VANISHED, FETCH with new flags, alerts…), oldest first. If more
    /// than [`Limits::max_events`](crate::Limits) arrived, they are gone
    /// and [`Events::overflowed`] says so.
    pub fn take_events(&mut self) -> Events {
        self.session.take_events()
    }

    /// Whether an earlier fatal error broke the connection.
    #[must_use]
    pub fn is_broken(&self) -> bool {
        self.connection.is_broken()
    }

    pub(crate) fn into_state<T>(self, state: T) -> Client<T> {
        Client {
            connection: self.connection,
            session: self.session,
            state,
        }
    }
}

impl Client<Selected> {
    /// The selected mailbox.
    #[must_use]
    pub fn mailbox(&self) -> &MailboxState {
        &self.state.mailbox
    }
}

pub(crate) mod sealed {
    use super::MailboxState;

    /// Implemented by the three states only.
    pub trait State: Send {
        /// The selected mailbox, if any (to keep its counters up to date).
        fn selected_mut(&mut self) -> Option<&mut MailboxState>;
    }

    impl State for super::NotAuthenticated {
        fn selected_mut(&mut self) -> Option<&mut MailboxState> {
            None
        }
    }

    impl State for super::Authenticated {
        fn selected_mut(&mut self) -> Option<&mut MailboxState> {
            None
        }
    }

    impl State for super::Selected {
        fn selected_mut(&mut self) -> Option<&mut MailboxState> {
            Some(&mut self.mailbox)
        }
    }
}

/// The logged-in states (`Authenticated` and `Selected`): mailbox commands
/// such as LIST, STATUS or APPEND work in both.
pub trait LoggedIn: sealed::State {}

impl LoggedIn for Authenticated {}
impl LoggedIn for Selected {}

impl<S: sealed::State> Client<S> {
    /// Runs a command. Untagged responses go to `handle` first; what it
    /// gives back is kept as an event. The selected mailbox sees everything.
    ///
    /// # Errors
    ///
    /// [`Error::ResponseTooLarge`] if the command ended with OK but some
    /// response during it was dropped for its size (its data would be
    /// missing: a SEARCH without some UIDs reads as expunged messages).
    pub(crate) async fn run_with(
        &mut self,
        command: &impl Command,
        stray: StrayContinuation,
        handle: impl FnMut(Untagged) -> Option<Untagged> + Send,
    ) -> Result<Tagged, Error> {
        let (tagged, skipped) = self.run_counting_skipped(command, stray, handle).await?;
        if skipped > 0 && tagged.status == Completion::Ok {
            tracing::warn!(
                command = command.name(),
                skipped,
                "IMAP response over the limits"
            );
            return Err(Error::ResponseTooLarge(command.name()));
        }
        Ok(tagged)
    }

    /// As [`Self::run_with`], but tolerates dropped responses: it gives
    /// back how many there were. Only for commands that can do without
    /// some of their data (UID FETCH, which asks again for what is missing).
    pub(crate) async fn run_counting_skipped(
        &mut self,
        command: &impl Command,
        stray: StrayContinuation,
        mut handle: impl FnMut(Untagged) -> Option<Untagged> + Send,
    ) -> Result<(Tagged, usize), Error> {
        let options = self.session.encode_options();
        let Self {
            connection,
            session,
            state,
        } = self;
        let mut skipped = 0_usize;
        let mut on_untagged = |data: Untagged| {
            // Counted before `handle`, which may swallow it, and whether
            // the command asked for it or not.
            if let Untagged::Unknown(Unknown {
                reason: UnknownReason::TooLarge,
                ..
            }) = data
            {
                skipped = skipped.saturating_add(1);
            }
            if let Some(mailbox) = state.selected_mut() {
                mailbox.apply(&data);
            }
            session.observe(&data);
            if let Some(rest) = handle(data) {
                session.record(rest);
            }
        };
        let tagged = connection
            .run(command, options, stray, &mut on_untagged)
            .await?;
        session.observe_tagged(&tagged);
        Ok((tagged, skipped))
    }

    /// Runs a command whose untagged responses are all events, and checks
    /// that it ended with OK.
    pub(crate) async fn run_simple(&mut self, command: &impl Command) -> Result<Tagged, Error> {
        let tagged = self
            .run_with(command, StrayContinuation::Reject, Some)
            .await?;
        expect_ok(command.name(), tagged)
    }
}
