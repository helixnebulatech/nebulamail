// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! LOGIN / AUTHENTICATE, then capabilities and ENABLE.

use nebula_imap_proto::command::{
    Authenticate, Enable, EnableExtension, InitialResponse, Login, SaslMechanism,
};
use nebula_imap_proto::response::{Completion, ResponseCode, Tagged, Untagged};
use nebula_imap_proto::types::{AuthMechanism, Capability, NameEncoding};
use secrecy::SecretString;

use crate::core::client::{Authenticated, Client, NotAuthenticated, ObjectIdSupport, SyncLevel};
use crate::core::connection::StrayContinuation;
use crate::{Error, Refusal, StateError};

/// User name and password of a mail account.
pub struct Credentials {
    username: String,
    password: SecretString,
}

impl Credentials {
    /// Creates credentials.
    #[must_use]
    pub fn new(username: impl Into<String>, password: SecretString) -> Self {
        Self {
            username: username.into(),
            password,
        }
    }

    /// The user name.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Whether these are the same user name and password as `other`
    /// (compared in constant time; the password is never exposed).
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        use secrecy::ExposeSecret as _;
        use subtle::ConstantTimeEq as _;
        let names = self.username.as_bytes().ct_eq(other.username.as_bytes());
        let passwords = self
            .password
            .expose_secret()
            .as_bytes()
            .ct_eq(other.password.expose_secret().as_bytes());
        bool::from(names & passwords)
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Neither the password nor the user name (personal data) is printed.
        f.write_str("Credentials { .. }")
    }
}

/// An OAuth SASL mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OAuthMechanism {
    /// `XOAUTH2` (Gmail, Outlook).
    XOAuth2,
    /// `OAUTHBEARER` (RFC 7628).
    OAuthBearer,
}

impl Client<NotAuthenticated> {
    /// Logs in with a password: `AUTHENTICATE PLAIN` (with SASL-IR when
    /// possible) if the server offers it, else `LOGIN`.
    ///
    /// Afterwards capabilities are refreshed and `QRESYNC` (or `CONDSTORE`)
    /// and `UTF8=ACCEPT` are enabled when available.
    ///
    /// # Errors
    ///
    /// [`Error::AuthFailed`] for wrong credentials (the client comes back in
    /// the error, so another attempt is possible), other errors otherwise.
    pub async fn login(
        mut self,
        credentials: &Credentials,
    ) -> Result<Client<Authenticated>, StateError<Client<NotAuthenticated>>> {
        let result = self.authenticate_password(credentials).await;
        self.finish_login(result).await
    }

    /// Logs in with an OAuth access token (`XOAUTH2` or `OAUTHBEARER`).
    ///
    /// # Errors
    ///
    /// As [`Self::login`], and [`Error::NoAuthMechanism`] if the server
    /// does not offer the mechanism.
    pub async fn login_oauth(
        mut self,
        username: &str,
        token: &SecretString,
        mechanism: OAuthMechanism,
    ) -> Result<Client<Authenticated>, StateError<Client<NotAuthenticated>>> {
        let result = self.authenticate_oauth(username, token, mechanism).await;
        self.finish_login(result).await
    }

    async fn finish_login(
        self,
        result: Result<Tagged, Error>,
    ) -> Result<Client<Authenticated>, StateError<Client<NotAuthenticated>>> {
        let tagged = match result {
            Ok(tagged) => tagged,
            Err(error) => return Err(StateError::new(error, self)),
        };
        let mut client = self.into_state(Authenticated);
        client
            .after_authentication(tagged.code)
            .await
            .map_err(StateError::fatal)?;
        tracing::debug!(sync = ?client.session.sync, names = ?client.session.names, "IMAP logged in");
        Ok(client)
    }

    async fn authenticate_password(&mut self, credentials: &Credentials) -> Result<Tagged, Error> {
        let session = &self.session;
        let tagged = if session.capabilities.has_auth(&AuthMechanism::Plain) {
            let mechanism = SaslMechanism::Plain {
                username: &credentials.username,
                password: &credentials.password,
            };
            let command = Authenticate {
                mechanism,
                initial_response: self.initial_response(),
            };
            self.run_with(&command, StrayContinuation::CancelSasl, Some)
                .await?
        } else if !session.has(&Capability::LoginDisabled) {
            let command = Login {
                username: &credentials.username,
                password: &credentials.password,
            };
            self.run_with(&command, StrayContinuation::Reject, Some)
                .await?
        } else {
            return Err(Error::NoAuthMechanism);
        };
        check_auth(tagged)
    }

    async fn authenticate_oauth(
        &mut self,
        username: &str,
        token: &SecretString,
        mechanism: OAuthMechanism,
    ) -> Result<Tagged, Error> {
        let (wanted, mechanism, stray) = match mechanism {
            OAuthMechanism::XOAuth2 => (
                AuthMechanism::XOAuth2,
                SaslMechanism::XOAuth2 { username, token },
                StrayContinuation::CancelSasl,
            ),
            OAuthMechanism::OAuthBearer => (
                AuthMechanism::OAuthBearer,
                SaslMechanism::OAuthBearer { username, token },
                StrayContinuation::OAuthBearer,
            ),
        };
        if !self.session.capabilities.has_auth(&wanted) {
            return Err(Error::NoAuthMechanism);
        }
        let command = Authenticate {
            mechanism,
            initial_response: self.initial_response(),
        };
        let tagged = self.run_with(&command, stray, Some).await?;
        check_auth(tagged)
    }

    fn initial_response(&self) -> InitialResponse {
        if self.session.has(&Capability::SaslIr) {
            InitialResponse::Inline
        } else {
            InitialResponse::AfterContinuation
        }
    }
}

/// Maps a NO to [`Error::AuthFailed`] unless its code says the server has
/// a passing problem (`UNAVAILABLE`, `SERVERBUG`, `INUSE`, `LIMIT`). Any
/// other NO (`[ALERT]`, `[CONTACTADMIN]`, a vendor code…) counts as wrong
/// credentials: it goes to the login limiter and is not retried. BAD stays
/// a refusal: it is a protocol error, not an answer about the password.
fn check_auth(tagged: Tagged) -> Result<Tagged, Error> {
    match (tagged.status, &tagged.code) {
        (Completion::Ok, _) => Ok(tagged),
        (
            Completion::No,
            Some(
                ResponseCode::Unavailable
                | ResponseCode::ServerBug
                | ResponseCode::InUse
                | ResponseCode::Limit,
            ),
        ) => Err(Error::Refused(Refusal {
            command: "LOGIN",
            status: Completion::No,
            code: tagged.code,
            text: tagged.text,
        })),
        (Completion::No, _) => Err(Error::AuthFailed {
            code: tagged.code,
            text: tagged.text,
        }),
        (status, _) => Err(Error::Refused(Refusal {
            command: "LOGIN",
            status,
            code: tagged.code,
            text: tagged.text,
        })),
    }
}

impl Client<Authenticated> {
    /// After LOGIN or PREAUTH: fresh capabilities, then ENABLE.
    pub(crate) async fn after_authentication(
        &mut self,
        code: Option<ResponseCode>,
    ) -> Result<(), Error> {
        match code {
            Some(ResponseCode::Capabilities(capabilities)) => {
                self.session.capabilities = capabilities;
            }
            // Servers may change capabilities after login: ask again.
            _ => {
                self.refresh_capabilities().await?;
            }
        }
        self.enable_extensions().await
    }

    async fn enable_extensions(&mut self) -> Result<(), Error> {
        // RFC 9051 §5.1: with IMAP4rev2, mailbox names are UTF-8 (modified
        // UTF-7 is only for IMAP4rev1, Appendix A.1). A server with both
        // revisions stays rev1 (§6.1.1), since we don't ENABLE IMAP4rev2.
        if self.session.capabilities.is_rev2_only() {
            self.use_utf8_names();
        }
        let session = &mut self.session;
        // RFC 8474 needs no ENABLE.
        if session.has(&Capability::ObjectId) {
            session.object_ids = ObjectIdSupport::Rfc8474;
        }
        // CONDSTORE also works without ENABLE (by using its parameters).
        if session.has(&Capability::CondStore) {
            session.sync = SyncLevel::CondStore;
        }
        let mut wanted = Vec::new();
        if session.has(&Capability::QResync) {
            wanted.push(EnableExtension::QResync);
        } else if session.has(&Capability::CondStore) {
            wanted.push(EnableExtension::CondStore);
        }
        if session.has(&Capability::Utf8Accept) {
            wanted.push(EnableExtension::Utf8Accept);
        }
        if session.has(&Capability::ObjectIdPlus) && !session.has(&Capability::ObjectId) {
            wanted.push(EnableExtension::ObjectIdPlus);
        }
        if wanted.is_empty() || !session.has(&Capability::Enable) {
            return Ok(());
        }
        let mut enabled = Vec::new();
        let command = Enable { extensions: wanted };
        let tagged = self
            .run_with(&command, StrayContinuation::Reject, |data| match data {
                Untagged::Enabled(list) => {
                    enabled.extend(list);
                    None
                }
                other => Some(other),
            })
            .await?;
        // A NO to ENABLE is not fatal: we just go on without extensions.
        if tagged.status != Completion::Ok {
            tracing::warn!("IMAP ENABLE refused");
            return Ok(());
        }
        self.apply_enabled(&enabled);
        Ok(())
    }

    fn apply_enabled(&mut self, enabled: &[Capability]) {
        let session = &mut self.session;
        if enabled.contains(&Capability::QResync) {
            session.sync = SyncLevel::QResync;
        }
        if enabled.contains(&Capability::ObjectIdPlus) {
            session.object_ids = ObjectIdSupport::ObjectIdPlus;
        }
        if enabled.contains(&Capability::Utf8Accept) {
            self.session.utf8_accept = true;
            self.use_utf8_names();
        }
    }

    /// Mailbox names in UTF-8 from now on, both ways.
    fn use_utf8_names(&mut self) {
        self.session.names = NameEncoding::Utf8;
        self.connection
            .decoder_mut()
            .set_name_encoding(NameEncoding::Utf8);
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;

    use super::Credentials;

    #[test]
    fn same_as() {
        let a = Credentials::new("alice", SecretString::from("one"));
        assert!(a.same_as(&Credentials::new("alice", SecretString::from("one"))));
        assert!(!a.same_as(&Credentials::new("alice", SecretString::from("two"))));
        assert!(!a.same_as(&Credentials::new("bob", SecretString::from("one"))));
        assert!(!a.same_as(&Credentials::new("alice", SecretString::from("one "))));
    }
}
