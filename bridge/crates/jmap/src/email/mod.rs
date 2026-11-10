// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Email (RFC 8621 §4): metadata from the store, bodies by fetching and
//! parsing the raw message, changes through the mail layer.

pub(crate) mod body;
pub(crate) mod build;
pub(crate) mod changes;
pub(crate) mod copy;
pub(crate) mod encapsulate;
pub(crate) mod get;
pub(crate) mod headers;
pub(crate) mod import;
pub(crate) mod parse;
pub(crate) mod query;
pub(crate) mod set;
pub(crate) mod snippet;

use std::collections::{BTreeMap, BTreeSet};

use nebula_jmap_proto::{IdOrRef, MailboxId as JmapMailboxId, SetError};
use nebula_store::MailboxId;

use crate::core::context::Ctx;
use crate::core::ids;

/// Mailbox ids (maybe `#creationId`s) as store rows. `Err` with the first
/// one that is unknown.
pub(crate) fn mailbox_rows(
    ctx: &Ctx<'_>,
    list: &BTreeSet<IdOrRef<JmapMailboxId>>,
) -> Result<BTreeSet<MailboxId>, String> {
    list.iter()
        .map(|id| {
            ctx.resolve(id)
                .as_ref()
                .and_then(ids::store_mailbox)
                .ok_or_else(|| id.to_string())
        })
        .collect()
}

fn mailbox_error(why: String) -> SetError {
    SetError::invalid_properties(["mailboxIds"]).with_description(why)
}

/// Whether every mailbox in `rows` is in `known` (row → selectable) and
/// can hold emails (the same rule as `MailAccount::append` and
/// `set_mailboxes`).
fn validate(rows: &BTreeSet<MailboxId>, known: &BTreeMap<MailboxId, bool>) -> Result<(), SetError> {
    if rows.is_empty() {
        return Err(mailbox_error("at least one mailbox is required".to_owned()));
    }
    for row in rows {
        let name = ids::mailbox_id(*row).map_or_else(|_| String::new(), |id| id.to_string());
        match known.get(row) {
            None => return Err(mailbox_error(format!("unknown mailbox {name}"))),
            Some(false) => return Err(mailbox_error(format!("mailbox {name} cannot hold emails"))),
            Some(true) => {}
        }
    }
    Ok(())
}

/// The mailboxes of a create or import as store rows, all checked with
/// one read before anything is appended: an email is created whole or
/// not at all (RFC 8621 §4.6).
pub(crate) async fn checked_mailboxes(
    ctx: &Ctx<'_>,
    list: &BTreeSet<IdOrRef<JmapMailboxId>>,
) -> Result<BTreeSet<MailboxId>, SetError> {
    let rows =
        mailbox_rows(ctx, list).map_err(|id| mailbox_error(format!("unknown mailbox {id}")))?;
    let mailboxes = ctx
        .store()
        .mailboxes(ctx.id())
        .await
        .map_err(|e| crate::error::store_set_error(&e))?;
    let known = mailboxes.iter().map(|m| (m.id, m.selectable)).collect();
    validate(&rows, &known)?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::SetErrorType;

    use super::*;

    #[test]
    fn every_mailbox_is_checked() {
        let all: BTreeMap<MailboxId, bool> = [(1, true), (2, true), (3, false)]
            .into_iter()
            .filter_map(|(id, selectable)| Some((MailboxId::from_raw(id)?, selectable)))
            .collect();
        let rows = |list: &[i64]| -> BTreeSet<MailboxId> {
            list.iter()
                .filter_map(|n| MailboxId::from_raw(*n))
                .collect()
        };
        assert!(validate(&rows(&[1, 2]), &all).is_ok());
        let unknown = validate(&rows(&[1, 99]), &all).unwrap_err();
        assert_eq!(unknown.kind(), SetErrorType::InvalidProperties);
        assert!(
            unknown
                .description()
                .unwrap_or_default()
                .contains("unknown")
        );
        let folder = validate(&rows(&[1, 3]), &all).unwrap_err();
        assert!(
            folder
                .description()
                .unwrap_or_default()
                .contains("cannot hold")
        );
        assert!(validate(&rows(&[]), &all).is_err());
    }
}
