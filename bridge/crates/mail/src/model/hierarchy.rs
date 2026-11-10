// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! From the LIST answer to the mailbox tree: parent, display name and role
//! of each mailbox.
//!
//! - The parent is the nearest *listed* ancestor: for `A/B/C` without
//!   `A/B`, the parent is `A` and the name `B/C` (JMAP needs a real parent).
//! - With a personal namespace prefix such as `INBOX.` (Courier, Cyrus,
//!   some Dovecot setups), `INBOX.Sent` is shown as a top-level `Sent`, not
//!   as a child of the inbox.
//! - `\NonExistent` entries are skipped; `\Noselect` ones are kept (they
//!   hold children) but are not selectable.
//! - `\Noinferiors` is kept: no child can be created below it (RFC 9051
//!   §7.3.1), so JMAP's `mayCreateChild` is false (RFC 8621 §2).

use std::collections::HashSet;

use nebula_imap::proto::response::ListEntry;
use nebula_imap::proto::types::{MailboxAttribute, MailboxName};
use nebula_store::ListedMailbox;

use super::role::{RoleInput, assign, sort_order};

/// The personal namespace prefix (`INBOX.`), if it isn't empty, and the
/// prefixes of the other users' and shared namespaces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Namespace {
    /// The prefix including its delimiter, e.g. `INBOX.`.
    pub(crate) prefix: Option<String>,
    /// Prefixes of the namespaces that are not the user's own (`Other
    /// Users/`, `user.`, `Shared/`); never empty strings.
    pub(crate) others: Vec<String>,
}

impl Namespace {
    /// Whether a mailbox is in the user's own namespace: not under (nor
    /// the root of) another user's or a shared namespace. Someone else's
    /// folder never gets a role, so it can't become the Sent or Trash of
    /// this account.
    pub(crate) fn is_personal(&self, name: &str, delimiter: Option<char>) -> bool {
        !self.others.iter().filter(|p| !p.is_empty()).any(|prefix| {
            name.starts_with(prefix.as_str())
                || delimiter
                    .and_then(|d| prefix.strip_suffix(d))
                    .is_some_and(|root| name == root)
        })
    }

    /// The mailbox that is the namespace root (`INBOX` for `INBOX.`).
    fn root(&self, delimiter: Option<char>) -> Option<&str> {
        let prefix = self.prefix.as_deref()?;
        let delimiter = delimiter?;
        prefix.strip_suffix(delimiter)
    }
}

struct Placed {
    parent: Option<MailboxName>,
    name: String,
    top_level: bool,
}

fn place(entry: &ListEntry, names: &HashSet<&str>, namespace: &Namespace) -> Placed {
    let full = entry.name.as_str();
    let Some(delimiter) = entry.delimiter else {
        return Placed {
            parent: None,
            name: full.to_owned(),
            top_level: true,
        };
    };
    let root = namespace.root(Some(delimiter));
    let relative = match &namespace.prefix {
        Some(prefix) => full.strip_prefix(prefix.as_str()).unwrap_or(full),
        None => full,
    };
    // Walk up: "A/B/C" → "A/B" → "A".
    let mut candidate = full;
    while let Some((head, _)) = candidate.rsplit_once(delimiter) {
        candidate = head;
        if Some(candidate) == root {
            break;
        }
        if names.contains(candidate) {
            let rest = full
                .get(candidate.len().saturating_add(delimiter.len_utf8())..)
                .unwrap_or(full);
            return Placed {
                parent: MailboxName::new(candidate).ok(),
                name: rest.to_owned(),
                top_level: false,
            };
        }
    }
    Placed {
        parent: None,
        name: relative.to_owned(),
        top_level: !relative.contains(delimiter),
    }
}

/// The mailboxes to store, INBOX first, with roles and sort order.
/// `subscribed` holds the names of the subscribed mailboxes (from `RETURN
/// (SUBSCRIBED)` or LSUB).
pub(crate) fn build(
    entries: &[ListEntry],
    subscribed: &HashSet<String>,
    namespace: &Namespace,
) -> Vec<ListedMailbox> {
    let mut entries: Vec<&ListEntry> = entries
        .iter()
        .filter(|e| !e.attributes.contains(&MailboxAttribute::NonExistent))
        .collect();
    entries.sort_by_key(|e| !e.name.is_inbox());
    // A server listing a name twice: keep the first.
    let mut unique = HashSet::new();
    entries.retain(|e| unique.insert(e.name.as_str()));

    let names: HashSet<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    let placed: Vec<Placed> = entries
        .iter()
        .map(|e| place(e, &names, namespace))
        .collect();
    let inputs: Vec<RoleInput<'_>> = entries
        .iter()
        .zip(&placed)
        .map(|(entry, placed)| RoleInput {
            is_inbox: entry.name.is_inbox(),
            personal: namespace.is_personal(entry.name.as_str(), entry.delimiter),
            special_use: entry.special_use(),
            top_level_name: placed.top_level.then_some(placed.name.as_str()),
            selectable: entry.is_selectable(),
        })
        .collect();
    let roles = assign(&inputs);
    entries
        .iter()
        .zip(placed)
        .zip(roles)
        .map(|((entry, placed), role)| ListedMailbox {
            imap_name: entry.name.clone(),
            name: if entry.name.is_inbox() {
                "Inbox".to_owned()
            } else {
                placed.name
            },
            delimiter: entry.delimiter,
            parent: placed.parent,
            role,
            subscribed: subscribed.contains(entry.name.as_str()),
            selectable: entry.is_selectable(),
            no_inferiors: entry.attributes.contains(&MailboxAttribute::NoInferiors),
            sort_order: sort_order(role),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::types::SpecialUse;
    use nebula_store::Role;

    use super::*;

    fn entry(name: &str, attributes: Vec<MailboxAttribute>) -> ListEntry {
        ListEntry {
            attributes,
            delimiter: Some('/'),
            name: MailboxName::new(name).unwrap(),
        }
    }

    fn find<'a>(list: &'a [ListedMailbox], name: &str) -> &'a ListedMailbox {
        list.iter().find(|m| m.imap_name.as_str() == name).unwrap()
    }

    #[test]
    fn tree_with_gaps_and_roles() {
        let entries = vec![
            entry("Work/2026/Q1", vec![]),
            entry("Work", vec![]),
            entry("INBOX", vec![]),
            entry("Sent", vec![]),
            entry(
                "Basura",
                vec![MailboxAttribute::SpecialUse(SpecialUse::Trash)],
            ),
            entry("Gone", vec![MailboxAttribute::NonExistent]),
            entry("Shared", vec![MailboxAttribute::NoSelect]),
        ];
        let list = build(&entries, &HashSet::new(), &Namespace::default());
        assert_eq!(list.first().unwrap().imap_name.as_str(), "INBOX");
        assert_eq!(list.first().unwrap().name, "Inbox");
        assert_eq!(list.len(), 6);
        let q1 = find(&list, "Work/2026/Q1");
        assert_eq!(q1.parent.as_ref().map(MailboxName::as_str), Some("Work"));
        assert_eq!(q1.name, "2026/Q1");
        assert_eq!(find(&list, "Sent").role, Some(Role::Sent));
        assert_eq!(find(&list, "Basura").role, Some(Role::Trash));
        assert!(!find(&list, "Shared").selectable);
        assert!(!find(&list, "Shared").no_inferiors);
        assert_eq!(find(&list, "INBOX").role, Some(Role::Inbox));
        assert!(find(&list, "INBOX").sort_order < find(&list, "Work").sort_order);
        assert!(!find(&list, "Work").subscribed);
        let subscribed = HashSet::from(["Work".to_owned()]);
        let list = build(&entries, &subscribed, &Namespace::default());
        assert!(find(&list, "Work").subscribed);
        assert!(!find(&list, "Sent").subscribed);
    }

    #[test]
    fn inbox_namespace_prefix() {
        let dot = |name: &str| ListEntry {
            attributes: vec![],
            delimiter: Some('.'),
            name: MailboxName::new(name).unwrap(),
        };
        let entries = vec![
            dot("INBOX"),
            dot("INBOX.Sent"),
            dot("INBOX.Drafts"),
            dot("INBOX.Projects"),
            dot("INBOX.Projects.Nebula"),
        ];
        let namespace = Namespace {
            prefix: Some("INBOX.".to_owned()),
            others: Vec::new(),
        };
        let list = build(&entries, &HashSet::new(), &namespace);
        let sent = find(&list, "INBOX.Sent");
        assert_eq!(sent.parent, None);
        assert_eq!(sent.name, "Sent");
        assert_eq!(sent.role, Some(Role::Sent));
        assert_eq!(find(&list, "INBOX.Drafts").role, Some(Role::Drafts));
        let nebula = find(&list, "INBOX.Projects.Nebula");
        assert_eq!(
            nebula.parent.as_ref().map(MailboxName::as_str),
            Some("INBOX.Projects")
        );
        assert_eq!(nebula.name, "Nebula");
    }

    #[test]
    fn other_namespaces_get_no_role() {
        let drafts = MailboxAttribute::SpecialUse(SpecialUse::Drafts);
        let entries = vec![
            entry("INBOX", vec![]),
            entry("Other Users", vec![MailboxAttribute::NoSelect]),
            entry("Other Users/bob/Drafts", vec![drafts.clone()]),
            entry(
                "Shared/Sent",
                vec![MailboxAttribute::SpecialUse(SpecialUse::Sent)],
            ),
            entry("Drafts", vec![]),
            entry("Sent", vec![]),
        ];
        let namespace = Namespace {
            prefix: None,
            others: vec!["Other Users/".to_owned(), "Shared/".to_owned()],
        };
        let list = build(&entries, &HashSet::new(), &namespace);
        assert_eq!(find(&list, "Other Users/bob/Drafts").role, None);
        assert_eq!(find(&list, "Shared/Sent").role, None);
        assert_eq!(find(&list, "Drafts").role, Some(Role::Drafts));
        assert_eq!(find(&list, "Sent").role, Some(Role::Sent));

        // Cyrus: `user.` for other users and an empty shared prefix, which
        // must not make every mailbox count as someone else's.
        let dot = |name: &str, attributes| ListEntry {
            attributes,
            delimiter: Some('.'),
            name: MailboxName::new(name).unwrap(),
        };
        let entries = vec![
            dot("INBOX", vec![]),
            dot("INBOX.Drafts", vec![drafts]),
            dot("user", vec![MailboxAttribute::NoSelect]),
            dot(
                "user.bob.Trash",
                vec![MailboxAttribute::SpecialUse(SpecialUse::Trash)],
            ),
            dot("INBOX.Trash", vec![]),
        ];
        let namespace = Namespace {
            prefix: Some("INBOX.".to_owned()),
            others: vec!["user.".to_owned(), String::new()],
        };
        assert!(!namespace.is_personal("user", Some('.')));
        let list = build(&entries, &HashSet::new(), &namespace);
        assert_eq!(find(&list, "INBOX").role, Some(Role::Inbox));
        assert_eq!(find(&list, "INBOX.Drafts").role, Some(Role::Drafts));
        assert_eq!(find(&list, "user.bob.Trash").role, None);
        assert_eq!(find(&list, "INBOX.Trash").role, Some(Role::Trash));
    }

    #[test]
    fn real_inbox_children_stay_below_it() {
        let entries = vec![entry("INBOX", vec![]), entry("INBOX/Sent", vec![])];
        let list = build(&entries, &HashSet::new(), &Namespace::default());
        let child = find(&list, "INBOX/Sent");
        assert_eq!(
            child.parent.as_ref().map(MailboxName::as_str),
            Some("INBOX")
        );
        // Not top level: no role from its name.
        assert_eq!(child.role, None);
    }

    /// RFC 9051 §7.3.1: `\Noinferiors` means no child can exist below it.
    #[test]
    fn no_inferiors_is_kept() {
        let entries = vec![
            entry("INBOX", vec![]),
            entry("Leaf", vec![MailboxAttribute::NoInferiors]),
        ];
        let list = build(&entries, &HashSet::new(), &Namespace::default());
        assert!(find(&list, "Leaf").no_inferiors);
        assert!(find(&list, "Leaf").selectable);
        assert!(!find(&list, "INBOX").no_inferiors);
    }

    #[test]
    fn flat_namespace() {
        let entries = vec![ListEntry {
            attributes: vec![],
            delimiter: None,
            name: MailboxName::new("a/b").unwrap(),
        }];
        let list = build(&entries, &HashSet::new(), &Namespace::default());
        assert_eq!(list.first().unwrap().parent, None);
        assert_eq!(list.first().unwrap().name, "a/b");
    }
}
