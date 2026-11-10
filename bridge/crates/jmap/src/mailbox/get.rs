// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `Mailbox/get`: names, hierarchy and roles from the store, counts from
//! the synced emails.

use nebula_jmap_proto::method::{GetArguments, GetResponse};
use nebula_jmap_proto::object::{Mailbox, MailboxProperty};
use nebula_jmap_proto::{MailboxId as JmapMailboxId, MethodResponse, UnsignedInt};
use nebula_store::{DataType, Mailbox as StoredMailbox, MailboxCounts};

use super::{jmap_role, rights};
use crate::core::context::Ctx;
use crate::core::ids;
use crate::error::{Bug, CallError};

/// The JMAP form of a mailbox, with the properties asked for.
pub(crate) fn to_jmap(
    mailbox: &StoredMailbox,
    counts: MailboxCounts,
    properties: Option<&[MailboxProperty]>,
) -> Result<Mailbox, Bug> {
    let wants = |p: MailboxProperty| properties.is_none_or(|list| list.contains(&p));
    let count = UnsignedInt::saturating;
    let mut out = Mailbox {
        id: Some(ids::mailbox_id(mailbox.id)?),
        ..Mailbox::default()
    };
    if wants(MailboxProperty::Name) {
        out.name = Some(mailbox.name.clone());
    }
    if wants(MailboxProperty::ParentId) {
        out.parent_id = Some(mailbox.parent.map(ids::mailbox_id).transpose()?);
    }
    if wants(MailboxProperty::Role) {
        out.role = Some(mailbox.role.and_then(jmap_role));
    }
    if wants(MailboxProperty::SortOrder) {
        out.sort_order = Some(UnsignedInt::from(mailbox.sort_order));
    }
    if wants(MailboxProperty::TotalEmails) {
        out.total_emails = Some(count(counts.total_emails));
    }
    if wants(MailboxProperty::UnreadEmails) {
        out.unread_emails = Some(count(counts.unread_emails));
    }
    if wants(MailboxProperty::TotalThreads) {
        out.total_threads = Some(count(counts.total_threads));
    }
    if wants(MailboxProperty::UnreadThreads) {
        out.unread_threads = Some(count(counts.unread_threads));
    }
    if wants(MailboxProperty::MyRights) {
        out.my_rights = Some(rights(mailbox));
    }
    if wants(MailboxProperty::IsSubscribed) {
        out.is_subscribed = Some(mailbox.subscribed);
    }
    // Only when asked for: no JMAP client needs it (the web app's filters
    // do).
    if properties.is_some_and(|list| list.contains(&MailboxProperty::ImapName)) {
        out.imap_name = Some(mailbox.imap_name.as_str().to_owned());
    }
    Ok(out)
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<JmapMailboxId, MailboxProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let state = ctx.state(DataType::Mailbox).await?;
    let mailboxes = ctx.store().mailboxes(ctx.id()).await?;
    let counts = ctx.store().mailbox_counts(ctx.id()).await?;
    let properties = args.properties.as_deref();
    let counts_of = |m: &StoredMailbox| counts.get(&m.id).copied().unwrap_or_default();
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &args.ids {
        None => {
            for mailbox in &mailboxes {
                list.push(to_jmap(mailbox, counts_of(mailbox), properties)?);
            }
        }
        Some(wanted) => {
            for wanted in wanted {
                let Some(id) = ctx.resolve(wanted) else {
                    continue;
                };
                let found =
                    ids::store_mailbox(&id).and_then(|row| mailboxes.iter().find(|m| m.id == row));
                match found {
                    Some(mailbox) => list.push(to_jmap(mailbox, counts_of(mailbox), properties)?),
                    None => not_found.push(id),
                }
            }
        }
    }
    Ok(MethodResponse::MailboxGet(GetResponse {
        account_id: ctx.jmap_id(),
        state,
        list,
        not_found,
    }))
}
