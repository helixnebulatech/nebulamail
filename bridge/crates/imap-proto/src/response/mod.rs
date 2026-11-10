// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Server responses: framing with limits, parsing with `imap-proto`, and an
//! owned, validated model.
//!
//! ```
//! use nebula_imap_proto::response::{FrameLimits, Response, ResponseDecoder, Untagged};
//!
//! let mut decoder = ResponseDecoder::new(FrameLimits::default());
//! decoder.feed(b"* 3 EXISTS\r\nA1 OK done\r\n");
//! assert!(matches!(decoder.decode(), Ok(Some(Response::Untagged(Untagged::Exists(3))))));
//! assert!(matches!(decoder.decode(), Ok(Some(Response::Tagged(_)))));
//! assert!(matches!(decoder.decode(), Ok(None)));
//! ```

mod body;
mod code;
mod convert;
mod fetch;
mod framing;
mod tokens;

pub use body::{BodyStructure, Disposition, Multipart, PartCommon, SinglePart, TransferEncoding};
pub use code::{NewKeywords, ResponseCode};
pub use fetch::{BinarySection, BodySection, FetchData, ObjectId, Preview, SectionKind, ThreadId};
pub use framing::FrameLimits;

use crate::DecodeError;
use crate::types::{
    Capabilities, Capability, Flag, MailboxAttribute, MailboxName, ModSeq, NameEncoding, SeqNum,
    SeqSet, SpecialUse, Tag, Uid, UidSet, UidValidity,
};

/// A value in a response broke the rules of its type (internal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Invalid(pub(crate) &'static str);

/// One server response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Response {
    /// `+ ...`: the server waits for more data.
    Continuation(Continuation),
    /// `A1 OK ...`: a command finished.
    Tagged(Tagged),
    /// `* ...`: data, or a status without tag.
    Untagged(Untagged),
}

/// `+ text`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continuation {
    /// A response code, rare here.
    pub code: Option<ResponseCode>,
    /// The text, or a base64 SASL challenge.
    pub text: String,
}

/// How a command finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Completion {
    /// `OK`
    Ok,
    /// `NO`: the command failed.
    No,
    /// `BAD`: the server did not accept the command.
    Bad,
}

/// A tagged completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tagged {
    /// The command's tag.
    pub tag: Tag,
    /// OK, NO or BAD.
    pub status: Completion,
    /// Response code.
    pub code: Option<ResponseCode>,
    /// Human-readable text (characters outside ASCII become `?`).
    pub text: String,
}

/// The condition of an untagged status response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Condition {
    /// `OK`
    Ok,
    /// `NO`: a warning.
    No,
    /// `BAD`: a protocol error.
    Bad,
    /// `PREAUTH`: greeting of an already authenticated connection.
    PreAuth,
    /// `BYE`: the server closes the connection.
    Bye,
}

/// `* OK|NO|BAD|PREAUTH|BYE [code] text`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResponse {
    /// The condition.
    pub condition: Condition,
    /// Response code.
    pub code: Option<ResponseCode>,
    /// Human-readable text (characters outside ASCII become `?`).
    pub text: String,
}

/// Whether VANISHED is the answer to a resync (`EARLIER`) or news.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VanishedKind {
    /// `VANISHED (EARLIER)`: expunged before this command (QRESYNC resync).
    Earlier,
    /// `VANISHED`: just expunged (instead of EXPUNGE, with QRESYNC).
    Now,
}

/// `* VANISHED [(EARLIER)] uids`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vanished {
    /// Earlier or now.
    pub kind: VanishedKind,
    /// The expunged UIDs.
    pub uids: UidSet,
}

/// `* LIST (attributes) delimiter name`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    /// Attributes (`\Noselect`, `\HasChildren`, `\Sent`…).
    pub attributes: Vec<MailboxAttribute>,
    /// Hierarchy delimiter (`None` for a flat namespace).
    pub delimiter: Option<char>,
    /// The mailbox.
    pub name: MailboxName,
}

impl ListEntry {
    /// The special use, if the server marked one.
    #[must_use]
    pub fn special_use(&self) -> Option<SpecialUse> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                MailboxAttribute::SpecialUse(special_use) => Some(*special_use),
                _ => None,
            })
    }

    /// Whether the mailbox can be selected.
    #[must_use]
    pub fn is_selectable(&self) -> bool {
        !self.attributes.iter().any(|a| {
            matches!(
                a,
                MailboxAttribute::NoSelect | MailboxAttribute::NonExistent
            )
        })
    }
}

/// `* STATUS name (...)`
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MailboxStatus {
    /// The mailbox.
    pub mailbox: MailboxName,
    /// `MESSAGES`
    pub messages: Option<u32>,
    /// `RECENT`
    pub recent: Option<u32>,
    /// `UIDNEXT`
    pub uid_next: Option<Uid>,
    /// `UIDVALIDITY`
    pub uid_validity: Option<UidValidity>,
    /// `UNSEEN`
    pub unseen: Option<u32>,
    /// `HIGHESTMODSEQ`
    pub highest_mod_seq: Option<ModSeq>,
}

/// `* SEARCH n... [(MODSEQ m)]`. With UID SEARCH, the numbers are UIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    /// The matching numbers (UIDs for UID SEARCH).
    pub numbers: Vec<std::num::NonZeroU32>,
    /// Highest mod-sequence of the matches (CONDSTORE `MODSEQ` search).
    pub highest_mod_seq: Option<ModSeq>,
}

impl SearchResult {
    /// The numbers as UIDs (for UID SEARCH).
    #[must_use]
    pub fn uids(&self) -> Vec<Uid> {
        self.numbers.iter().copied().map(Uid::from).collect()
    }
}

/// The `ALL` of an ESEARCH answer: UIDs for `UID SEARCH`, else sequence
/// numbers. Kept as a compact set: a range like `1:4000000000` is not
/// expanded until the caller asks (with its own limit).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ESearchAll {
    /// After `UID`.
    Uids(UidSet),
    /// Without `UID`.
    SeqNums(SeqSet),
}

/// `* ESEARCH [(TAG "A1")] [UID] [MIN n] [MAX n] [COUNT n] [ALL set]
/// [MODSEQ n]` (RFC 4731, RFC 9051). Unknown return data is skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ESearchResult {
    /// The tag of the command it answers, if given.
    pub tag: Option<Tag>,
    /// Whether the numbers are UIDs.
    pub uid: bool,
    /// `MIN`
    pub min: Option<std::num::NonZeroU32>,
    /// `MAX`
    pub max: Option<std::num::NonZeroU32>,
    /// `COUNT`
    pub count: Option<u32>,
    /// `ALL` (absent when nothing matched).
    pub all: Option<ESearchAll>,
    /// `MODSEQ`: highest mod-sequence of the matches (CONDSTORE).
    pub mod_seq: Option<ModSeq>,
}

/// `* QUOTAROOT mailbox root...`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaRoot {
    /// The mailbox asked about.
    pub mailbox: MailboxName,
    /// Its quota roots.
    pub roots: Vec<String>,
}

/// A quota resource name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum QuotaResourceName {
    /// `STORAGE`: in units of 1024 bytes.
    Storage,
    /// `MESSAGE`: number of messages.
    Message,
    /// Any other, upper-cased.
    Other(String),
}

/// One resource of a QUOTA response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaResource {
    /// Which resource.
    pub name: QuotaResourceName,
    /// Current usage.
    pub usage: u64,
    /// Limit.
    pub limit: u64,
}

/// `* QUOTA root (resource usage limit ...)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quota {
    /// The quota root.
    pub root: String,
    /// Its resources.
    pub resources: Vec<QuotaResource>,
}

/// One namespace of a NAMESPACE response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceEntry {
    /// Prefix such as `INBOX.` or `""` (decoded like a mailbox name).
    pub prefix: String,
    /// Hierarchy delimiter.
    pub delimiter: Option<char>,
}

/// `* NAMESPACE personal other shared`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Namespaces {
    /// The user's own mailboxes.
    pub personal: Vec<NamespaceEntry>,
    /// Other users' mailboxes.
    pub other_users: Vec<NamespaceEntry>,
    /// Shared mailboxes.
    pub shared: Vec<NamespaceEntry>,
}

/// Why a response was not understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnknownReason {
    /// The syntax is not valid (or not supported by the parser).
    Unparsed,
    /// Valid, but data we do not use (METADATA, ACL…).
    Unsupported,
    /// A value broke the rules of its type (for example UID 0).
    InvalidValue(&'static str),
    /// Nested deeper than the limit.
    TooDeep,
    /// Larger than a limit of [`FrameLimits`] (line, literal, number of
    /// literals or whole response): dropped without being buffered, and
    /// the connection goes on with the next response.
    TooLarge,
}

/// What kind of response an [`Unknown`] seemed to be, from its first words.
/// A client waiting for search results or FETCH data can tell "a result I
/// could not read" from unrelated noise (METADATA, vendor responses…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UnknownKind {
    /// `* SEARCH ...`
    Search,
    /// `* ESEARCH ...`
    ESearch,
    /// `* n FETCH ...`
    Fetch,
    /// Anything else.
    Other,
}

impl UnknownKind {
    /// From the second and third words of the response, upper-cased.
    pub(crate) fn from_words(word: &str, third: &str) -> Self {
        match word {
            "SEARCH" => Self::Search,
            "ESEARCH" => Self::ESearch,
            _ if third == "FETCH" => Self::Fetch,
            _ => Self::Other,
        }
    }
}

/// An untagged response we could not use. Only its size is kept (never
/// the content, which may be private), so it can be logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unknown {
    /// Size in bytes of the response.
    pub len: usize,
    /// Why.
    pub reason: UnknownReason,
    /// What it seemed to be.
    pub about: UnknownKind,
}

/// An untagged response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Untagged {
    /// `* OK|NO|BAD|PREAUTH|BYE ...`
    Status(StatusResponse),
    /// `* CAPABILITY ...`
    Capabilities(Capabilities),
    /// `* ENABLED ...`
    Enabled(Vec<Capability>),
    /// `* n EXISTS`
    Exists(u32),
    /// `* n RECENT`
    Recent(u32),
    /// `* n EXPUNGE`
    Expunge(SeqNum),
    /// `* VANISHED ...`
    Vanished(Vanished),
    /// `* n FETCH (...)`
    Fetch(Box<FetchData>),
    /// `* FLAGS (...)`: flags defined in the mailbox.
    Flags(Vec<Flag>),
    /// `* LIST ...`
    List(ListEntry),
    /// `* STATUS ...`
    MailboxStatus(MailboxStatus),
    /// `* SEARCH ...`
    Search(SearchResult),
    /// `* ESEARCH ...`
    ESearch(ESearchResult),
    /// `* QUOTAROOT ...`
    QuotaRoot(QuotaRoot),
    /// `* QUOTA ...`
    Quota(Quota),
    /// `* ID ...`: the server's identification (`None` for `NIL`).
    Id(Option<Vec<(String, String)>>),
    /// `* NAMESPACE ...`
    Namespace(Namespaces),
    /// Not understood; the connection goes on.
    Unknown(Unknown),
}

/// Decodes responses from the bytes read from the connection.
#[derive(Debug)]
pub struct ResponseDecoder {
    buffer: Vec<u8>,
    scanner: framing::FrameScanner,
    limits: FrameLimits,
    names: NameEncoding,
}

impl ResponseDecoder {
    /// A decoder with these limits; mailbox names in modified UTF-7.
    #[must_use]
    pub fn new(limits: FrameLimits) -> Self {
        Self {
            buffer: Vec::new(),
            scanner: framing::FrameScanner::default(),
            limits,
            names: NameEncoding::ModifiedUtf7,
        }
    }

    /// Mailbox names are UTF-8 from now on (after `ENABLE UTF8=ACCEPT`).
    pub fn set_name_encoding(&mut self, names: NameEncoding) {
        self.names = names;
    }

    /// The current mailbox name encoding.
    #[must_use]
    pub fn name_encoding(&self) -> NameEncoding {
        self.names
    }

    /// Adds bytes read from the connection.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes received but not yet decoded. Must be zero right after the
    /// STARTTLS response, or a man in the middle injected data.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Drops all buffered bytes.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.scanner = framing::FrameScanner::default();
    }

    /// The next complete response, or `None` if more bytes are needed.
    ///
    /// A response that breaks a limit is dropped as it arrives (the buffer
    /// stays within the limits) and comes out as
    /// [`UnknownReason::TooLarge`]; a tagged one keeps its tag and status,
    /// read from its first KiB, so the command still completes.
    ///
    /// # Errors
    ///
    /// None at the moment: the limits no longer end the connection. The
    /// `Result` stays so that a future fatal case does not change the API.
    pub fn decode(&mut self) -> Result<Option<Response>, DecodeError> {
        match self.scanner.scan(&self.buffer, &self.limits) {
            framing::Scan::NeedMore { drop } => {
                self.buffer.drain(..drop.min(self.buffer.len()));
                Ok(None)
            }
            framing::Scan::Frame(frame) => {
                let context = convert::Context::new(self.names, &self.limits);
                let bytes = self.buffer.get(..frame.len).unwrap_or_default();
                let response = convert::parse_frame(bytes, &frame.literals, &context);
                self.buffer.drain(..frame.len.min(self.buffer.len()));
                Ok(Some(response))
            }
            framing::Scan::Skipped(skipped) => {
                self.buffer.drain(..skipped.consumed.min(self.buffer.len()));
                Ok(Some(convert::parse_skipped(&skipped.prefix, skipped.len)))
            }
        }
    }
}

/// Parses one complete response (for fuzzing and tests): `frame` must be
/// exactly one response, CRLF included.
#[must_use]
pub fn parse_response(frame: &[u8], names: NameEncoding, limits: &FrameLimits) -> Option<Response> {
    let mut scanner = framing::FrameScanner::default();
    match scanner.scan(frame, limits) {
        framing::Scan::Frame(end) if end.len == frame.len() => {
            let context = convert::Context::new(names, limits);
            Some(convert::parse_frame(frame, &end.literals, &context))
        }
        framing::Scan::Skipped(skipped) if skipped.consumed == frame.len() => {
            Some(convert::parse_skipped(&skipped.prefix, skipped.len))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
