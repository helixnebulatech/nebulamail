// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Router tests with a fake backend and a temporary store.

mod bff;
mod held;
mod jmap;
mod oauth;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use bytes::Bytes;
use data_encoding::BASE64;
use futures::StreamExt as _;
use futures::stream::BoxStream;
use http::{HeaderMap, Request, StatusCode, header};
use nebula_crypto::{ClientSecretHash, KeyVersion, Keyring, MasterKey};
use nebula_jmap_proto::push::StateChange;
use nebula_jmap_proto::response::{ResponseInvocation, ResponseName};
use nebula_jmap_proto::session::{
    AccountCapabilities, MailAccountCapability, SubmissionAccountCapability,
};
use nebula_jmap_proto::{BlobId, Response as JmapResponse, UnsignedInt};
use nebula_store::{AccountId, Store, UnixTime};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use url::Url;

use crate::Server;
use crate::backend::{
    AccountContext, ApiCall, Backend, BackendError, Blob, BlobBody, LoginName, LoginOutcome,
    MediaType, PushEvent, StartReason, UploadBody, UploadError, UploadedBlob,
};
use crate::config::{AppOrigin, HttpConfig, OAuthClient, PublicUrl, RedirectUri};
use crate::state::Shared;

pub(crate) const ISSUER: &str = "https://jmap.example.com";
pub(crate) const APP: &str = "https://mail.example.com";
pub(crate) const REDIRECT: &str = "https://mail.example.com/auth/callback";
pub(crate) const CLIENT_ID: &str = "nebulamail";
pub(crate) const CLIENT_SECRET: &str = "s3cret value";
/// RFC 7636 appendix B.
pub(crate) const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
pub(crate) const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
pub(crate) const START: i64 = 1_800_000_000;

/// What the fake backend saw.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    /// (login, password) of every `account_started`.
    pub(crate) started: Vec<(String, String)>,
    /// Why each `account_started` was called.
    pub(crate) reasons: Vec<StartReason>,
    /// `account_started` answers that the mail server refused the password.
    pub(crate) refuse_password: bool,
    pub(crate) stopped: Vec<AccountId>,
    /// Accounts of every `session_ended`.
    pub(crate) sessions_ended: Vec<AccountId>,
    pub(crate) uploads: HashMap<String, (Bytes, MediaType)>,
    pub(crate) api_calls: usize,
    /// The `alerts` of every `events` call.
    pub(crate) event_alerts: Vec<bool>,
    /// "started" and "stopped", in order.
    pub(crate) lifecycle: Vec<&'static str>,
}

/// Accepts alice / "right password" with any case (like most servers),
/// bob, carol / "carol password" and Carol / "other carol" (two users of a
/// server that tells case apart); "down" is unavailable, "odd" gets an
/// unusual answer.
#[derive(Debug, Clone)]
pub(crate) struct Fake {
    pub(crate) seen: Arc<Mutex<Seen>>,
    pub(crate) events: broadcast::Sender<PushEvent>,
    /// If set, `account_started` waits for a permit (after notifying
    /// `entered`).
    pub(crate) start_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub(crate) entered: Arc<tokio::sync::Notify>,
    /// What `push_states` answers.
    pub(crate) states: Arc<Mutex<StateChange>>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            seen: Arc::default(),
            events: broadcast::channel(16).0,
            start_gate: None,
            entered: Arc::default(),
            states: Arc::default(),
        }
    }
}

// The fake answers at once, so most methods have nothing to await.
#[allow(clippy::unused_async_trait_impl)]
impl Backend for Fake {
    type Events = BoxStream<'static, PushEvent>;

    async fn verify_login(&self, login: &LoginName, password: &SecretString) -> LoginOutcome {
        match (login.as_str(), password.expose_secret()) {
            ("alice" | "Alice", "right password") => LoginOutcome::Success {
                email: "alice@example.test".to_owned(),
            },
            ("bob", "right password") => LoginOutcome::Success {
                email: format!("bob@{}", "example.com"),
            },
            // A server that tells case apart: two different users.
            ("carol", "carol password") | ("Carol", "other carol") => LoginOutcome::Success {
                email: "carol@example.com".to_owned(),
            },
            ("down", _) => LoginOutcome::Unavailable,
            ("odd", _) => LoginOutcome::Rejected,
            ("locked", _) => LoginOutcome::WrongCredentials {
                alert: Some(crate::LoginAlert::new("Account locked <b>call</b> us")),
            },
            _ => LoginOutcome::WrongCredentials { alert: None },
        }
    }

    async fn account_started(
        &self,
        account: &AccountContext,
        password: SecretString,
        key: nebula_crypto::AccountKey,
        reason: StartReason,
    ) -> Result<(), BackendError> {
        // The data key is bound to the login, like the password.
        assert_eq!(
            key.account().as_bytes(),
            format!("login:{}", account.login.to_ascii_lowercase()).as_bytes()
        );
        if let Some(gate) = &self.start_gate {
            self.entered.notify_one();
            gate.acquire().await.unwrap().forget();
        }
        let mut seen = self.seen.lock().unwrap();
        seen.started
            .push((account.login.clone(), password.expose_secret().to_owned()));
        seen.reasons.push(reason);
        if seen.refuse_password {
            return Err(BackendError::PasswordRefused);
        }
        seen.lifecycle.push("started");
        Ok(())
    }

    async fn account_stopped(&self, account: AccountId) {
        let mut seen = self.seen.lock().unwrap();
        seen.stopped.push(account);
        seen.lifecycle.push("stopped");
    }

    async fn session_ended(&self, account: AccountId) {
        self.seen.lock().unwrap().sessions_ended.push(account);
    }

    async fn account_capabilities(
        &self,
        _account: &AccountContext,
    ) -> Result<AccountCapabilities, BackendError> {
        Ok(AccountCapabilities {
            mail: Some(MailAccountCapability {
                max_mailboxes_per_email: None,
                max_mailbox_depth: None,
                max_size_mailbox_name: UnsignedInt::from(255_u32),
                max_size_attachments_per_email: UnsignedInt::from(50_000_000_u32),
                email_query_sort_options: vec!["receivedAt".to_owned()],
                may_create_top_level_mailbox: true,
            }),
            submission: Some(SubmissionAccountCapability {
                max_delayed_send: UnsignedInt::from(0_u32),
                submission_extensions: BTreeMap::new(),
            }),
            quota: false,
            mdn: false,
            ..AccountCapabilities::default()
        })
    }

    async fn jmap(
        &self,
        _account: &AccountContext,
        call: ApiCall,
    ) -> Result<JmapResponse, BackendError> {
        self.seen.lock().unwrap().api_calls += 1;
        // Echo every call back.
        let method_responses = call
            .request
            .method_calls
            .into_iter()
            .map(|invocation| ResponseInvocation {
                name: ResponseName::Method(invocation.name),
                arguments: invocation.arguments,
                call_id: invocation.call_id,
            })
            .collect();
        Ok(JmapResponse {
            method_responses,
            created_ids: None,
            session_state: call.session_state,
        })
    }

    async fn upload(
        &self,
        _account: &AccountContext,
        media_type: MediaType,
        mut body: UploadBody,
    ) -> Result<UploadedBlob, UploadError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = body.chunk().await? {
            bytes.extend_from_slice(&chunk);
        }
        if media_type.as_str() == "application/x-full" {
            return Err(UploadError::OverQuota);
        }
        let bytes = Bytes::from(bytes);
        let size = u64::try_from(bytes.len()).unwrap();
        self.seen
            .lock()
            .unwrap()
            .uploads
            .insert("b1".to_owned(), (bytes, media_type.clone()));
        Ok(UploadedBlob {
            blob_id: "b1".parse().unwrap(),
            media_type,
            size,
        })
    }

    async fn download(
        &self,
        _account: &AccountContext,
        blob_id: &BlobId,
    ) -> Result<Option<Blob>, BackendError> {
        Ok(match blob_id.as_str() {
            // The bridge is busy with other large messages.
            "busy" => return Err(BackendError::Unavailable),
            "b1" => Some(Blob {
                body: BlobBody::Bytes(Bytes::from_static(b"<script>alert(1)</script>")),
                media_type: "text/html".parse().unwrap(),
            }),
            // Streamed in three pieces.
            "b2" => Some(Blob {
                body: BlobBody::Stream {
                    size: 9,
                    chunks: futures::stream::iter(
                        ["abc", "def", "ghi"].map(|s| Ok(Bytes::from_static(s.as_bytes()))),
                    )
                    .boxed(),
                },
                media_type: "text/plain".parse().unwrap(),
            }),
            // Never ends (a slow message part).
            "b3" => Some(Blob {
                body: BlobBody::Stream {
                    size: 1_000_000,
                    chunks: futures::stream::pending().boxed(),
                },
                media_type: "text/plain".parse().unwrap(),
            }),
            _ => None,
        })
    }

    async fn events(
        &self,
        _account: &AccountContext,
        alerts: bool,
    ) -> Result<Self::Events, BackendError> {
        self.seen.lock().unwrap().event_alerts.push(alerts);
        let receiver = self.events.subscribe();
        Ok(
            futures::stream::unfold(receiver, |mut receiver| async move {
                loop {
                    match receiver.recv().await {
                        Ok(change) => return Some((change, receiver)),
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            })
            .boxed(),
        )
    }

    async fn push_states(&self, _account: &AccountContext) -> Result<StateChange, BackendError> {
        Ok(self.states.lock().unwrap().clone())
    }
}

/// A response with its body read.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Bytes,
}

impl Reply {
    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8(self.body.to_vec()).unwrap()
    }

    pub(crate) fn header(&self, name: impl header::AsHeaderName) -> Option<&str> {
        self.headers.get(name).map(|v| v.to_str().unwrap())
    }

    /// The `Location` of a redirect, parsed.
    pub(crate) fn location(&self) -> Url {
        Url::parse(self.header(header::LOCATION).unwrap()).unwrap()
    }
}

/// Query parameters of a URL as a map.
pub(crate) fn query(url: &Url) -> HashMap<String, String> {
    url.query_pairs().into_owned().collect()
}

pub(crate) struct Harness {
    pub(crate) server: Server<Fake>,
    pub(crate) router: Router,
    pub(crate) fake: Fake,
    pub(crate) clock: Arc<AtomicI64>,
    pub(crate) store: Store,
    pub(crate) shutdown: CancellationToken,
    pub(crate) dir: tempfile::TempDir,
}

pub(crate) fn keyring() -> Keyring {
    let key = MasterKey::from_bytes(KeyVersion::new(1).unwrap(), &[7; 32]).unwrap();
    Keyring::new(key, []).unwrap()
}

pub(crate) fn config() -> HttpConfig {
    // One hash for the whole test binary: argon2 is slow on purpose.
    static HASH: std::sync::LazyLock<ClientSecretHash> = std::sync::LazyLock::new(|| {
        ClientSecretHash::hash(&SecretString::from(CLIENT_SECRET)).unwrap()
    });
    let client = OAuthClient::new(
        CLIENT_ID.parse().unwrap(),
        HASH.clone(),
        vec![RedirectUri::parse(REDIRECT).unwrap()],
    )
    .unwrap();
    let mut config = HttpConfig::new(
        PublicUrl::parse(ISSUER).unwrap(),
        vec![AppOrigin::parse(APP).unwrap()],
        vec![client],
    )
    .unwrap();
    // Tests pick the client's address with `X-Forwarded-For` (see
    // [`FROM`]); without it every request shares one address.
    config.client_ip = crate::config::ClientIpSource::XForwardedFor;
    config
}

/// The header that sets a test request's client address.
pub(crate) const FROM: &str = "x-forwarded-for";

impl Harness {
    pub(crate) async fn new() -> Self {
        Self::with_config(config()).await
    }

    pub(crate) async fn with_config(config: HttpConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("bridge.db")).await.unwrap();
        Self::on_store(config, store, dir, Fake::default())
    }

    /// A new server on an existing store (a restart).
    pub(crate) fn on_store(
        config: HttpConfig,
        store: Store,
        dir: tempfile::TempDir,
        fake: Fake,
    ) -> Self {
        let clock = Arc::new(AtomicI64::new(START));
        let shutdown = CancellationToken::new();
        let mut shared = Shared::new(
            config,
            store.clone(),
            keyring(),
            fake.clone(),
            shutdown.clone(),
        );
        let time = Arc::clone(&clock);
        shared.set_clock(Arc::new(move || {
            Ok(UnixTime::from_secs(time.load(Ordering::SeqCst)))
        }));
        shared.download_wait = std::time::Duration::from_millis(200);
        let server = Server {
            shared: Arc::new(shared),
        };
        let router = server.router();
        Self {
            server,
            router,
            fake,
            clock,
            store,
            shutdown,
            dir,
        }
    }

    pub(crate) fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    pub(crate) async fn send(&self, request: Request<Body>) -> Reply {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        Reply {
            status,
            headers,
            body,
        }
    }

    pub(crate) async fn get(&self, uri: &str) -> Reply {
        self.send(Request::get(uri).body(Body::empty()).unwrap())
            .await
    }
}

/// The query of a valid authorization request, as the web BFF builds it.
pub(crate) fn authorize_query() -> Vec<(&'static str, &'static str)> {
    vec![
        ("client_id", CLIENT_ID),
        ("code_challenge", CHALLENGE),
        ("code_challenge_method", "S256"),
        ("redirect_uri", REDIRECT),
        ("response_type", "code"),
        ("scope", "urn:ietf:params:oauth:scope:mail offline_access"),
        ("state", "the-state"),
    ]
}

pub(crate) fn encode(params: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish()
}

/// `client_secret_basic` as the BFF sends it (form-encoded parts).
pub(crate) fn basic(id: &str, secret: &str) -> String {
    let encode = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    format!(
        "Basic {}",
        BASE64.encode(format!("{}:{}", encode(id), encode(secret)).as_bytes())
    )
}

/// An opened login form: the pending id and the CSRF cookie.
pub(crate) struct OpenForm {
    pub(crate) pending: String,
    pub(crate) cookie: String,
}

impl Harness {
    pub(crate) async fn open_form(&self) -> OpenForm {
        self.open_form_with(&authorize_query()).await
    }

    /// Opens the sign-in form of an authorization request with `params`.
    pub(crate) async fn open_form_with(&self, params: &[(&str, &str)]) -> OpenForm {
        let reply = self
            .get(&format!("/oauth/authorize?{}", encode(params)))
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        let html = reply.text();
        let marker = "name=\"pending\" value=\"";
        let start = html.find(marker).unwrap() + marker.len();
        let pending = html.get(start..start + 43).unwrap().to_owned();
        let cookie = reply
            .header(header::SET_COOKIE)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        OpenForm { pending, cookie }
    }

    pub(crate) async fn post_form(&self, form: &OpenForm, fields: &[(&str, &str)]) -> Reply {
        self.post_form_with(form, &[], fields).await
    }

    /// Posts the form from the client address `ip`.
    pub(crate) async fn post_form_from(
        &self,
        form: &OpenForm,
        ip: &str,
        fields: &[(&str, &str)],
    ) -> Reply {
        self.post_form_with(form, &[(FROM, ip)], fields).await
    }

    /// Posts the form with extra headers.
    pub(crate) async fn post_form_with(
        &self,
        form: &OpenForm,
        headers: &[(&str, &str)],
        fields: &[(&str, &str)],
    ) -> Reply {
        let mut params = vec![("pending", form.pending.as_str())];
        params.extend_from_slice(fields);
        let mut request = Request::post("/oauth/authorize")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, &form.cookie)
            .header("sec-fetch-site", "same-origin");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.send(request.body(Body::from(encode(&params))).unwrap())
            .await
    }

    /// Logs alice in; returns the code.
    pub(crate) async fn code(&self) -> String {
        let form = self.open_form().await;
        let reply = self
            .post_form(
                &form,
                &[
                    ("login", "alice"),
                    ("password", "right password"),
                    ("action", "login"),
                ],
            )
            .await;
        assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
        query(&reply.location())["code"].clone()
    }

    pub(crate) async fn token_request(
        &self,
        authorization: &str,
        params: &[(&str, &str)],
    ) -> Reply {
        self.token_request_with(authorization, &[], params).await
    }

    /// A token request with extra headers.
    pub(crate) async fn token_request_with(
        &self,
        authorization: &str,
        headers: &[(&str, &str)],
        params: &[(&str, &str)],
    ) -> Reply {
        let mut request = Request::post("/oauth/token")
            .header(header::AUTHORIZATION, authorization)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ACCEPT, "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.send(request.body(Body::from(encode(params))).unwrap())
            .await
    }

    pub(crate) async fn exchange(&self, code: &str) -> Reply {
        self.token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[
                ("code", code),
                ("code_verifier", VERIFIER),
                ("grant_type", "authorization_code"),
                ("redirect_uri", REDIRECT),
            ],
        )
        .await
    }

    pub(crate) async fn refresh(&self, refresh_token: &str) -> Reply {
        self.token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ],
        )
        .await
    }

    /// Full login: (access token, refresh token).
    pub(crate) async fn login(&self) -> (String, String) {
        let code = self.code().await;
        let reply = self.exchange(&code).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        let json = reply.json();
        (
            json["access_token"].as_str().unwrap().to_owned(),
            json["refresh_token"].as_str().unwrap().to_owned(),
        )
    }

    pub(crate) async fn with_bearer(
        &self,
        method: &str,
        uri: &str,
        token: &str,
        body: Body,
    ) -> Reply {
        self.send(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(body)
                .unwrap(),
        )
        .await
    }
}
