// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Mailbox roles: from SPECIAL-USE (RFC 6154) first, then from well-known
//! names for the roles no mailbox claimed.
//!
//! Name heuristics are careful on purpose (a wrong Trash is worse than none):
//! only selectable mailboxes at the top level of the personal namespace,
//! whole names compared without case (`Sent Items` yes, `Sent 2019` no),
//! and only for a role no SPECIAL-USE mailbox already has. The first match
//! in LIST order wins. Mailboxes of other users' or shared namespaces get
//! no role at all, not even from SPECIAL-USE (only INBOX is always one).

use nebula_imap::proto::types::SpecialUse;
use nebula_store::Role;

/// The role a SPECIAL-USE attribute gives.
pub(crate) fn from_special_use(special: SpecialUse) -> Option<Role> {
    Some(match special {
        SpecialUse::All => Role::All,
        SpecialUse::Archive => Role::Archive,
        SpecialUse::Drafts => Role::Drafts,
        SpecialUse::Flagged => Role::Flagged,
        SpecialUse::Important => Role::Important,
        SpecialUse::Junk => Role::Junk,
        SpecialUse::Sent => Role::Sent,
        SpecialUse::Trash => Role::Trash,
        _ => return None,
    })
}

/// Well-known names (lowercase), in English, Spanish, German, French,
/// Italian, Portuguese and Dutch, plus the usual Outlook/Gmail spellings.
const NAMES: &[(Role, &[&str])] = &[
    (
        Role::Sent,
        &[
            "sent",
            "sent items",
            "sent messages",
            "sent mail",
            "enviados",
            "elementos enviados",
            "correo enviado",
            "gesendet",
            "gesendete objekte",
            "gesendete elemente",
            "envoyés",
            "messages envoyés",
            "éléments envoyés",
            "inviata",
            "posta inviata",
            "posta in uscita",
            "itens enviados",
            "verzonden",
            "verzonden items",
        ],
    ),
    (
        Role::Drafts,
        &[
            "drafts",
            "draft",
            "borradores",
            "entwürfe",
            "brouillons",
            "bozze",
            "rascunhos",
            "concepten",
        ],
    ),
    (
        Role::Trash,
        &[
            "trash",
            "deleted items",
            "deleted messages",
            "deleted",
            "bin",
            "papelera",
            "elementos eliminados",
            "papierkorb",
            "gelöschte elemente",
            "gelöschte objekte",
            "corbeille",
            "éléments supprimés",
            "cestino",
            "posta eliminata",
            "lixeira",
            "itens excluídos",
            "prullenbak",
            "verwijderde items",
        ],
    ),
    (
        Role::Junk,
        &[
            "junk",
            "spam",
            "junk e-mail",
            "junk email",
            "junk mail",
            "bulk mail",
            "correo no deseado",
            "no deseado",
            "spam-verdacht",
            "courrier indésirable",
            "indésirables",
            "posta indesiderata",
            "lixo eletrônico",
            "ongewenste e-mail",
        ],
    ),
    (
        Role::Archive,
        &[
            "archive", "archives", "archivo", "archiv", "archiver", "archivio", "arquivo",
        ],
    ),
];

/// The role a mailbox name suggests, if it's a well-known one.
pub(crate) fn from_name(leaf: &str) -> Option<Role> {
    let lower = leaf.trim().to_lowercase();
    NAMES
        .iter()
        .find(|(_, names)| names.contains(&lower.as_str()))
        .map(|(role, _)| *role)
}

/// What [`assign`] needs to know about one mailbox.
#[derive(Debug, Clone)]
pub(crate) struct RoleInput<'a> {
    /// `INBOX`.
    pub(crate) is_inbox: bool,
    /// In the user's own namespace (not another user's or a shared one).
    pub(crate) personal: bool,
    /// The SPECIAL-USE attribute, if any.
    pub(crate) special_use: Option<SpecialUse>,
    /// The display name, if the mailbox is at the top level (else `None`).
    pub(crate) top_level_name: Option<&'a str>,
    /// Not `\Noselect`.
    pub(crate) selectable: bool,
}

/// Roles for a list of mailboxes, at most one mailbox per role.
pub(crate) fn assign(mailboxes: &[RoleInput<'_>]) -> Vec<Option<Role>> {
    let mut roles: Vec<Option<Role>> = mailboxes
        .iter()
        .map(|m| {
            if m.is_inbox {
                Some(Role::Inbox)
            } else if !m.personal {
                // Another user's or a shared folder: never this account's.
                None
            } else {
                m.special_use.and_then(from_special_use)
            }
        })
        .collect();
    // One mailbox per role: the first that claims it.
    let mut taken = Vec::new();
    for role in &mut roles {
        if let Some(found) = *role {
            if taken.contains(&found) {
                *role = None;
            } else {
                taken.push(found);
            }
        }
    }
    for (index, mailbox) in mailboxes.iter().enumerate() {
        let Some(slot) = roles.get_mut(index) else {
            continue;
        };
        if slot.is_some() || !mailbox.selectable || !mailbox.personal {
            continue;
        }
        let guess = mailbox.top_level_name.and_then(from_name);
        if let Some(role) = guess
            && !taken.contains(&role)
        {
            taken.push(role);
            *slot = Some(role);
        }
    }
    roles
}

/// Mailboxes with a role come first, in this order; then the rest
/// (sorted by name by the client).
pub(crate) fn sort_order(role: Option<Role>) -> u32 {
    match role {
        Some(Role::Inbox) => 1,
        Some(Role::Drafts) => 2,
        Some(Role::Sent) => 3,
        Some(Role::Archive) => 4,
        Some(Role::Junk) => 5,
        Some(Role::Trash) => 6,
        Some(Role::All) => 7,
        Some(Role::Flagged | Role::Important) => 8,
        _ => 100,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, special: Option<SpecialUse>) -> RoleInput<'_> {
        RoleInput {
            is_inbox: name.eq_ignore_ascii_case("inbox"),
            personal: true,
            special_use: special,
            top_level_name: Some(name),
            selectable: true,
        }
    }

    #[test]
    fn names() {
        assert_eq!(from_name("Sent Items"), Some(Role::Sent));
        assert_eq!(from_name("PAPELERA"), Some(Role::Trash));
        assert_eq!(from_name("Entwürfe"), Some(Role::Drafts));
        assert_eq!(from_name("Spam"), Some(Role::Junk));
        assert_eq!(from_name("Sent 2019"), None);
        assert_eq!(from_name("Trashy novels"), None);
    }

    #[test]
    fn special_use_wins_over_names() {
        let list = [
            input("INBOX", None),
            input("Sent", None),
            input("Enviados", Some(SpecialUse::Sent)),
            input("Trash", None),
            input("Deleted Items", None),
        ];
        assert_eq!(
            assign(&list),
            vec![
                Some(Role::Inbox),
                None,
                Some(Role::Sent),
                Some(Role::Trash),
                None
            ]
        );
    }

    #[test]
    fn duplicates_and_nested_names() {
        let mut nested = input("Trash", None);
        nested.top_level_name = None;
        let mut noselect = input("Drafts", None);
        noselect.selectable = false;
        let list = [
            input("A", Some(SpecialUse::Junk)),
            input("B", Some(SpecialUse::Junk)),
            nested,
            noselect,
        ];
        assert_eq!(assign(&list), vec![Some(Role::Junk), None, None, None]);
    }

    #[test]
    fn other_namespaces_get_no_role() {
        let mut theirs = input("Other Users/bob/Sent", Some(SpecialUse::Sent));
        theirs.personal = false;
        let mut their_trash = input("Trash", None);
        their_trash.personal = false;
        let list = [
            theirs,
            their_trash,
            input("Sent", None),
            input("Papelera", None),
        ];
        assert_eq!(
            assign(&list),
            vec![None, None, Some(Role::Sent), Some(Role::Trash)]
        );
    }

    #[test]
    fn order() {
        assert!(sort_order(Some(Role::Inbox)) < sort_order(Some(Role::Trash)));
        assert!(sort_order(Some(Role::Trash)) < sort_order(None));
    }
}
