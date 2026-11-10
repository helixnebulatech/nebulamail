// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! What a method call works with: the account, the store, the mail account
//! and the ids created earlier in the request.

use std::sync::Arc;

use nebula_http::AccountContext;
use nebula_jmap_proto::{
    AccountId as JmapAccountId, CreatedIds, IdOrRef, MethodError, MethodErrorType, MethodLimits,
    State,
};
use nebula_mail::MailAccount;
use nebula_store::{AccountId, DataType, Store, UnixTime};

use super::backend::Inner;
use super::config::JmapConfig;
use super::ids;
use crate::error::CallError;

/// One request in progress.
pub(crate) struct Ctx<'a> {
    /// Shared, so work that must outlive the request (an SMTP send cut off
    /// by a closed tab or the HTTP timeout) can own it.
    pub(crate) inner: &'a Arc<Inner>,
    pub(crate) account: &'a AccountContext,
    /// Creation id → id, for `#creationId` references (RFC 8620 §5.3).
    pub(crate) created: CreatedIds,
    pub(crate) limits: MethodLimits,
    /// The request has the bridge's vendor capability in `using`: its
    /// extensions apply (RFC 8620 §1.8; the Calendar property
    /// `https://helixnebula.tech/jmap/bridge/components`).
    pub(crate) uses_bridge: bool,
    /// The request's `using`: what the client understands (RFC 8620
    /// §3.3; e.g. which data types a Quota may name, RFC 9425 §4.1).
    pub(crate) using: Vec<nebula_jmap_proto::Capability>,
    /// The request's `Accept-Language` (text the bridge writes for the
    /// user: invitation emails).
    pub(crate) languages: nebula_http::AcceptLanguage,
    /// Whose calendars and address books the call sees (someone else's in
    /// their shared account).
    pub(crate) scope: crate::sharing::Scope,
    /// The login session of the access token: push subscriptions are
    /// per session (RFC 8620 §7.2).
    pub(crate) session: nebula_store::GrantId,
}

impl Ctx<'_> {
    pub(crate) fn store(&self) -> &Store {
        &self.inner.store
    }

    pub(crate) fn config(&self) -> &JmapConfig {
        &self.inner.config
    }

    /// The store account.
    pub(crate) fn id(&self) -> AccountId {
        self.account.id
    }

    /// The JMAP `accountId`.
    pub(crate) fn jmap_id(&self) -> JmapAccountId {
        self.account.jmap_id.clone()
    }

    /// Whether the request has `capability` in `using`.
    pub(crate) fn uses(&self, capability: &nebula_jmap_proto::Capability) -> bool {
        self.using.contains(capability)
    }

    /// Every call names the account; only the logged-in one runs here (a
    /// shared account the method doesn't support is told apart in
    /// [`crate::sharing::scope::unsupported_account`]).
    pub(crate) fn check_account(&self, requested: &JmapAccountId) -> Result<(), CallError> {
        if *requested == self.account.jmap_id {
            Ok(())
        } else {
            Err(MethodErrorType::AccountNotFound.into())
        }
    }

    /// The running mail account (for anything that talks to IMAP).
    pub(crate) fn mail(&self) -> Result<Arc<MailAccount>, CallError> {
        self.inner.mail.account(self.account.id).ok_or_else(|| {
            MethodError::new(MethodErrorType::ServerUnavailable)
                .with_description("the account is not connected to the mail server")
                .into()
        })
    }

    /// The current JMAP state of a data type.
    pub(crate) async fn state(&self, data_type: DataType) -> Result<State, CallError> {
        Ok(ids::state(self.store().state(self.id(), data_type).await?))
    }

    /// `ifInState`: the call fails unless the state is still this one.
    pub(crate) async fn check_state(
        &self,
        data_type: DataType,
        expected: Option<&State>,
    ) -> Result<State, CallError> {
        let current = self.state(data_type).await?;
        match expected {
            Some(expected) if *expected != current => Err(MethodErrorType::StateMismatch.into()),
            Some(_) | None => Ok(current),
        }
    }

    /// Resolves an id that may be a `#creationId`.
    pub(crate) fn resolve<T: nebula_jmap_proto::types::IdKind>(
        &self,
        id: &IdOrRef<T>,
    ) -> Option<T> {
        id.resolve(&self.created).ok()
    }

    pub(crate) fn now() -> Result<UnixTime, CallError> {
        Ok(UnixTime::now()?)
    }
}
