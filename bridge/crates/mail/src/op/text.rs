// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Body text without the whole message: `BODYSTRUCTURE`, then only the
//! first bytes of the text part (`BODY.PEEK[1.2]<0.n>`), for previews and
//! search snippets. Emails of the same mailbox are fetched together, one
//! `UID FETCH` per part path (usually one or two).

use std::collections::HashMap;
use std::num::NonZeroU32;

use nebula_imap::proto::command::{FetchAttribute, FetchModifiers, PartPath, Partial, Section};
use nebula_imap::proto::response::SectionKind;
use nebula_imap::proto::types::{Uid, UidSet};
use nebula_store::{EmailId, Location};

use super::select_location_by_name;
use crate::MailError;
use crate::core::account::MailAccount;
use crate::model::preview;
use crate::model::text::{TextPart, decode, text_part};

/// The emails whose text part is at the same path.
type PathGroup = (Vec<u32>, Vec<(Uid, TextPart)>);

/// Bytes of the text part fetched to compute a missing preview.
const PREVIEW_BYTES: u32 = 4096;

impl MailAccount {
    /// The body text (HTML turned into text) of each email, from at most
    /// `max_bytes` of its text part. Emails without a text part, or no
    /// longer on the server, are left out.
    ///
    /// # Errors
    /// Connection errors.
    pub async fn body_texts(
        &self,
        emails: &[EmailId],
        max_bytes: NonZeroU32,
    ) -> Result<HashMap<EmailId, String>, MailError> {
        let mut out = HashMap::new();
        let mut groups: Vec<Vec<(EmailId, Location)>> = Vec::new();
        for email in emails {
            if out.contains_key(email) {
                continue;
            }
            // The store first: a cached message may have been deleted.
            let location = match self.visible_locations(*email).await {
                Ok(list) => list.into_iter().next(),
                Err(MailError::NotFound(_)) => None,
                Err(error) => return Err(error),
            };
            let Some(location) = location else {
                continue;
            };
            // Already downloaded whole: nothing to fetch.
            if let Some(raw) = self.shared.cache.get(self.id, *email) {
                if let Some(mut text) = crate::ParsedMessage::parse(raw.as_ref())
                    .and_then(|m| m.message().body_text(0).map(std::borrow::Cow::into_owned))
                {
                    truncate_at_char(&mut text, max_bytes.get());
                    out.insert(*email, text);
                }
                continue;
            }
            match groups.iter_mut().find(|g| {
                g.first()
                    .is_some_and(|(_, l)| l.imap_name == location.imap_name)
            }) {
                Some(group) => group.push((*email, location)),
                None => groups.push(vec![(*email, location)]),
            }
        }
        for group in groups {
            self.fetch_texts(&group, max_bytes, &mut out).await?;
        }
        Ok(out)
    }

    async fn fetch_texts(
        &self,
        group: &[(EmailId, Location)],
        max_bytes: NonZeroU32,
        out: &mut HashMap<EmailId, String>,
    ) -> Result<(), MailError> {
        let Some((_, first)) = group.first() else {
            return Ok(());
        };
        let mut slot = self.slot().await;
        let client = select_location_by_name(self, &mut slot, &first.imap_name).await?;
        let validity = client.mailbox().uid_validity;
        // Copies from before a UIDVALIDITY change are out of date.
        let by_uid: HashMap<Uid, EmailId> = group
            .iter()
            .filter(|(_, l)| l.uid_validity == Some(validity))
            .map(|(email, l)| (l.uid, *email))
            .collect();
        let Some(set) = UidSet::from_values(by_uid.keys().copied()) else {
            return Ok(());
        };
        let result = client
            .uid_fetch(&set, &[FetchAttribute::BodyStructure], FetchModifiers::None)
            .await;
        let fetched = slot.check(result)?;
        // Path → the emails whose text is there.
        let mut by_path: Vec<PathGroup> = Vec::new();
        for data in fetched.messages {
            let (Some(uid), Some(structure)) = (data.uid, data.body_structure.as_ref()) else {
                continue;
            };
            let Some(part) = text_part(structure) else {
                continue;
            };
            match by_path.iter_mut().find(|(path, _)| *path == part.path) {
                Some((_, list)) => list.push((uid, part)),
                None => by_path.push((part.path.clone(), vec![(uid, part)])),
            }
        }
        for (path, parts) in by_path {
            let Some(section) = part_path(&path) else {
                continue;
            };
            let Some(set) = UidSet::from_values(parts.iter().map(|(uid, _)| *uid)) else {
                continue;
            };
            let attributes = [FetchAttribute::BodyPeek {
                section: Section::Part(section),
                partial: Some(Partial {
                    offset: 0,
                    length: max_bytes,
                }),
            }];
            let client = slot
                .selected()
                .ok_or(MailError::NotFound("selected mailbox"))?;
            let result = client
                .uid_fetch(&set, &attributes, FetchModifiers::None)
                .await;
            let fetched = slot.check(result)?;
            for data in fetched.messages {
                let Some(uid) = data.uid else { continue };
                let (Some(email), Some((_, part))) =
                    (by_uid.get(&uid), parts.iter().find(|(u, _)| *u == uid))
                else {
                    continue;
                };
                let bytes = data
                    .sections
                    .into_iter()
                    .find(|s| s.section == SectionKind::Part(path.clone()))
                    .and_then(|s| s.data);
                let Some(bytes) = bytes else { continue };
                let truncated = u32::try_from(bytes.len()).is_ok_and(|n| n >= max_bytes.get());
                if let Some(text) = decode(part, &bytes, truncated) {
                    out.insert(*email, text);
                }
            }
        }
        Ok(())
    }

    /// Computes and stores the previews the server didn't give (no
    /// `PREVIEW` extension), from the first bytes of each text part.
    /// Returns the ones found.
    ///
    /// # Errors
    /// Connection and database errors.
    pub async fn fill_previews(
        &self,
        emails: &[EmailId],
    ) -> Result<HashMap<EmailId, String>, MailError> {
        let bytes = NonZeroU32::new(PREVIEW_BYTES).ok_or(MailError::Invalid("preview size"))?;
        let texts = self.body_texts(emails, bytes).await?;
        let mut out = HashMap::new();
        for (email, text) in texts {
            let clean = preview::clean(&text);
            if clean.is_empty() {
                continue;
            }
            self.store()
                .set_preview(self.id, email, clean.clone())
                .await?;
            out.insert(email, clean);
        }
        Ok(out)
    }
}

/// Cuts `text` to at most `max` bytes, at a character boundary.
fn truncate_at_char(text: &mut String, max: u32) {
    let max = usize::try_from(max).unwrap_or(usize::MAX);
    if text.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    text.truncate(end);
}

fn part_path(path: &[u32]) -> Option<PartPath> {
    let parts = path
        .iter()
        .map(|n| NonZeroU32::new(*n))
        .collect::<Option<Vec<_>>>()?;
    PartPath::new(parts).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_at_a_character() {
        let mut text = "añob".to_owned();
        truncate_at_char(&mut text, 2);
        assert_eq!(text, "a");
        let mut text = "añob".to_owned();
        truncate_at_char(&mut text, 3);
        assert_eq!(text, "añ");
        let mut text = "abc".to_owned();
        truncate_at_char(&mut text, 10);
        assert_eq!(text, "abc");
        truncate_at_char(&mut text, 0);
        assert_eq!(text, "");
    }
}
