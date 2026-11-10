// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! [`JmapBackend`]: the `nebula_http::Backend` on top of `nebula-mail`
//! (IMAP), `nebula-smtp` and, if configured, `nebula-groupware` (DAV).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use futures::stream::BoxStream;
use nebula_crypto::AccountKey;
use nebula_groupware::Groupware;
use nebula_http::{
    AccountContext, ApiCall, Backend, BackendError, Blob, LoginAlert, LoginName, LoginOutcome,
    MediaType, PushEvent, SharedAccount, StartReason, UploadBody, UploadError, UploadedBlob,
};
use nebula_jmap_proto::session::{
    AccountCapabilities, MailAccountCapability, SubmissionAccountCapability,
};
use nebula_jmap_proto::{BlobId, Response, UnsignedInt};
use nebula_mail::{Mail, MailError, Start};
use nebula_smtp::ServerExtensions;
use nebula_store::{AccountId, Store, UnixTime};
use net_guard::Resolver;
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use super::config::JmapConfig;
use super::{dispatch, events};
use crate::blob;
use crate::email::query::SORT_OPTIONS;
use crate::quota::QuotaHistory;

/// The login and password of a running account, for SMTP `AUTH`.
///
/// The IMAP layer keeps the same password for the whole session (sync and
/// IDLE need it), so this adds no exposure: it lives exactly as long, in a
/// `SecretString` that is wiped on drop, and is never stored in clear.
pub(crate) struct SmtpLogin {
    pub(crate) login: String,
    pub(crate) password: SecretString,
    /// The data key, which seals and opens its held messages
    /// (FUTURERELEASE): a copy of the one the groupware layer keeps as long.
    pub(crate) key: AccountKey,
}

/// What every request shares.
pub(crate) struct Inner {
    pub(crate) store: Store,
    pub(crate) mail: Mail,
    /// Calendars and contacts over CalDAV/CardDAV, if `[upstream.dav]` is
    /// configured.
    pub(crate) groupware: Option<Groupware>,
    pub(crate) resolver: Arc<dyn Resolver>,
    pub(crate) config: JmapConfig,
    /// Web Push, if configured.
    pub(crate) push: Option<crate::push::Service>,
    /// ManageSieve, if configured.
    pub(crate) sieve: Option<Arc<crate::sieve::Service>>,
    logins: Mutex<HashMap<AccountId, Arc<SmtpLogin>>>,
    /// Recent quota lists, for `Quota/changes`.
    pub(crate) quotas: QuotaHistory,
    /// The SMTP server's extensions, from the last connection.
    smtp_extensions: Mutex<Option<ServerExtensions>>,
    /// Wakes the scheduler of held messages (a new one, or an account that
    /// started).
    pub(crate) held_wake: tokio::sync::Notify,
}

impl Inner {
    /// The SMTP login of a running account.
    pub(crate) fn smtp_login(&self, account: AccountId) -> Option<Arc<SmtpLogin>> {
        self.logins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&account)
            .cloned()
    }

    pub(crate) fn set_smtp_extensions(&self, extensions: ServerExtensions) {
        *self
            .smtp_extensions
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(extensions);
    }

    /// `submissionExtensions`: what clients may use through `envelope`
    /// parameters. DSN (RFC 3461; `NOTIFY`, `ORCPT`, `RET`, `ENVID`) only
    /// if the server announced it; FUTURERELEASE (RFC 4865; `HOLDFOR`,
    /// `HOLDUNTIL`) when holds are on, which the bridge does itself, with
    /// the longest hold in seconds (no fixed latest date: it moves).
    fn submission_extensions(&self) -> BTreeMap<String, Vec<String>> {
        let guard = self
            .smtp_extensions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut out = BTreeMap::new();
        if guard.as_ref().is_some_and(|e| e.has("DSN")) {
            out.insert("DSN".to_owned(), Vec::new());
        }
        drop(guard);
        let hold = &self.config.hold;
        if hold.enabled() {
            out.insert(
                "FUTURERELEASE".to_owned(),
                vec![hold.max_delay.as_secs().to_string()],
            );
        }
        out
    }

    fn set_login(&self, account: AccountId, login: Option<SmtpLogin>) {
        let mut logins = self.logins.lock().unwrap_or_else(PoisonError::into_inner);
        match login {
            Some(login) => logins.insert(account, Arc::new(login)),
            None => logins.remove(&account),
        };
    }

    /// The server refused the stored password: end every session of the
    /// account (its tokens stop working at once, and pending login codes
    /// too), forget the password and stop the account as after its last
    /// session. The HTTP layer's periodic cleanup then forgets it as
    /// started.
    async fn end_sessions(&self, account: AccountId) {
        tracing::warn!(
            account = account.get(),
            "the mail server refused the password: ending the account's sessions"
        );
        let result = async {
            let now = UnixTime::now()?;
            self.store.revoke_account_grants(account, now).await?;
            self.store.delete_secrets(account).await
        }
        .await;
        if let Err(error) = result {
            tracing::error!(error = %error, "could not end the sessions");
        }
        self.stop_services(account).await;
    }

    /// Stops everything the account runs and forgets its password: the
    /// one way an account stops, after its last session ended or when the
    /// server refused its password.
    pub(crate) async fn stop_services(&self, account: AccountId) {
        // Nothing more is pushed to its browsers.
        if let Some(push) = &self.push {
            push.stop(account);
            if let Err(error) = self.store.delete_account_push_subscriptions(account).await {
                tracing::warn!(%error, "push subscriptions not deleted");
            }
        }
        self.set_login(account, None);
        self.quotas.forget(account);
        if let Some(sieve) = &self.sieve {
            sieve.stop(account);
        }
        if let Some(groupware) = &self.groupware {
            groupware.close(account).await;
        }
        self.mail.close(account).await;
    }
}

/// The JMAP backend of the bridge. Cheap to clone.
///
/// Run [`JmapBackend::run`] next to the HTTP server: it ends the sessions of
/// accounts whose password the server refuses, and deletes expired uploads
/// and submissions.
#[derive(Clone)]
pub struct JmapBackend {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for JmapBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmapBackend").finish_non_exhaustive()
    }
}

impl JmapBackend {
    /// The backend over `mail` (which must use the same `store`).
    #[must_use]
    pub fn new(store: Store, mail: Mail, resolver: Arc<dyn Resolver>, config: JmapConfig) -> Self {
        Self::with_groupware(store, mail, None, resolver, config)
    }

    /// The calendars and contacts service, if configured (tests use it to
    /// run a sync round on demand).
    #[must_use]
    pub fn groupware(&self) -> Option<&Groupware> {
        self.inner.groupware.as_ref()
    }

    /// The backend over `mail` and, if given, `groupware` (both on the same
    /// `store`). Calendars and contacts start and stop with each account;
    /// a DAV failure never stops the mail.
    #[must_use]
    pub fn with_groupware(
        store: Store,
        mail: Mail,
        groupware: Option<Groupware>,
        resolver: Arc<dyn Resolver>,
        config: JmapConfig,
    ) -> Self {
        let push = config.web_push.clone().map(crate::push::Service::new);
        let sieve = config
            .sieve
            .clone()
            .map(|settings| Arc::new(crate::sieve::Service::new(settings)));
        Self {
            inner: Arc::new(Inner {
                store,
                mail,
                groupware,
                resolver,
                config,
                push,
                sieve,
                logins: Mutex::new(HashMap::new()),
                quotas: QuotaHistory::default(),
                smtp_extensions: Mutex::new(None),
                held_wake: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Background work until `shutdown`: refused passwords end their
    /// sessions; expired uploads and submissions are deleted.
    pub async fn run(&self, shutdown: CancellationToken) {
        // What the SMTP server offers, for `submissionExtensions`; every
        // send refreshes it.
        // In the background: the server may be slow or down.
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            match crate::submission::send::probe(&inner).await {
                Ok(extensions) => inner.set_smtp_extensions(extensions),
                Err(error) => tracing::warn!(error = %error, "could not reach the SMTP server"),
            }
        });
        // Held messages (undo send, scheduled send) go out when due.
        tokio::spawn(crate::submission::release::run(
            Arc::clone(&self.inner),
            shutdown.clone(),
        ));
        let mut ticker = tokio::time::interval(self.inner.config.cleanup_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                // Not the event channel: that one drops what a slow reader
                // missed, and a refused password must end the sessions.
                accounts = self.inner.mail.auth_failures() => {
                    for account in accounts {
                        self.inner.end_sessions(account).await;
                    }
                }
                _ = ticker.tick() => self.purge().await,
            }
        }
    }

    async fn purge(&self) {
        let result = match UnixTime::now() {
            Ok(now) => self.inner.store.purge_expired(now).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            tracing::error!(error = %error, "cleanup of uploads and submissions failed");
        }
    }

    fn capabilities(&self, account: AccountId) -> AccountCapabilities {
        let config = &self.inner.config;
        // Contacts while the account's CardDAV works.
        let contacts = crate::contacts::capability(&self.inner, account);
        // Principals while DAV found the user's; free-busy only with a
        // scheduling server (outbox).
        let (principals, principals_availability) =
            crate::principals::capability(&self.inner, account);
        // Sieve once the account's ManageSieve server took the login.
        let (sieve, vacation_response) = crate::sieve::capability(&self.inner, account);
        AccountCapabilities {
            mail: Some(MailAccountCapability {
                max_mailboxes_per_email: None,
                max_mailbox_depth: None,
                max_size_mailbox_name: UnsignedInt::from(255_u32),
                max_size_attachments_per_email: UnsignedInt::saturating(
                    config.max_size_attachments,
                ),
                email_query_sort_options: SORT_OPTIONS.iter().map(|s| (*s).to_owned()).collect(),
                may_create_top_level_mailbox: true,
            }),
            // FUTURERELEASE: the bridge holds messages itself.
            submission: Some(SubmissionAccountCapability {
                max_delayed_send: if config.hold.enabled() {
                    UnsignedInt::saturating(config.hold.max_delay.as_secs())
                } else {
                    UnsignedInt::ZERO
                },
                submission_extensions: self.inner.submission_extensions(),
            }),
            // QUOTA as announced at the account's last IMAP login.
            quota: self
                .inner
                .mail
                .account(account)
                .is_some_and(|mail| mail.supports_quota()),
            // MDN/send goes through the same SMTP server; MDN/parse
            // only reads blobs.
            mdn: true,
            sieve,
            vacation_response,
            // Calendars while the account's CalDAV server has a calendar
            // home and takes the login; `calendars:parse`
            // always with them (the web sends both in `using`).
            calendars: crate::calendars::capability(&self.inner, account),
            calendars_parse: crate::calendars::capability(&self.inner, account).is_some(),
            contacts_parse: contacts.is_some(),
            contacts,
            principals,
            principals_availability,
            bridge: Some(crate::calendars::bridge_capability(&self.inner, account)),
            // Delivered mail inside the push (Stalwart 1.0), with `[push]`.
            email_push: self.inner.push.is_some(),
        }
    }
}

fn backend_error(error: &MailError) -> BackendError {
    match error {
        MailError::Unavailable(_) | MailError::AuthFailed => BackendError::Unavailable,
        _ => BackendError::Internal("mail layer"),
    }
}

/// What a failed login check means for the login limits. Only a check
/// with no verdict from the server (network, TLS, timeout, a server that
/// says it is unavailable) or a local problem (configuration, database) is
/// `Unavailable`, which doesn't count against the login. Anything else the
/// server answered counts as an attempt, even if it isn't the usual
/// "wrong password": an unknown answer must not give free tries.
fn login_outcome(error: &MailError, alert: Option<LoginAlert>) -> LoginOutcome {
    use nebula_imap::proto::response::ResponseCode;
    match error {
        MailError::AuthFailed => LoginOutcome::WrongCredentials { alert },
        error if error.is_retryable() => {
            tracing::warn!(error = %error, "login check failed");
            LoginOutcome::Unavailable
        }
        // RFC 5530: the server can't check it right now. LOGINDISABLED or
        // no usable mechanism: the server's configuration, not the user.
        MailError::Refused(refusal) if matches!(refusal.code, Some(ResponseCode::Unavailable)) => {
            tracing::warn!(error = %error, "login check failed");
            LoginOutcome::Unavailable
        }
        MailError::Config(_) | MailError::Store(_) | MailError::Unsupported(_) => {
            tracing::warn!(error = %error, "login check failed");
            LoginOutcome::Unavailable
        }
        _ => {
            tracing::warn!(error = %error, "unexpected answer to a login");
            LoginOutcome::Rejected
        }
    }
}

impl Backend for JmapBackend {
    type Events = BoxStream<'static, PushEvent>;

    async fn verify_login(&self, login: &LoginName, password: &SecretString) -> LoginOutcome {
        let (result, alerts) = self
            .inner
            .mail
            .check_login(login.as_str(), password.clone())
            .await;
        match result {
            Ok(()) => LoginOutcome::Success {
                email: self.inner.config.address_of(login.as_str()),
            },
            // The last alert says why (RFC 3501 §7.1: shown to the user).
            Err(error) => login_outcome(
                &error,
                alerts.last().map(|alert| LoginAlert::new(alert.as_str())),
            ),
        }
    }

    async fn account_started(
        &self,
        account: &AccountContext,
        password: SecretString,
        key: AccountKey,
        reason: StartReason,
    ) -> Result<(), BackendError> {
        let start = match reason {
            StartReason::Login => Start::Login,
            _ => Start::Restore,
        };
        match self
            .inner
            .mail
            .open(account.id, &account.login, password.clone(), start)
            .await
        {
            Ok(_) => {}
            // The stored (or just typed) password no longer works: end the
            // sessions now, so no request logs in with it again.
            Err(MailError::AuthFailed) => {
                self.inner.end_sessions(account.id).await;
                return Err(BackendError::PasswordRefused);
            }
            Err(error) => {
                tracing::warn!(error = %error, "account could not start");
                return Err(backend_error(&error));
            }
        }
        // Web Push deliveries for this account (they need its data key).
        if let Some(push) = &self.inner.push {
            push.start(Arc::downgrade(&self.inner), account, key.duplicate());
        }
        // Held messages (FUTURERELEASE) are sealed and opened with it.
        let held_key = key.duplicate();
        // Same login and password for DAV. It starts in the
        // background; whatever happens there, mail goes on.
        if let Some(groupware) = &self.inner.groupware
            && let Err(error) = groupware.open(account.id, &account.login, password.clone(), key)
        {
            tracing::warn!(error = %error, "calendars and contacts could not start");
        }
        // ManageSieve with the same login and password, probed in the
        // background.
        if let Some(sieve) = &self.inner.sieve {
            sieve.start(
                Arc::downgrade(&self.inner),
                account.id,
                nebula_managesieve::Credentials::new(account.login.clone(), password.clone()),
            );
        }
        self.inner.set_login(
            account.id,
            Some(SmtpLogin {
                login: account.login.clone(),
                password,
                key: held_key,
            }),
        );
        // Its held messages that waited for it go out now.
        crate::submission::release::account_started(&self.inner, account.id).await;
        Ok(())
    }

    async fn account_stopped(&self, account: AccountId) {
        // The last session ended.
        self.inner.stop_services(account).await;
    }

    fn session_ended(&self, account: AccountId) -> impl Future<Output = ()> + Send {
        // Its push subscriptions are gone from the store: the delivery
        // task drops them from memory.
        if let Some(push) = &self.inner.push {
            push.tell(account, crate::push::Command::Reload);
        }
        std::future::ready(())
    }

    fn account_capabilities(
        &self,
        account: &AccountContext,
    ) -> impl Future<Output = Result<AccountCapabilities, BackendError>> + Send {
        std::future::ready(Ok(self.capabilities(account.id)))
    }

    fn web_push_key(&self) -> Option<String> {
        self.inner
            .push
            .as_ref()
            .map(crate::push::Service::application_server_key)
    }

    async fn shared_accounts(
        &self,
        account: &AccountContext,
    ) -> Result<Vec<SharedAccount>, BackendError> {
        Ok(crate::sharing::accounts::shared_accounts(&self.inner, account).await)
    }

    async fn jmap(
        &self,
        account: &AccountContext,
        call: ApiCall,
    ) -> Result<Response, BackendError> {
        Ok(dispatch::run(&self.inner, account, call).await)
    }

    async fn upload(
        &self,
        account: &AccountContext,
        media_type: MediaType,
        body: UploadBody,
    ) -> Result<UploadedBlob, UploadError> {
        blob::upload::store(&self.inner, account, media_type, body).await
    }

    async fn download(
        &self,
        account: &AccountContext,
        blob_id: &BlobId,
    ) -> Result<Option<Blob>, BackendError> {
        blob::download::fetch(&self.inner, account, blob_id).await
    }

    fn events(
        &self,
        account: &AccountContext,
        alerts: bool,
    ) -> impl Future<Output = Result<Self::Events, BackendError>> + Send {
        std::future::ready(Ok(events::stream(&self.inner, account, alerts)))
    }

    async fn push_states(
        &self,
        account: &AccountContext,
    ) -> Result<nebula_jmap_proto::push::StateChange, BackendError> {
        events::current(&self.inner, account).await
    }
}

#[cfg(test)]
mod login_tests {
    use nebula_imap::proto::response::{Completion, ResponseCode};
    use nebula_imap::{Error as ImapError, Refusal};

    use super::*;

    fn refused(code: Option<ResponseCode>) -> MailError {
        MailError::Refused(Refusal {
            command: "LOGIN",
            status: Completion::No,
            code,
            text: "no".to_owned(),
        })
    }

    fn outcome(error: &MailError) -> LoginOutcome {
        login_outcome(error, None)
    }

    #[test]
    fn login_outcome_counts_unknown_answers() {
        assert_eq!(
            outcome(&MailError::AuthFailed),
            LoginOutcome::WrongCredentials { alert: None }
        );
        assert_eq!(
            outcome(&refused(Some(ResponseCode::Alert))),
            LoginOutcome::Rejected
        );
        assert_eq!(outcome(&refused(None)), LoginOutcome::Rejected);
        assert_eq!(
            outcome(&MailError::Invalid("value")),
            LoginOutcome::Rejected
        );
        assert_eq!(
            outcome(&MailError::from(ImapError::ConnectionClosed)),
            LoginOutcome::Unavailable
        );
        assert_eq!(
            outcome(&refused(Some(ResponseCode::Unavailable))),
            LoginOutcome::Unavailable
        );
        assert_eq!(
            outcome(&MailError::from(ImapError::NoAuthMechanism)),
            LoginOutcome::Unavailable
        );
        assert_eq!(
            outcome(&MailError::from(ImapError::InsecureTlsNotLoopback)),
            LoginOutcome::Unavailable
        );
    }
}
