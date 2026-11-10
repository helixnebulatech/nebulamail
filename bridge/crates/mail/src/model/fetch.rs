// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! FETCH responses → what the store takes.

use nebula_imap::proto::response::{FetchData, Preview, SectionKind, ThreadId};
use nebula_imap::proto::types::Uid;
use nebula_store::{EmailMeta, FlagUpdate, NewMessage, ServerId, ServerIds, UnixTime};

use super::keywords::from_flags;
use super::{headers, preview};

/// A message from a metadata FETCH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fetched {
    /// A message to store.
    Message(Box<NewMessage>),
    /// Marked `\Deleted`: treated as gone.
    Deleted(Uid),
}

/// The flags of a message from a FLAGS (and MODSEQ) FETCH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FlagChange {
    /// New flags.
    Update(FlagUpdate),
    /// Marked `\Deleted`: treated as gone.
    Deleted(Uid),
}

pub(crate) fn flag_change(data: &FetchData) -> Option<FlagChange> {
    let uid = data.uid?;
    let flags = from_flags(data.flags.as_deref()?);
    if flags.deleted {
        return Some(FlagChange::Deleted(uid));
    }
    Some(FlagChange::Update(FlagUpdate {
        uid,
        mod_seq: data.mod_seq,
        keywords: flags.keywords,
    }))
}

fn server_ids(data: &FetchData) -> ServerIds {
    let email = data
        .email_id
        .as_ref()
        .and_then(|id| ServerId::new(id.as_str()).ok())
        .or_else(|| {
            data.gmail_msg_id
                .and_then(|id| ServerId::new(&format!("gm{id}")).ok())
        });
    let thread = match &data.thread_id {
        Some(ThreadId::Id(id)) => ServerId::new(id.as_str()).ok(),
        _ => data
            .gmail_thread_id
            .and_then(|id| ServerId::new(&format!("gt{id}")).ok()),
    };
    ServerIds { email, thread }
}

/// Whether a FETCH response is the answer to a metadata FETCH (it carries
/// the header section), and not an unsolicited FLAGS or MODSEQ update for
/// the same UID that came first: that one would be stored as an empty
/// email.
pub(crate) fn is_metadata_answer(data: &FetchData) -> bool {
    data.sections
        .iter()
        .any(|section| section.section == SectionKind::Header)
}

/// Converts a metadata FETCH. `now` is the receive time of a message
/// without INTERNALDATE (a broken server).
pub(crate) fn message(data: &FetchData, now: UnixTime) -> Option<Fetched> {
    let uid = data.uid?;
    let flags = from_flags(data.flags.as_deref().unwrap_or(&[]));
    if flags.deleted {
        return Some(Fetched::Deleted(uid));
    }
    let header = data
        .sections
        .iter()
        .find(|section| section.section == SectionKind::Header)
        .and_then(|section| section.data.as_deref())
        .unwrap_or(&[]);
    let parsed = headers::parse(header);
    let received_at = data
        .internal_date
        .as_ref()
        .map(|date| UnixTime::from_secs(date.unix_timestamp()))
        .or(parsed.sent_at.map(|sent| sent.utc))
        .unwrap_or(now);
    let preview = match &data.preview {
        Some(Preview::Text(text)) => Some(preview::clean(text)).filter(|p| !p.is_empty()),
        _ => None,
    };
    let size = data
        .size
        .map_or_else(|| u64::try_from(header.len()).unwrap_or(0), u64::from);
    let meta = EmailMeta {
        message_id: parsed.message_id,
        in_reply_to: parsed.in_reply_to,
        references: parsed.references,
        subject: parsed.subject,
        from: parsed.from,
        to: parsed.to,
        cc: parsed.cc,
        bcc: parsed.bcc,
        reply_to: parsed.reply_to,
        sender: parsed.sender,
        sent_at: parsed.sent_at,
        received_at,
        size,
        has_attachment: data
            .body_structure
            .as_ref()
            .is_some_and(nebula_imap::proto::response::BodyStructure::has_attachment),
        preview,
        headers: parsed.raw,
    };
    Some(Fetched::Message(Box::new(NewMessage {
        uid,
        mod_seq: data.mod_seq,
        keywords: flags.keywords,
        server: server_ids(data),
        meta,
    })))
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::response::{FrameLimits, Response, ResponseDecoder, Untagged};

    use nebula_imap::proto::types::ModSeq;

    use super::*;

    fn decode(wire: &[u8]) -> FetchData {
        let mut decoder = ResponseDecoder::new(FrameLimits::default());
        decoder.feed(wire);
        match decoder.decode() {
            Ok(Some(Response::Untagged(Untagged::Fetch(data)))) => *data,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn metadata_fetch() {
        let header = "Subject: Hola\r\nMessage-ID: <m1@x>\r\nFrom: Bob <bob@x>\r\n\r\n";
        let wire = format!(
            "* 3 FETCH (UID 42 FLAGS (\\Seen $Forwarded) INTERNALDATE \"24-Oct-2026 10:00:00 +0000\" \
             RFC822.SIZE 1234 MODSEQ (77) OBJECTID (EMAILID e1 THREADID t1) \
             BODY[HEADER.FIELDS (SUBJECT MESSAGE-ID FROM)] {{{}}}\r\n{header})\r\n",
            header.len()
        );
        let data = decode(wire.as_bytes());
        let Some(Fetched::Message(message)) = message(&data, UnixTime::from_secs(1)) else {
            panic!()
        };
        assert_eq!(message.uid.get(), 42);
        assert_eq!(message.mod_seq.map(ModSeq::get), Some(77));
        assert_eq!(message.keywords.len(), 2);
        assert_eq!(
            message.server.email.as_ref().map(ServerId::as_str),
            Some("e1")
        );
        assert_eq!(
            message.server.thread.as_ref().map(ServerId::as_str),
            Some("t1")
        );
        assert_eq!(message.meta.subject.as_deref(), Some("Hola"));
        assert_eq!(message.meta.message_id.as_deref(), Some("m1@x"));
        assert_eq!(message.meta.size, 1234);
        assert_eq!(message.meta.received_at.secs(), 1_792_836_000);
        assert_eq!(message.meta.headers, header.as_bytes());
        assert!(!message.meta.has_attachment);
        assert!(is_metadata_answer(&data));
        let flags_only = decode(b"* 5 FETCH (UID 123 FLAGS (\\Seen) MODSEQ (900))\r\n");
        assert!(!is_metadata_answer(&flags_only));
    }

    #[test]
    fn deleted_and_flag_only() {
        let data = decode(b"* 1 FETCH (UID 5 FLAGS (\\Deleted \\Seen))\r\n");
        assert_eq!(
            message(&data, UnixTime::from_secs(1)),
            Some(Fetched::Deleted(Uid::new(5).unwrap()))
        );
        assert_eq!(
            flag_change(&data),
            Some(FlagChange::Deleted(Uid::new(5).unwrap()))
        );
        let data = decode(b"* 1 FETCH (UID 6 MODSEQ (9) FLAGS (\\Flagged))\r\n");
        let Some(FlagChange::Update(update)) = flag_change(&data) else {
            panic!()
        };
        assert_eq!(update.uid.get(), 6);
        assert_eq!(update.mod_seq.map(ModSeq::get), Some(9));
        // No FLAGS: nothing to apply.
        assert_eq!(flag_change(&decode(b"* 1 FETCH (UID 6)\r\n")), None);
    }

    #[test]
    fn gmail_ids() {
        let data = decode(b"* 1 FETCH (UID 7 X-GM-MSGID 123 X-GM-THRID 456 FLAGS ())\r\n");
        let Some(Fetched::Message(message)) = message(&data, UnixTime::from_secs(9)) else {
            panic!()
        };
        assert_eq!(
            message.server.email.as_ref().map(ServerId::as_str),
            Some("gm123")
        );
        assert_eq!(
            message.server.thread.as_ref().map(ServerId::as_str),
            Some("gt456")
        );
        // No INTERNALDATE nor Date: the fallback time.
        assert_eq!(message.meta.received_at.secs(), 9);
    }
}
