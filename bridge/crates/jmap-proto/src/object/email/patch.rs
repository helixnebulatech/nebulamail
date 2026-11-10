// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Email/set` updates (RFC 8621 §4.6): only `keywords` and `mailboxIds`
//! can change.

use std::collections::BTreeSet;
use std::str::FromStr;

use serde_json::Value;

use super::property::EmailField;
use crate::error::SetError;
use crate::method::PatchObject;
use crate::method::parse::PropertyErrors;
use crate::method::patch::{Patch, Target};
use crate::references::IdOrRef;
use crate::types::{Keyword, MailboxId, TrueSet};

/// A change to a set-like property (`keywords`, `mailboxIds`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SetPatch<T: Ord> {
    /// Replace the whole set (`"keywords": {...}`).
    Replace(BTreeSet<T>),
    /// Add and remove members (`"keywords/$seen": true` / `null`). The two
    /// sets are disjoint.
    Change {
        /// Members to add.
        add: BTreeSet<T>,
        /// Members to remove.
        remove: BTreeSet<T>,
    },
}

impl<T: Ord> SetPatch<T> {
    /// Applies the patch to the current members.
    pub fn apply(&self, current: &mut BTreeSet<T>)
    where
        T: Clone,
    {
        match self {
            Self::Replace(set) => set.clone_into(current),
            Self::Change { add, remove } => {
                current.retain(|item| !remove.contains(item));
                current.extend(add.iter().cloned());
            }
        }
    }
}

/// A parsed `Email/set` update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmailPatch {
    /// Keyword changes, if any.
    pub keywords: Option<SetPatch<Keyword>>,
    /// Mailbox changes, if any (`#creationId` allowed).
    pub mailbox_ids: Option<SetPatch<IdOrRef<MailboxId>>>,
}

/// Members collected from `keywords/…` or `mailboxIds/…` patches (also
/// `calendarIds/…` and `addressBookIds/…`).
pub(crate) struct Members<T: Ord> {
    add: BTreeSet<T>,
    remove: BTreeSet<T>,
}

impl<T: Ord> Default for Members<T> {
    fn default() -> Self {
        Self {
            add: BTreeSet::new(),
            remove: BTreeSet::new(),
        }
    }
}

impl<T: Ord + FromStr> Members<T> {
    /// Records `property/member: value` (`true` adds, `null` removes).
    pub(crate) fn record(
        &mut self,
        key: &str,
        member: &str,
        value: &Value,
    ) -> Result<(), SetError> {
        let Ok(item) = member.parse::<T>() else {
            return Err(SetError::invalid_properties([key]));
        };
        let duplicate = match value {
            Value::Bool(true) => self.remove.contains(&item) || !self.add.insert(item),
            Value::Null => self.add.contains(&item) || !self.remove.insert(item),
            Value::Bool(false)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_)
            | Value::Object(_) => {
                return Err(SetError::invalid_properties([key])
                    .with_description(format!("{key}: value must be true or null")));
            }
        };
        if duplicate {
            // Two keys for the same keyword in different case.
            return Err(SetError::invalid_patch(format!("{key:?} is patched twice")));
        }
        Ok(())
    }

    pub(crate) fn into_patch(self) -> Option<SetPatch<T>> {
        if self.add.is_empty() && self.remove.is_empty() {
            None
        } else {
            Some(SetPatch::Change {
                add: self.add,
                remove: self.remove,
            })
        }
    }
}

impl EmailPatch {
    /// Parses a `PatchObject`.
    ///
    /// # Errors
    /// `invalidPatch` for bad or overlapping paths; `invalidProperties`
    /// for other (immutable or unknown) properties and bad values.
    pub fn parse(patch: &PatchObject) -> Result<Self, SetError> {
        let mut errors = PropertyErrors::new();
        let mut keywords = None;
        let mut mailbox_ids = None;
        let mut keyword_members = Members::<Keyword>::default();
        let mut mailbox_members = Members::<IdOrRef<MailboxId>>::default();
        for Patch { key, path, value } in patch.patches()? {
            match Target::of(&path) {
                Target::Property("keywords") => {
                    keywords = errors
                        .parse::<TrueSet<Keyword>>(&key, value)
                        .map(|set| SetPatch::Replace(set.into_set()));
                }
                Target::Property("mailboxIds") => {
                    mailbox_ids = errors
                        .parse::<TrueSet<IdOrRef<MailboxId>>>(&key, value)
                        .map(|set| SetPatch::Replace(set.into_set()));
                }
                Target::Member("keywords", member) => {
                    keyword_members.record(&key, member, &value)?;
                }
                Target::Member("mailboxIds", member) => {
                    mailbox_members.record(&key, member, &value)?;
                }
                Target::Deeper("keywords" | "mailboxIds") => {
                    return Err(SetError::invalid_patch(format!("{key:?} goes too deep")));
                }
                Target::Property(property)
                | Target::Member(property, _)
                | Target::Deeper(property) => {
                    let known =
                        property.parse::<EmailField>().is_ok() || property.starts_with("header:");
                    if known {
                        errors.invalid_because(&key, "immutable");
                    } else {
                        errors.invalid(key);
                    }
                }
            }
        }
        errors.finish()?;
        Ok(Self {
            keywords: keywords.or_else(|| keyword_members.into_patch()),
            mailbox_ids: mailbox_ids.or_else(|| mailbox_members.into_patch()),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::SetErrorType;

    fn parse(value: Value) -> Result<EmailPatch, SetError> {
        EmailPatch::parse(&serde_json::from_value(value).unwrap())
    }

    fn ids(items: &[&str]) -> BTreeSet<IdOrRef<MailboxId>> {
        items.iter().map(|s| s.parse().unwrap()).collect()
    }

    /// What the web client sends when a message is sent (compose.ts).
    #[test]
    fn move_from_drafts_to_sent() {
        let patch = parse(json!({
            "keywords/$draft": null,
            "mailboxIds/m-drafts": null,
            "mailboxIds/m-sent": true
        }))
        .unwrap();
        assert_eq!(
            patch.keywords,
            Some(SetPatch::Change {
                add: BTreeSet::new(),
                remove: [Keyword::draft()].into()
            })
        );
        assert_eq!(
            patch.mailbox_ids,
            Some(SetPatch::Change {
                add: ids(&["m-sent"]),
                remove: ids(&["m-drafts"])
            })
        );
        let mut current: BTreeSet<_> = ids(&["m-drafts", "m-other"]);
        patch.mailbox_ids.unwrap().apply(&mut current);
        assert_eq!(current, ids(&["m-other", "m-sent"]));
    }

    /// RFC 8621 §4.10 example: replace and creation references.
    #[test]
    fn replace_and_references() {
        let patch = parse(json!({
            "keywords": {"$seen": true, "$flagged": true},
            "mailboxIds/#newbox": true
        }))
        .unwrap();
        assert_eq!(
            patch.keywords,
            Some(SetPatch::Replace(
                [Keyword::seen(), Keyword::flagged()].into()
            ))
        );
        assert_eq!(
            patch.mailbox_ids,
            Some(SetPatch::Change {
                add: ids(&["#newbox"]),
                remove: BTreeSet::new()
            })
        );
        // `~1` is `/` inside a keyword, which is allowed in keywords.
        let escaped = parse(json!({"keywords/a~1b": true})).unwrap();
        assert!(matches!(
            escaped.keywords,
            Some(SetPatch::Change { add, .. }) if add.contains(&"a/b".parse().unwrap())
        ));
    }

    #[test]
    fn errors() {
        let kind = |value: Value| parse(value).unwrap_err().kind();
        assert_eq!(
            kind(json!({"subject": "x"})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(kind(json!({"nope": 1})), SetErrorType::InvalidProperties);
        assert_eq!(
            kind(json!({"header:X-A": " 1"})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"keywords/$seen": false})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"keywords/a b": true})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"mailboxIds/a b": true})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"keywords": {"$seen": false}})),
            SetErrorType::InvalidProperties
        );
        assert_eq!(
            kind(json!({"keywords": {}, "keywords/$seen": true})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(
            kind(json!({"keywords/$seen/x": true})),
            SetErrorType::InvalidPatch
        );
        assert_eq!(
            kind(json!({"keywords/$Seen": true, "keywords/$seen": null})),
            SetErrorType::InvalidPatch
        );
    }
}
