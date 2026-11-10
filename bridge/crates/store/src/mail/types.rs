// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Public types of the mail data: what the sync writes and what the JMAP
//! layer reads.

use std::collections::BTreeSet;

use nebula_imap_proto::types::{MailboxName, ModSeq, Uid, UidValidity};
use nebula_jmap_proto::Keyword;
use nebula_jmap_proto::object::email::EmailAddress;

use crate::{DataType, EmailId, MailboxId, Seq, StoreError, ThreadId, UnixTime};

/// What a mailbox is for (RFC 8621 `role`, from SPECIAL-USE or the name).
/// The number is what the database stores: never reuse one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Role {
    /// The inbox.
    Inbox,
    /// Archive.
    Archive,
    /// Drafts.
    Drafts,
    /// Sent messages.
    Sent,
    /// Trash.
    Trash,
    /// Spam.
    Junk,
    /// Every message (Gmail "All Mail").
    All,
    /// Flagged messages (virtual folder).
    Flagged,
    /// Important messages (RFC 8457).
    Important,
}

impl Role {
    pub(crate) fn code(self) -> i32 {
        match self {
            Self::Inbox => 1,
            Self::Archive => 2,
            Self::Drafts => 3,
            Self::Sent => 4,
            Self::Trash => 5,
            Self::Junk => 6,
            Self::All => 7,
            Self::Flagged => 8,
            Self::Important => 9,
        }
    }

    pub(crate) fn from_code(code: i32) -> Result<Self, StoreError> {
        Ok(match code {
            1 => Self::Inbox,
            2 => Self::Archive,
            3 => Self::Drafts,
            4 => Self::Sent,
            5 => Self::Trash,
            6 => Self::Junk,
            7 => Self::All,
            8 => Self::Flagged,
            9 => Self::Important,
            _ => return Err(StoreError::Corrupt("mailbox role")),
        })
    }
}

/// The IMAP sync state of a mailbox. Not visible over JMAP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MailboxSyncState {
    /// UIDVALIDITY of the stored UIDs.
    pub uid_validity: Option<UidValidity>,
    /// UIDNEXT at the last complete sync.
    pub uid_next: Option<Uid>,
    /// HIGHESTMODSEQ at the last complete sync (`None` until one finished,
    /// or when the server has no mod-sequences).
    pub highest_mod_seq: Option<ModSeq>,
    /// Number of messages at the last sync (to compare with STATUS).
    pub messages: Option<u32>,
    /// When the last sync finished.
    pub synced_at: Option<UnixTime>,
}

/// A mailbox (IMAP folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    /// Row id.
    pub id: MailboxId,
    /// The full IMAP name.
    pub imap_name: MailboxName,
    /// Display name: the part of the name below the parent.
    pub name: String,
    /// Hierarchy delimiter (`None` for a flat namespace).
    pub delimiter: Option<char>,
    /// Parent mailbox (`parentId`).
    pub parent: Option<MailboxId>,
    /// Role, if any (unique per account).
    pub role: Option<Role>,
    /// Whether the user is subscribed to it.
    pub subscribed: bool,
    /// `false` for `\Noselect` folders, which hold no messages.
    pub selectable: bool,
    /// `\Noinferiors` (RFC 9051 §7.3.1): no child mailbox can exist.
    pub no_inferiors: bool,
    /// Sort order (`sortOrder`).
    pub sort_order: u32,
    /// IMAP sync state.
    pub sync: MailboxSyncState,
}

/// Counts of a mailbox (RFC 8621 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MailboxCounts {
    /// Emails in the mailbox.
    pub total_emails: u64,
    /// Emails in the mailbox without `$seen` and `$draft`.
    pub unread_emails: u64,
    /// Threads with an email in the mailbox.
    pub total_threads: u64,
    /// Threads with an email in the mailbox and an unread email anywhere
    /// (emails in Trash only count for Trash, RFC 8621 §2).
    pub unread_threads: u64,
}

/// A mailbox as listed by the server, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedMailbox {
    /// Full IMAP name.
    pub imap_name: MailboxName,
    /// Display name (below the parent).
    pub name: String,
    /// Hierarchy delimiter.
    pub delimiter: Option<char>,
    /// The nearest listed ancestor, if any. It must be in the same list.
    pub parent: Option<MailboxName>,
    /// Role. If two mailboxes claim one, the first in the list keeps it.
    pub role: Option<Role>,
    /// Subscribed.
    pub subscribed: bool,
    /// Selectable (no `\Noselect`).
    pub selectable: bool,
    /// `\Noinferiors` (RFC 9051 §7.3.1): no child mailbox can exist.
    pub no_inferiors: bool,
    /// Sort order.
    pub sort_order: u32,
}

/// An object id given by the server (`EMAILID`, `OBJECTID`, `X-GM-MSGID`,
/// `THREADID`…): 1 to 255 printable ASCII characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerId(String);

impl ServerId {
    /// Validates a server id.
    ///
    /// # Errors
    /// [`StoreError::Invalid`] if it is empty, too long or not printable
    /// ASCII.
    pub fn new(text: &str) -> Result<Self, StoreError> {
        let printable = text.bytes().all(|b| b.is_ascii_graphic());
        if text.is_empty() || text.len() > 255 || !printable {
            return Err(StoreError::Invalid("server id"));
        }
        Ok(Self(text.to_owned()))
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The server's ids for a message, when it has them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerIds {
    /// Email id: copies with the same id are one email.
    pub email: Option<ServerId>,
    /// Thread id.
    pub thread: Option<ServerId>,
}

/// The `Date` header: the instant and the sender's UTC offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentAt {
    /// The instant.
    pub utc: UnixTime,
    /// The offset written in the header, in minutes (`+0200` is 120).
    pub offset_minutes: i16,
}

/// Metadata of an email, from FETCH and its header fields. Immutable once
/// stored (only the preview may be filled in later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailMeta {
    /// `Message-ID`, without angle brackets.
    pub message_id: Option<String>,
    /// `In-Reply-To` ids.
    pub in_reply_to: Vec<String>,
    /// `References` ids.
    pub references: Vec<String>,
    /// Decoded `Subject`.
    pub subject: Option<String>,
    /// `From` (`None` if the field is missing).
    pub from: Option<Vec<EmailAddress>>,
    /// `To`.
    pub to: Option<Vec<EmailAddress>>,
    /// `Cc`.
    pub cc: Option<Vec<EmailAddress>>,
    /// `Bcc`.
    pub bcc: Option<Vec<EmailAddress>>,
    /// `Reply-To`.
    pub reply_to: Option<Vec<EmailAddress>>,
    /// `Sender`.
    pub sender: Option<Vec<EmailAddress>>,
    /// `Date`.
    pub sent_at: Option<SentAt>,
    /// INTERNALDATE.
    pub received_at: UnixTime,
    /// RFC822.SIZE.
    pub size: u64,
    /// From BODYSTRUCTURE.
    pub has_attachment: bool,
    /// Up to 256 characters of body text, when known.
    pub preview: Option<String>,
    /// The fetched header fields, raw. Message content: never log it.
    pub headers: Vec<u8>,
}

/// A message to add to a mailbox (new UID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// Its UID in the mailbox.
    pub uid: Uid,
    /// Its MODSEQ, with CONDSTORE.
    pub mod_seq: Option<ModSeq>,
    /// Its flags as JMAP keywords.
    pub keywords: BTreeSet<Keyword>,
    /// Server ids, if the server has them.
    pub server: ServerIds,
    /// Metadata.
    pub meta: EmailMeta,
}

/// New flags of a known message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagUpdate {
    /// Its UID.
    pub uid: Uid,
    /// Its MODSEQ, if the server sent one (kept with the location).
    pub mod_seq: Option<ModSeq>,
    /// All its keywords.
    pub keywords: BTreeSet<Keyword>,
}

/// An email as JMAP sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    /// Row id.
    pub id: EmailId,
    /// Its thread.
    pub thread: ThreadId,
    /// Server email id, if any.
    pub server_id: Option<ServerId>,
    /// The mailboxes it is in (at least one).
    pub mailboxes: BTreeSet<MailboxId>,
    /// Keywords.
    pub keywords: BTreeSet<Keyword>,
    /// Metadata.
    pub meta: EmailMeta,
}

/// One copy of an email on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The mailbox.
    pub mailbox: MailboxId,
    /// Its IMAP name.
    pub imap_name: MailboxName,
    /// The UIDVALIDITY the UID belongs to.
    pub uid_validity: Option<UidValidity>,
    /// The UID.
    pub uid: Uid,
    /// Last known MODSEQ.
    pub mod_seq: Option<ModSeq>,
}

/// A stored copy in a mailbox, for comparing with the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredUid {
    /// The UID.
    pub uid: Uid,
    /// The email.
    pub email: EmailId,
    /// Last known MODSEQ.
    pub mod_seq: Option<ModSeq>,
}

/// The JMAP states a write moved forward. Empty if nothing visible changed.
/// Also the emails it made visible for the first time (what moved
/// `EmailDelivery`), for Web Push's `EmailPush`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recorded {
    states: Vec<(DataType, Seq)>,
    delivered: Vec<EmailId>,
}

impl Recorded {
    pub(crate) fn push(&mut self, data_type: DataType, seq: Seq) {
        self.states.push((data_type, seq));
    }

    pub(crate) fn set_delivered(&mut self, emails: Vec<EmailId>) {
        self.delivered = emails;
    }

    /// The emails that became visible (new mail), oldest row first. Empty
    /// after [`Recorded::forget`] of `EmailDelivery` or
    /// [`Recorded::not_delivered`].
    #[must_use]
    pub fn delivered(&self) -> &[EmailId] {
        &self.delivered
    }

    /// The new emails are not deliveries (the bridge wrote them itself:
    /// `Email/import`, `APPEND`, copies). `EmailDelivery` stays announced
    /// (a `StateChange`), as Stalwart does for those.
    pub fn not_delivered(&mut self) {
        self.delivered.clear();
    }

    /// Whether nothing visible changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// The data types whose state changed.
    pub fn data_types(&self) -> impl Iterator<Item = DataType> + '_ {
        self.states.iter().map(|(data_type, _)| *data_type)
    }

    /// The new state of a data type, if it changed.
    #[must_use]
    pub fn state(&self, data_type: DataType) -> Option<Seq> {
        self.states
            .iter()
            .find(|(t, _)| *t == data_type)
            .map(|(_, seq)| *seq)
    }

    /// Drops a data type from what is announced (its state stays as
    /// recorded): the first sync of a mailbox copies old mail, which is no
    /// `EmailDelivery` for push.
    pub fn forget(&mut self, data_type: DataType) {
        self.states.retain(|(t, _)| *t != data_type);
        if data_type == DataType::EmailDelivery {
            self.delivered.clear();
        }
    }

    /// Adds the states of a later write.
    pub fn merge(&mut self, later: Self) {
        for email in later.delivered {
            if !self.delivered.contains(&email) {
                self.delivered.push(email);
            }
        }
        for (data_type, seq) in later.states {
            match self.states.iter_mut().find(|(t, _)| *t == data_type) {
                Some(entry) => entry.1 = entry.1.max(seq),
                None => self.states.push((data_type, seq)),
            }
        }
    }
}
