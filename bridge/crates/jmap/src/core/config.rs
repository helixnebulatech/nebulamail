// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Settings of the JMAP backend, built by the binary from the TOML
//! configuration.

use std::time::Duration;

use nebula_smtp::SmtpClient;
use net_guard::{Policy, Target};

/// Who saves a copy of sent mail in the Sent mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SentCopy {
    /// The bridge, with IMAP `APPEND` (most servers).
    #[default]
    Append,
    /// The server does it by itself (Gmail): appending would save it twice.
    Server,
}

/// The SMTP submission server: where to connect (through `net-guard`) and
/// how to talk to it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SmtpSettings {
    /// The SMTP client (TLS settings, `EHLO` name, timeouts).
    pub client: SmtpClient,
    /// Host and port.
    pub target: Target,
    /// Allowed ports and networks.
    pub policy: Policy,
}

impl SmtpSettings {
    /// The settings.
    #[must_use]
    pub fn new(client: SmtpClient, target: Target, policy: Policy) -> Self {
        Self {
            client,
            target,
            policy,
        }
    }
}

/// Held submissions (RFC 4865 FUTURERELEASE): undo send
/// (`HOLDFOR`) and scheduled send (`HOLDUNTIL`). The bridge keeps the
/// message, sealed with the account's data key, and sends it when it is
/// due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct HoldSettings {
    /// The longest hold (`maxDelayedSend` and the FUTURERELEASE interval).
    /// Zero: nothing is held and FUTURERELEASE is not announced.
    pub max_delay: Duration,
    /// Most held messages per account at once.
    pub max_count: u64,
    /// Most bytes of held messages per account at once.
    pub max_bytes: u64,
    /// How long a due message may wait for its account to be signed in, or
    /// for the SMTP server to take it, before it fails (shown as not
    /// delivered, never dropped silently).
    pub max_wait: Duration,
    /// Whether a held message may be marked to go out even after the user
    /// signs out (`EmailSubmission`'s vendor property `sendWhenSignedOut`):
    /// the HTTP layer then keeps the account running, and its password
    /// sealed, until the message is settled or `max_wait` past its time.
    pub send_while_signed_out: bool,
}

impl HoldSettings {
    /// The defaults: up to 7 days ahead, 100 messages and 200 MiB per
    /// account, 3 days of waiting once due, and nothing sent after the
    /// user signs out.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_delay: Duration::from_hours(7 * 24),
            max_count: 100,
            max_bytes: 200 * 1024 * 1024,
            max_wait: Duration::from_hours(3 * 24),
            send_while_signed_out: false,
        }
    }

    /// Nothing is held: messages go out at once.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            max_delay: Duration::ZERO,
            ..Self::new()
        }
    }

    /// Whether messages can be held at all.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        !self.max_delay.is_zero()
    }

    /// Whether held messages may go out after the user signs out (only if
    /// messages can be held at all).
    #[must_use]
    pub const fn sends_while_signed_out(&self) -> bool {
        self.enabled() && self.send_while_signed_out
    }
}

impl Default for HoldSettings {
    fn default() -> Self {
        Self::new()
    }
}

/// Settings of the JMAP backend.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct JmapConfig {
    /// The SMTP server.
    pub smtp: SmtpSettings,
    /// Who saves sent mail.
    pub sent_copy: SentCopy,
    /// Domain added to logins without `@` to make the account's address
    /// (`alice` → `alice@example.com`). `None`: the login is the address.
    pub email_domain: Option<String>,
    /// How long an uploaded blob is kept.
    pub upload_lifetime: Duration,
    /// Most bytes of unexpired uploads per account.
    pub upload_quota: u64,
    /// How long a sent submission stays visible to `EmailSubmission/get`.
    pub submission_lifetime: Duration,
    /// Undo send and scheduled send.
    pub hold: HoldSettings,
    /// `maxSizeAttachmentsPerEmail` of the mail capability.
    pub max_size_attachments: u64,
    /// Most bytes of messages one `Email/import` call holds in memory (each
    /// blob counted once); the imports past it fail with `tooLarge`.
    pub max_import_bytes: u64,
    /// How often expired uploads and submissions are deleted.
    pub cleanup_interval: Duration,
    /// Web Push (`[push]`); `None`: no `PushSubscription` and no
    /// `webpush-vapid` capability.
    pub web_push: Option<crate::push::WebPushSettings>,
    /// Sieve scripts and the vacation response over ManageSieve
    ///if `[upstream.sieve]` is set.
    pub sieve: Option<crate::sieve::SieveSettings>,
}

impl JmapConfig {
    /// The defaults: uploads kept 24 h with 200 MB per account,
    /// submissions kept 24 h, messages held up to 7 days
    /// ([`HoldSettings::new`]), 50 MB of attachments per email, 100 MB per
    /// `Email/import`, cleanup every 10 minutes, Sent copies by `APPEND`.
    #[must_use]
    pub fn new(smtp: SmtpSettings) -> Self {
        Self {
            smtp,
            sent_copy: SentCopy::Append,
            email_domain: None,
            upload_lifetime: Duration::from_hours(24),
            upload_quota: 200 * 1024 * 1024,
            submission_lifetime: Duration::from_hours(24),
            hold: HoldSettings::new(),
            max_size_attachments: 50 * 1024 * 1024,
            max_import_bytes: 100 * 1024 * 1024,
            cleanup_interval: Duration::from_mins(10),
            web_push: None,
            sieve: None,
        }
    }

    /// The address of a login: the login itself if it has an `@`, else
    /// `login@email_domain` when a domain is set.
    #[must_use]
    pub fn address_of(&self, login: &str) -> String {
        match &self.email_domain {
            Some(domain) if !login.contains('@') => format!("{login}@{domain}"),
            _ => login.to_owned(),
        }
    }
}
