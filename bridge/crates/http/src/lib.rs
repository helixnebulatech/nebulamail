// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP server of the bridge: its own OAuth 2.1
//! authorization server, the JMAP session, API, blobs and push.
//!
//! - **OAuth** (`/.well-known/oauth-authorization-server`, `/oauth/authorize`,
//!   `/oauth/token`, `/oauth/revoke`): code + PKCE S256, `iss` (RFC 9207),
//!   `client_secret_basic` with argon2id hashes from the configuration,
//!   refresh tokens that rotate on every use. The user logs in with their
//!   mail login and password, checked against the mail server.
//! - **JMAP** (`/.well-known/jmap`, `/jmap/api`, `/jmap/upload/…`,
//!   `/jmap/download/…`, `/jmap/eventsource/`): Bearer tokens, the core
//!   limits, CORS for the web app's origins only.
//!
//! The mail side plugs in through [`Backend`]; this crate depends on no
//! mail crate. Passwords are sealed here (`nebula-crypto`) and stored
//! (`nebula-store`) only while the account has a login session.
//!
//! ```no_run
//! # async fn example<B: nebula_http::Backend>(
//! #     config: nebula_http::HttpConfig,
//! #     store: nebula_store::Store,
//! #     keyring: nebula_crypto::Keyring,
//! #     backend: B,
//! # ) -> std::io::Result<()> {
//! use tokio_util::sync::CancellationToken;
//!
//! let shutdown = CancellationToken::new();
//! let server = nebula_http::Server::new(config, store, keyring, backend, shutdown.clone());
//! tokio::spawn({
//!     let server = server.clone();
//!     async move { server.run_cleanup(std::time::Duration::from_secs(60)).await }
//! });
//! let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await?;
//! server.serve(listener).await
//! # }
//! ```

#![forbid(unsafe_code)]

mod backend;
mod config;
mod error;
mod form;
mod jmap;
mod limits;
mod middleware;
mod oauth;
mod serve;
mod state;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use http::{HeaderName, Method, header};
use nebula_crypto::Keyring;
use nebula_store::Store;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};

pub use backend::{
    AcceptLanguage, AccountContext, ApiCall, Backend, BackendError, Blob, BlobBody,
    InvalidCredentials, InvalidMediaType, LoginAlert, LoginName, LoginOutcome, MediaType,
    PushEvent, SharedAccount, StartReason, UploadBody, UploadBodyError, UploadError, UploadedBlob,
    jmap_account_id, shared_account_id, shared_account_key,
};
pub use config::{
    AppOrigin, ClientId, ClientIpSource, ConfigError, HttpConfig, JmapLimits, Lifetimes,
    LoginLimits, OAuthClient, PublicUrl, RateLimit, RedirectUri, SignedOutSending, is_loopback,
};
pub use error::HttpError;
pub use state::{RewrapReport, rewrap_data_keys};

use crate::middleware::Deadline;
use crate::state::Shared;

/// Largest body of the OAuth endpoints (forms); JMAP API and uploads have
/// their own limits from the core capability.
const FORM_BODY_LIMIT: usize = 16 * 1024;
/// Time limits per group of endpoints. `EventSource` has none (it ends
/// after an access token's lifetime).
const OAUTH_TIMEOUT: Duration = Duration::from_secs(30);
const JMAP_TIMEOUT: Duration = Duration::from_secs(300);

/// The HTTP server. Cheap to clone.
#[derive(Debug)]
pub struct Server<B> {
    shared: Arc<Shared<B>>,
}

impl<B> Clone for Server<B> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<B: Backend> Server<B> {
    /// A server. `shutdown` ends open push streams and the cleanup loop.
    #[must_use]
    pub fn new(
        config: HttpConfig,
        store: Store,
        keyring: Keyring,
        backend: B,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            shared: Arc::new(Shared::new(config, store, keyring, backend, shutdown)),
        }
    }

    /// The routes with all their layers.
    pub fn router(&self) -> Router {
        // The sign-in page answers in HTML, the other OAuth endpoints in
        // JSON: each its own 503 when it takes too long.
        let login = Router::new()
            .route(
                "/oauth/authorize",
                get(oauth::authorize::authorize_get::<B>)
                    .post(oauth::authorize::authorize_post::<B>),
            )
            .layer(axum::middleware::from_fn_with_state(
                Deadline::login_page(OAUTH_TIMEOUT),
                middleware::deadline,
            ));
        let oauth = Router::new()
            .route(
                "/.well-known/oauth-authorization-server",
                get(oauth::discovery::<B>),
            )
            .route("/oauth/token", post(oauth::token::token::<B>))
            .route("/oauth/revoke", post(oauth::revoke::revoke::<B>))
            .layer(axum::middleware::from_fn_with_state(
                Deadline::oauth(OAUTH_TIMEOUT),
                middleware::deadline,
            ))
            .merge(login);
        let jmap = Router::new()
            .route("/.well-known/jmap", get(jmap::session::session::<B>))
            .route(jmap::session::API_PATH, post(jmap::api::api::<B>))
            .route("/jmap/upload/{account_id}", post(jmap::blob::upload::<B>))
            .route(
                "/jmap/download/{account_id}/{blob_id}/{name}",
                get(jmap::blob::download::<B>),
            )
            // Not the event source, added after: it has no time limit.
            .layer(axum::middleware::from_fn_with_state(
                Deadline::jmap(JMAP_TIMEOUT),
                middleware::deadline,
            ))
            .route(
                "/jmap/eventsource/",
                get(jmap::eventsource::eventsource::<B>),
            )
            .layer(self.cors());
        let https = self.shared.config.public_url().is_https();
        Router::new()
            .merge(oauth)
            .merge(jmap)
            .route("/healthz", get(async || "ok"))
            .route(oauth::page::FONT_PATH, get(oauth::page::font))
            .fallback(middleware::not_found)
            .layer(DefaultBodyLimit::max(FORM_BODY_LIMIT))
            .layer(axum::middleware::from_fn(middleware::log_requests))
            .layer(axum::middleware::map_response_with_state(
                https,
                middleware::security_headers,
            ))
            .layer(CatchPanicLayer::custom(middleware::panic_response))
            .with_state(Arc::clone(&self.shared))
    }

    /// CORS for the web app: it calls JMAP from the browser with a Bearer
    /// token, so only the configured origins, no cookies, and the headers
    /// its client sends (`Last-Event-ID` when push reconnects).
    fn cors(&self) -> CorsLayer {
        let origins = self
            .shared
            .config
            .app_origins()
            .iter()
            .filter_map(|origin| origin.as_str().parse().ok())
            .collect::<Vec<_>>();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                header::ACCEPT,
                // The web app's interface language (invitation emails).
                header::ACCEPT_LANGUAGE,
                HeaderName::from_static("last-event-id"),
            ])
            // Without it the browser hides when to retry a 503.
            .expose_headers([header::RETRY_AFTER])
            .max_age(Duration::from_secs(600))
    }

    /// Serves until the shutdown token is cancelled. At that point the
    /// listener closes and open requests get 20 seconds to finish; after
    /// that their connections are dropped (a long upload is cut, and the
    /// client tries again). A client must send each request's headers
    /// within 15 seconds (slowloris), and a socket that takes no bytes for
    /// 60 seconds is closed (a client that stopped reading).
    ///
    /// # Errors
    /// If accepting connections fails.
    pub async fn serve(&self, listener: TcpListener) -> std::io::Result<()> {
        serve::serve(
            listener,
            self.router(),
            self.shared.shutdown.clone(),
            serve::Timeouts::DEFAULT,
        )
        .await
    }

    /// Starts these accounts now if they still have a session, or held
    /// messages that go out after sign-out (after a restart: the accounts
    /// with Web Push subscriptions, since new mail and reminders must reach
    /// them with every tab closed, and [`Self::accounts_held_while_signed_out`]).
    /// Failures are logged; the account starts at its next request
    /// instead.
    pub async fn start_accounts(&self, accounts: &[nebula_store::AccountId]) {
        for &id in accounts {
            if let Err(error) = self.shared.start_if_active(id).await {
                tracing::warn!(account = id.get(), %error, "account not started at boot");
            }
        }
    }

    /// The accounts whose held messages go out after sign-out
    /// ([`SignedOutSending`]): start them at boot with
    /// [`Self::start_accounts`]. Empty when that is off.
    ///
    /// # Errors
    /// On store failures.
    pub async fn accounts_held_while_signed_out(
        &self,
    ) -> Result<Vec<nebula_store::AccountId>, HttpError> {
        self.shared.accounts_held_while_signed_out().await
    }

    /// One cleanup pass: expired codes and tokens, and accounts whose last
    /// session ended (their password is deleted and the backend stops).
    ///
    /// # Errors
    /// On store failures.
    pub async fn cleanup(&self) -> Result<(), HttpError> {
        self.shared.cleanup().await
    }

    /// Runs [`Self::cleanup`] every `every` until shutdown.
    pub async fn run_cleanup(&self, every: Duration) {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shared.shutdown.cancelled() => return,
                _ = ticker.tick() => {
                    if let Err(error) = self.cleanup().await {
                        tracing::error!(error = %error, "cleanup failed");
                    }
                }
            }
        }
    }
}

/// The parsers of untrusted input, for the fuzz targets (`fuzz/`) only.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    /// A form body or query string, parsed the way the OAuth endpoints do,
    /// and the decoding of `client_secret_basic` parts.
    pub fn form(input: &[u8]) {
        if let Ok(form) = crate::form::Form::parse(input) {
            let _ = form.get("client_id");
        }
        if let Ok(text) = std::str::from_utf8(input) {
            let _ = crate::form::decode_component(text);
        }
    }

    /// An `Authorization: Basic …` header value.
    pub fn basic_auth(value: &[u8]) {
        if let Ok(value) = http::HeaderValue::from_bytes(value) {
            let mut headers = http::HeaderMap::new();
            headers.insert(http::header::AUTHORIZATION, value);
            let _ = crate::oauth::client_auth::parse_basic(&headers);
        }
    }

    /// The text fields of uploads and the login form.
    pub fn fields(text: &str) {
        let _ = text.parse::<crate::MediaType>();
        let _ = text.parse::<crate::LoginName>();
        let _ = crate::backend::parse_password(text);
        let _ = crate::jmap::blob::safe_file_name(text);
    }
}
