// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Quotas of the account: `GETQUOTAROOT INBOX` (RFC 9208, RFC 2087).

use nebula_imap::proto::response::{Quota, QuotaResourceName};
use nebula_imap::proto::types::{Capability, MailboxName};

use crate::MailError;
use crate::core::account::MailAccount;

/// What a quota counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum QuotaResource {
    /// Bytes (IMAP `STORAGE`, which counts KiB: converted here).
    Storage,
    /// Number of messages (`MESSAGE`).
    Messages,
    /// Number of mailboxes (`MAILBOX`, RFC 9208).
    Mailboxes,
}

/// One limited resource of one quota root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaUsage {
    /// The IMAP quota root (any text, may be empty: Dovecot says
    /// `User quota`, others `""`…).
    pub root: String,
    /// What it counts.
    pub resource: QuotaResource,
    /// Current usage (bytes for [`QuotaResource::Storage`]).
    pub used: u64,
    /// The limit, same unit.
    pub limit: u64,
}

impl MailAccount {
    /// The quotas that apply to the INBOX, or `None` if the server has no
    /// QUOTA extension. Other resources (`ANNOTATION-STORAGE`…) are left
    /// out: JMAP has no type for them.
    ///
    /// # Errors
    /// Connection errors and a NO/BAD answer.
    pub async fn quotas(&self) -> Result<Option<Vec<QuotaUsage>>, MailError> {
        let mut slot = self.slot().await;
        let session = slot.session(self).await?;
        let supported = session.capabilities().has(&Capability::Quota);
        self.set_supports_quota(supported);
        if !supported {
            return Ok(None);
        }
        let result = session.quota_root(&MailboxName::inbox()).await;
        let info = slot.check(result)?;
        drop(slot);
        Ok(Some(usages(&info.roots, info.quotas)))
    }
}

fn usages(roots: &[String], quotas: Vec<Quota>) -> Vec<QuotaUsage> {
    let mut out: Vec<QuotaUsage> = Vec::new();
    for quota in quotas {
        // Only the roots of the INBOX (a server may send more).
        if !roots.is_empty() && !roots.contains(&quota.root) {
            continue;
        }
        for resource in quota.resources {
            let (kind, factor) = match resource.name {
                QuotaResourceName::Storage => (QuotaResource::Storage, 1024),
                QuotaResourceName::Message => (QuotaResource::Messages, 1),
                QuotaResourceName::Other(name) if name == "MAILBOX" => {
                    (QuotaResource::Mailboxes, 1)
                }
                _ => continue,
            };
            let usage = QuotaUsage {
                root: quota.root.clone(),
                resource: kind,
                used: resource.usage.saturating_mul(factor),
                limit: resource.limit.saturating_mul(factor),
            };
            // The same root and resource twice: keep the first answer.
            if !out
                .iter()
                .any(|u| u.root == usage.root && u.resource == usage.resource)
            {
                out.push(usage);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use nebula_imap::proto::response::QuotaResource as ImapResource;

    use super::*;

    fn resource(name: QuotaResourceName, usage: u64, limit: u64) -> ImapResource {
        ImapResource { name, usage, limit }
    }

    #[test]
    fn converts_units_and_skips_unknown() {
        let quotas = vec![
            Quota {
                root: "User quota".to_owned(),
                resources: vec![
                    resource(QuotaResourceName::Storage, 10, 1024),
                    resource(QuotaResourceName::Message, 3, 100),
                    resource(QuotaResourceName::Other("ANNOTATION-STORAGE".into()), 1, 2),
                    resource(QuotaResourceName::Other("MAILBOX".into()), 4, 50),
                ],
            },
            Quota {
                root: "other".to_owned(),
                resources: vec![resource(QuotaResourceName::Storage, 1, 2)],
            },
        ];
        let out = usages(&["User quota".to_owned()], quotas);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].resource, QuotaResource::Storage);
        assert_eq!((out[0].used, out[0].limit), (10 * 1024, 1024 * 1024));
        assert_eq!(out[1].resource, QuotaResource::Messages);
        assert_eq!(out[2].resource, QuotaResource::Mailboxes);
        // Overflow saturates instead of wrapping.
        let huge = usages(
            &[],
            vec![Quota {
                root: String::new(),
                resources: vec![resource(QuotaResourceName::Storage, u64::MAX, u64::MAX)],
            }],
        );
        assert_eq!(huge[0].used, u64::MAX);
    }
}
