// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Downloads in pieces: a whole message (`BODY.PEEK[]<offset.length>`) or
//! one part (`BINARY.PEEK[1.2]<…>` when the server has BINARY and agrees on
//! the decoded size, else `BODY.PEEK[1.2]<…>` decoded here piece by piece).
//! Memory stays at one piece per download, whatever the size.
//!
//! Messages up to [`MailConfig::raw_cache_max_item`](crate::MailConfig)
//! are still fetched whole, because they go into the cache; only the
//! bigger ones (never cached) are streamed. Each piece takes the command
//! connection for one `UID FETCH`, so other operations go on meanwhile.
//!
//! The size is known before the first byte (it is the `Content-Length`):
//! RFC822.SIZE for a message (checked against the server first), and the
//! decoded size checked when the part's section was given out
//! ([`crate::streamable_parts`]). If the server sends anything else, the
//! reader fails and the download is cut short: never a wrong length.

use std::sync::Arc;

use nebula_imap::proto::command::{FetchAttribute, FetchModifiers, PartPath, Partial, Section};
use nebula_imap::proto::response::SectionKind;
use nebula_imap::proto::types::Capability;
use nebula_store::{EmailId, Location};

use super::{select_location, single};
use crate::MailError;
use crate::core::account::MailAccount;
use crate::model::layout::{MessageStructure, PartSection, encoding_of, find, server_type};
use crate::model::transfer::{Decoder, Encoding, check_pending};

/// Bytes asked for in each `UID FETCH`.
const PIECE: u32 = 1024 * 1024;

/// Most bytes a decoder may hold back between pieces (one piece): more is
/// a hostile part (a quoted-printable line of only blanks).
const MAX_PENDING: usize = 1024 * 1024;

/// What one piece is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// `BODY.PEEK[]`
    Message,
    /// `BODY.PEEK[part]` (still encoded).
    Body(PartPath),
    /// `BINARY.PEEK[part]` (decoded by the server).
    Binary(PartPath),
}

/// Reads a message or a part piece by piece; see the module docs.
pub struct ChunkReader {
    account: Arc<MailAccount>,
    location: Location,
    source: Source,
    /// Next offset in the source.
    offset: u32,
    size: u64,
    sent: u64,
    decoder: Option<Decoder>,
    done: bool,
}

impl std::fmt::Debug for ChunkReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkReader")
            .field("source", &self.source)
            .field("size", &self.size)
            .field("sent", &self.sent)
            .finish_non_exhaustive()
    }
}

/// A part to download in pieces.
#[derive(Debug)]
#[non_exhaustive]
pub struct PartDownload {
    /// `type/subtype` from the server's BODYSTRUCTURE (lower case).
    pub media_type: String,
    /// The pieces.
    pub reader: ChunkReader,
}

impl ChunkReader {
    /// The total size in bytes that will be read.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The next piece; `None` at the end.
    ///
    /// # Errors
    /// [`MailError::NotFound`] if the message went away, [`MailError::Invalid`]
    /// if the server sent other bytes than announced (more or fewer once
    /// decoded), and connection errors.
    pub async fn next(&mut self) -> Result<Option<Vec<u8>>, MailError> {
        while !self.done {
            let data = self
                .account
                .fetch_piece(&self.location, &self.source, self.offset, PIECE)
                .await?;
            let read = u32::try_from(data.len()).map_err(|_| MailError::Invalid("piece size"))?;
            if read > PIECE {
                return Err(MailError::Invalid("server sent a bigger piece"));
            }
            self.offset = self.offset.saturating_add(read);
            let last = read < PIECE;
            let mut out = match &mut self.decoder {
                None => data,
                Some(decoder) => {
                    let mut out = Vec::with_capacity(data.len());
                    decoder.push(&data, &mut out);
                    if let Err(error) = check_pending(decoder, MAX_PENDING) {
                        self.done = true;
                        return Err(error);
                    }
                    out
                }
            };
            if last {
                self.done = true;
                if let Some(decoder) = self.decoder.take() {
                    decoder.finish(&mut out);
                }
            }
            let len = u64::try_from(out.len()).map_err(|_| MailError::Invalid("piece size"))?;
            self.sent = self.sent.saturating_add(len);
            if self.sent > self.size || (self.done && self.sent != self.size) {
                self.done = true;
                return Err(MailError::Invalid(
                    "download size differs from the server's",
                ));
            }
            if !out.is_empty() {
                return Ok(Some(out));
            }
        }
        Ok(None)
    }
}

impl MailAccount {
    /// Messages bigger than this are streamed (and never cached); smaller
    /// ones are fetched whole into the cache.
    #[must_use]
    pub fn stream_threshold(&self) -> u64 {
        self.config().raw_cache_max_item
    }

    /// The message if it is in the cache (no server round trip), checked
    /// against the store: an email that is gone is forgotten.
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an email that is not (any more) in the
    /// store, and database errors.
    pub async fn cached_message(&self, email: EmailId) -> Result<Option<Arc<[u8]>>, MailError> {
        self.visible_locations(email).await?;
        Ok(self.shared.cache.get(self.id, email))
    }

    /// The server's MIME structure of a message, `None` if it has none we
    /// can read (too deeply nested).
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an unknown email, and connection errors.
    pub async fn body_structure(
        &self,
        email: EmailId,
    ) -> Result<Option<MessageStructure>, MailError> {
        let location = self.first_location(email).await?;
        self.structure_at(&location).await
    }

    async fn first_location(&self, email: EmailId) -> Result<Location, MailError> {
        self.email_locations(email)
            .await?
            .into_iter()
            .next()
            .ok_or(MailError::NotFound("email"))
    }

    pub(crate) async fn structure_at(
        &self,
        location: &Location,
    ) -> Result<Option<MessageStructure>, MailError> {
        let mut slot = self.slot().await;
        let client = select_location(self, &mut slot, location).await?;
        let result = client
            .uid_fetch(
                &single(location),
                &[FetchAttribute::BodyStructure],
                FetchModifiers::None,
            )
            .await;
        let fetched = slot.check(result)?;
        Ok(fetched
            .messages
            .into_iter()
            .filter(|data| data.uid == Some(location.uid))
            .find_map(|data| data.body_structure)
            .map(MessageStructure))
    }

    /// The whole message in pieces, if it is bigger than
    /// [`Self::stream_threshold`]; `None` means "fetch it whole" (small, or
    /// the server's RFC822.SIZE is not its real size).
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an unknown email, and connection errors.
    pub async fn message_reader(
        self: &Arc<Self>,
        email: EmailId,
    ) -> Result<Option<ChunkReader>, MailError> {
        let size = self
            .store()
            .emails(self.id, vec![email])
            .await?
            .first()
            .map(|stored| stored.meta.size)
            .ok_or(MailError::NotFound("email"))?;
        if size <= self.stream_threshold() {
            return Ok(None);
        }
        let Some(last) = size.checked_sub(1).and_then(|n| u32::try_from(n).ok()) else {
            return Ok(None);
        };
        let location = self.first_location(email).await?;
        // The `Content-Length` is RFC822.SIZE: check that the message ends
        // exactly there (its last byte exists and nothing follows).
        let tail = self
            .fetch_piece(&location, &Source::Message, last, 2)
            .await?;
        if tail.len() != 1 {
            tracing::debug!("RFC822.SIZE is not the message size; fetching it whole");
            return Ok(None);
        }
        Ok(Some(ChunkReader {
            account: Arc::clone(self),
            location,
            source: Source::Message,
            offset: 0,
            size,
            sent: 0,
            decoder: None,
            done: false,
        }))
    }

    /// One part in pieces, at the section checked by
    /// [`crate::streamable_parts`]. `None` if the server's structure no
    /// longer matches (then download it from the whole message).
    ///
    /// # Errors
    /// [`MailError::NotFound`] for an unknown email, and connection errors.
    pub async fn part_reader(
        self: &Arc<Self>,
        email: EmailId,
        section: &PartSection,
    ) -> Result<Option<PartDownload>, MailError> {
        let location = self.first_location(email).await?;
        let Some(structure) = self.structure_at(&location).await? else {
            return Ok(None);
        };
        let Some((part, _)) = find(&structure.0, section.path()) else {
            return Ok(None);
        };
        let Some(encoding) = encoding_of(&part.encoding) else {
            return Ok(None);
        };
        let media_type = server_type(&part.common.media_type, &part.common.subtype);
        let mut source = Source::Body(section.part_path().clone());
        let mut decoder = Some(Decoder::new(encoding));
        if encoding != Encoding::Identity
            && self.config().binary_downloads
            && self.binary_size(&location, section).await? == Some(section.size())
        {
            source = Source::Binary(section.part_path().clone());
            decoder = None;
        }
        Ok(Some(PartDownload {
            media_type,
            reader: ChunkReader {
                account: Arc::clone(self),
                location,
                source,
                offset: 0,
                size: section.size(),
                sent: 0,
                decoder,
                done: false,
            },
        }))
    }

    /// `BINARY.SIZE[part]`, `None` without BINARY.
    async fn binary_size(
        &self,
        location: &Location,
        section: &PartSection,
    ) -> Result<Option<u64>, MailError> {
        let mut slot = self.slot().await;
        let client = select_location(self, &mut slot, location).await?;
        if !client.capabilities().has(&Capability::Binary) {
            return Ok(None);
        }
        let attributes = [FetchAttribute::BinarySize {
            part: Some(section.part_path().clone()),
        }];
        let result = client
            .uid_fetch(&single(location), &attributes, FetchModifiers::None)
            .await;
        let fetched = match slot.check(result) {
            Ok(fetched) => fetched,
            // A server that refuses (e.g. it can't decode that part):
            // decode it here instead.
            Err(MailError::Refused(_) | MailError::Unsupported(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let wanted: Vec<u32> = section.path().iter().map(|n| n.get()).collect();
        Ok(fetched
            .messages
            .into_iter()
            .filter(|data| data.uid == Some(location.uid))
            .flat_map(|data| data.binary_sizes)
            .find(|(part, _)| *part == wanted)
            .map(|(_, size)| u64::from(size)))
    }

    pub(crate) async fn fetch_piece(
        &self,
        location: &Location,
        source: &Source,
        offset: u32,
        length: u32,
    ) -> Result<Vec<u8>, MailError> {
        let length = std::num::NonZeroU32::new(length).ok_or(MailError::Invalid("piece size"))?;
        let partial = Some(Partial { offset, length });
        let attribute = match source {
            Source::Message => FetchAttribute::BodyPeek {
                section: Section::Full,
                partial,
            },
            Source::Body(path) => FetchAttribute::BodyPeek {
                section: Section::Part(path.clone()),
                partial,
            },
            Source::Binary(path) => FetchAttribute::BinaryPeek {
                part: Some(path.clone()),
                partial,
            },
        };
        let mut slot = self.slot().await;
        let client = select_location(self, &mut slot, location).await?;
        let result = client
            .uid_fetch(&single(location), &[attribute], FetchModifiers::None)
            .await;
        let fetched = slot.check(result)?;
        drop(slot);
        let mine = fetched
            .messages
            .into_iter()
            .filter(|data| data.uid == Some(location.uid));
        let found = match source {
            Source::Message => mine
                .flat_map(|data| data.sections)
                .find(|s| s.section == SectionKind::Full && s.origin.is_none_or(|o| o == offset))
                .map(|s| s.data),
            Source::Body(path) => {
                let wanted: Vec<u32> = path.parts().iter().map(|n| n.get()).collect();
                mine.flat_map(|data| data.sections)
                    .find(|s| {
                        s.section == SectionKind::Part(wanted.clone())
                            && s.origin.is_none_or(|o| o == offset)
                    })
                    .map(|s| s.data)
            }
            Source::Binary(path) => {
                let wanted: Vec<u32> = path.parts().iter().map(|n| n.get()).collect();
                mine.flat_map(|data| data.binary)
                    .find(|b| b.part == wanted && b.origin.is_none_or(|o| o == offset))
                    .map(|b| b.data)
            }
        };
        // `NIL` (or an empty string) past the end: nothing more.
        found
            .map(Option::unwrap_or_default)
            .ok_or(MailError::NotFound("message"))
    }
}
