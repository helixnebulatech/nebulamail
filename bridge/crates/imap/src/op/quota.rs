// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! GETQUOTAROOT and NAMESPACE.

use nebula_imap_proto::command::{GetQuotaRoot, Namespace};
use nebula_imap_proto::response::{Namespaces, Quota, Untagged};
use nebula_imap_proto::types::{Capability, MailboxName};

use crate::Error;
use crate::core::client::{Client, LoggedIn};
use crate::core::connection::{StrayContinuation, expect_ok};

/// Quota roots of a mailbox and their usage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct QuotaInfo {
    /// Names of the quota roots.
    pub roots: Vec<String>,
    /// Usage and limits per root.
    pub quotas: Vec<Quota>,
}

impl<S: LoggedIn> Client<S> {
    /// `GETQUOTAROOT` (needs QUOTA).
    ///
    /// # Errors
    ///
    /// Missing QUOTA, NO/BAD or connection errors.
    pub async fn get_quota_root(&mut self, mailbox: &MailboxName) -> Result<QuotaInfo, Error> {
        if !self.session.has(&Capability::Quota) {
            return Err(Error::Unsupported("QUOTA"));
        }
        let mut info = QuotaInfo::default();
        let tagged = self
            .run_with(
                &GetQuotaRoot { mailbox },
                StrayContinuation::Reject,
                |data| match data {
                    Untagged::QuotaRoot(root) => {
                        info.roots.extend(root.roots);
                        None
                    }
                    Untagged::Quota(quota) => {
                        info.quotas.push(quota);
                        None
                    }
                    other => Some(other),
                },
            )
            .await?;
        expect_ok("GETQUOTAROOT", tagged)?;
        Ok(info)
    }

    /// `NAMESPACE` (needs NAMESPACE): personal, other users' and shared
    /// prefixes.
    ///
    /// # Errors
    ///
    /// Missing NAMESPACE, NO/BAD or connection errors.
    pub async fn namespace(&mut self) -> Result<Namespaces, Error> {
        if !self.session.has(&Capability::Namespace) {
            return Err(Error::Unsupported("NAMESPACE"));
        }
        let mut found = None;
        let tagged = self
            .run_with(&Namespace, StrayContinuation::Reject, |data| match data {
                Untagged::Namespace(namespaces) => {
                    found = Some(namespaces);
                    None
                }
                other => Some(other),
            })
            .await?;
        expect_ok("NAMESPACE", tagged)?;
        found.ok_or(Error::Protocol("NAMESPACE without data"))
    }
}
