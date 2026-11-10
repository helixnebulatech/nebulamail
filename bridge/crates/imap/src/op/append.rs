// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! APPEND, with MULTIAPPEND (RFC 3502), BINARY `literal8` (RFC 3516) and
//! the `UTF8` data extension (RFC 6855 §4).

use nebula_imap_proto::command::{Append, AppendData, AppendMessage, MultiAppend};
use nebula_imap_proto::response::ResponseCode;
use nebula_imap_proto::types::{Capability, Flag, InternalDate, MailboxName, Uid, UidValidity};

use crate::Error;
use crate::core::client::{Client, LoggedIn};

/// Result of APPEND.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct AppendResult {
    /// UIDVALIDITY and UID of the new message (UIDPLUS `APPENDUID`).
    pub uid: Option<(UidValidity, Uid)>,
}

/// Result of an APPEND of several messages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct AppendManyResult {
    /// UIDVALIDITY and the UIDs of the new messages, in the order given
    /// (UIDPLUS `APPENDUID`; `None` if the server sent none, or a set of
    /// the wrong size).
    pub uids: Option<(UidValidity, Vec<Uid>)>,
}

impl<S: LoggedIn> Client<S> {
    fn append_data(&self) -> AppendData {
        let mut data = AppendData::TEXT;
        if self.session.has(&Capability::Binary) {
            data = data.with_binary();
        }
        // RFC 6855 §4: UTF-8 header fields go as `UTF8 (~{n})`, which is
        // only allowed after `ENABLE UTF8=ACCEPT`.
        if self.session.utf8_accept {
            data = data.with_utf8();
        }
        data
    }

    /// Appends a message (RFC 5322 bytes, CRLF line endings) to a mailbox.
    /// Uses a non-synchronizing literal with LITERAL+ (or LITERAL- up to
    /// 4 KiB), else waits for the server's `+`. A message with NUL bytes is
    /// sent as `literal8` when the server has BINARY, and one with UTF-8
    /// header fields as `UTF8 (~{n})` after `ENABLE UTF8=ACCEPT`.
    ///
    /// # Errors
    ///
    /// NO (`TRYCREATE`, `OVERQUOTA`, `LIMIT`…), messages with NUL bytes
    /// without BINARY, or connection errors.
    pub async fn append(
        &mut self,
        mailbox: &MailboxName,
        message: &[u8],
        flags: &[Flag],
        internal_date: Option<InternalDate>,
    ) -> Result<AppendResult, Error> {
        let command = Append {
            mailbox,
            flags,
            internal_date,
            message,
            data: self.append_data(),
        };
        let tagged = self.run_simple(&command).await?;
        let uid = match tagged.code {
            Some(ResponseCode::AppendUid { uid_validity, uids }) => {
                // One message appended: one UID.
                uids.iter_values()
                    .and_then(|mut values| values.next())
                    .map(|uid| (uid_validity, uid))
            }
            _ => None,
        };
        Ok(AppendResult { uid })
    }

    /// Appends several messages to one mailbox in a single command
    /// (MULTIAPPEND): the server adds all of them or none.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] without MULTIAPPEND; otherwise as
    /// [`Self::append`] (a NO means nothing was added).
    pub async fn append_many(
        &mut self,
        mailbox: &MailboxName,
        messages: &[AppendMessage<'_>],
    ) -> Result<AppendManyResult, Error> {
        if !self.session.has(&Capability::MultiAppend) {
            return Err(Error::Unsupported("MULTIAPPEND"));
        }
        let command = MultiAppend {
            mailbox,
            messages,
            data: self.append_data(),
        };
        let tagged = self.run_simple(&command).await?;
        let uids = match tagged.code {
            Some(ResponseCode::AppendUid { uid_validity, uids }) => {
                let count = u64::try_from(messages.len()).unwrap_or(u64::MAX);
                if uids.count() == Some(count) {
                    uids.iter_values()
                        .map(|values| (uid_validity, values.collect()))
                } else {
                    tracing::warn!("APPENDUID ignored: set of the wrong size");
                    None
                }
            }
            _ => None,
        };
        Ok(AppendManyResult { uids })
    }
}
