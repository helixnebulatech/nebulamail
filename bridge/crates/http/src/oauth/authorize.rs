// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `GET`/`POST /oauth/authorize`: validate the request, show the login
//! form, check the credentials against the mail server and redirect back
//! with a code (RFC 6749 §4.1, RFC 7636, RFC 9207).

use std::net::IpAddr;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Redirect, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use nebula_crypto::{CodeChallenge, Token};
use nebula_store::{AccountId, NewCode, UnixTime};
use secrecy::SecretString;

use crate::backend::{Backend, LoginAlert, LoginName, LoginOutcome, parse_password};
use crate::config::{Lifetimes, RedirectUri};
use crate::error::{HttpError, no_store};
use crate::form::Form;
use crate::limits::{LoginBucket, client_ip};
use crate::oauth::page::{ErrorPage, FormError, Lang, LoginForm, error_page, login_form};
use crate::oauth::pending::Pending;
use crate::oauth::scope::Scope;
use crate::oauth::store_hash;
use crate::state::Shared;

/// Largest login form body.
const MAX_FORM_BYTES: usize = 16 * 1024;
/// Longest `state` we echo back.
const MAX_STATE_LEN: usize = 512;
/// How long the browser keeps its CSRF cookie.
const CSRF_COOKIE_MAX_AGE: u64 = 3600;

/// OAuth errors sent back to the client's redirect URI (RFC 6749 §4.1.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedirectError {
    InvalidRequest,
    UnsupportedResponseType,
    AccessDenied,
    ServerError,
}

impl RedirectError {
    fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnsupportedResponseType => "unsupported_response_type",
            Self::AccessDenied => "access_denied",
            Self::ServerError => "server_error",
        }
    }
}

/// A 303 to the client with `params` plus `iss` (RFC 9207) added to its
/// redirect URI.
fn redirect<B>(shared: &Shared<B>, uri: &RedirectUri, params: &[(&str, &str)]) -> Response {
    let mut url = uri.url().clone();
    {
        let mut query = url.query_pairs_mut();
        for (name, value) in params {
            query.append_pair(name, value);
        }
        query.append_pair("iss", shared.config.public_url().issuer());
    }
    no_store(Redirect::to(url.as_str()).into_response())
}

fn redirect_error<B>(
    shared: &Shared<B>,
    uri: &RedirectUri,
    error: RedirectError,
    state: Option<&str>,
) -> Response {
    match state {
        Some(state) => redirect(shared, uri, &[("error", error.code()), ("state", state)]),
        None => redirect(shared, uri, &[("error", error.code())]),
    }
}

/// The CSRF cookie's name: `__Host-` needs `Secure`, so only over https.
fn cookie_name(https: bool) -> &'static str {
    if https {
        "__Host-nebula-authz"
    } else {
        "nebula-authz"
    }
}

/// The known-browser cookie's name (see [`remember_device`]).
fn device_cookie_name(https: bool) -> &'static str {
    if https {
        "__Host-nebula-device"
    } else {
        "nebula-device"
    }
}

/// The CSRF cookie sent by the browser, if well formed.
fn read_cookie(headers: &HeaderMap, https: bool) -> Option<Token> {
    read_named_cookie(headers, cookie_name(https))
}

/// A token cookie sent by the browser, if well formed.
fn read_named_cookie(headers: &HeaderMap, name: &str) -> Option<Token> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| Token::parse(value).ok())
}

fn set_cookie(token: &Token, https: bool) -> Option<HeaderValue> {
    let secure = if https { "; Secure" } else { "" };
    let value = format!(
        "{}={}; Path=/; Max-Age={CSRF_COOKIE_MAX_AGE}; HttpOnly; SameSite=Strict{secure}",
        cookie_name(https),
        token.expose()
    );
    HeaderValue::from_str(&value).ok()
}

/// The checks after the client and redirect URI are known; failures go
/// back to the client.
fn validate(form: &Form) -> Result<(CodeChallenge, Scope), RedirectError> {
    match form.get("response_type") {
        Some("code") => {}
        Some(_) => return Err(RedirectError::UnsupportedResponseType),
        None => return Err(RedirectError::InvalidRequest),
    }
    // OAuth 2.1: PKCE is required, and `plain` is not accepted.
    if form.get("code_challenge_method") != Some("S256") {
        return Err(RedirectError::InvalidRequest);
    }
    let challenge = form
        .get("code_challenge")
        .and_then(|text| CodeChallenge::parse(text).ok())
        .ok_or(RedirectError::InvalidRequest)?;
    Ok((challenge, Scope::granted(form.get("scope"))))
}

/// `GET /oauth/authorize`: shows the login form.
pub(crate) async fn authorize_get<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    request: Request,
) -> Response {
    let headers = request.headers();
    let lang = Lang::from_headers(headers);
    let query = request.uri().query().unwrap_or_default();
    // Until the redirect URI is validated, errors are a page, never a
    // redirect: otherwise this would be an open redirector.
    let Ok(form) = Form::parse(query.as_bytes()) else {
        return error_page(lang, ErrorPage::InvalidRequest);
    };
    let Some(client) = form
        .get("client_id")
        .and_then(|id| shared.config.client(id))
    else {
        return error_page(lang, ErrorPage::InvalidRequest);
    };
    let Some(redirect_uri) = form
        .get("redirect_uri")
        .and_then(|uri| client.redirect_uri(uri))
    else {
        return error_page(lang, ErrorPage::InvalidRequest);
    };
    // `state` is RECOMMENDED, not required (RFC 6749 §4.1.1): PKCE, which
    // is required here, already stops CSRF on the redirect URI (RFC 9700
    // §2.1, §4.7.1). An empty one is as if absent (RFC 6749 §3.1). One
    // longer than we echo back is refused (a server limit).
    let state = form.get("state").filter(|s| !s.is_empty());
    if state.is_some_and(|s| s.len() > MAX_STATE_LEN) {
        return redirect_error(&shared, redirect_uri, RedirectError::InvalidRequest, None);
    }
    let (challenge, scope) = match validate(&form) {
        Ok(valid) => valid,
        Err(error) => return redirect_error(&shared, redirect_uri, error, state),
    };
    let Ok(now) = shared.now() else {
        return redirect_error(&shared, redirect_uri, RedirectError::ServerError, state);
    };

    let https = shared.config.public_url().is_https();
    // A browser that already has the cookie keeps its token, but the
    // cookie is sent again so it outlives this form (its `Max-Age` starts
    // over).
    let Ok(csrf) = read_cookie(headers, https).map_or_else(Token::generate, Ok) else {
        return redirect_error(&shared, redirect_uri, RedirectError::ServerError, state);
    };
    let pending = Pending::new(
        client.id().clone(),
        redirect_uri.clone(),
        state.map(str::to_owned),
        challenge,
        scope,
        csrf.hash(),
        now,
    );
    let ip = client_ip(shared.config.client_ip, headers, request.extensions());
    let Ok(id) = shared.pending.insert(pending, ip, now) else {
        return error_page(lang, ErrorPage::Busy);
    };
    let mut response = login_form(
        StatusCode::OK,
        &LoginForm {
            lang,
            pending: id.expose(),
            login: "",
            error: None,
            alert: None,
            redirect_origin: &redirect_uri.origin(),
        },
    );
    if let Some(cookie) = set_cookie(&csrf, https) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

/// A form post the browser says came from another site. Browsers send
/// `Origin: null` here because of `Referrer-Policy: no-referrer`, so
/// `Sec-Fetch-Site` is the header to check; the CSRF cookie is the real
/// protection.
fn is_cross_site(headers: &HeaderMap) -> bool {
    headers
        .get("sec-fetch-site")
        .is_some_and(|site| site != "same-origin")
}

/// `POST /oauth/authorize`: the login form was submitted.
pub(crate) async fn authorize_post<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    request: Request,
) -> Response {
    let lang = Lang::from_headers(request.headers());
    if is_cross_site(request.headers()) {
        return error_page(lang, ErrorPage::Expired);
    }
    let https = shared.config.public_url().is_https();
    let csrf = read_cookie(request.headers(), https);
    let device = read_named_cookie(request.headers(), device_cookie_name(https));
    let ip = client_ip(
        shared.config.client_ip,
        request.headers(),
        request.extensions(),
    );
    let Ok(body) = to_bytes(request.into_body(), MAX_FORM_BYTES).await else {
        return error_page(lang, ErrorPage::InvalidRequest);
    };
    let Ok(form) = Form::parse(&body) else {
        return error_page(lang, ErrorPage::InvalidRequest);
    };
    let Ok(now) = shared.now() else {
        return error_page(lang, ErrorPage::Internal);
    };
    let Some(id) = form.get("pending").and_then(|text| Token::parse(text).ok()) else {
        return error_page(lang, ErrorPage::Expired);
    };
    let Some(pending) = shared.pending.get(&id, now) else {
        return error_page(lang, ErrorPage::Expired);
    };
    // The form must be posted by the browser that opened it.
    if !csrf.is_some_and(|token| pending.csrf.matches(&token)) {
        return error_page(lang, ErrorPage::Expired);
    }
    match form.get("action") {
        Some("deny") => {
            shared.pending.remove(&id);
            return redirect_error(
                &shared,
                &pending.redirect_uri,
                RedirectError::AccessDenied,
                pending.state.as_deref(),
            );
        }
        Some("login") => {}
        _ => return error_page(lang, ErrorPage::InvalidRequest),
    }

    let typed_login = form.get("login").unwrap_or_default();
    let show_form = |status: StatusCode, error: FormError, alert: Option<&LoginAlert>| {
        login_form(
            status,
            &LoginForm {
                lang,
                pending: id.expose(),
                login: typed_login,
                error: Some(error),
                alert: alert.map(LoginAlert::as_str),
                redirect_origin: &pending.redirect_uri.origin(),
            },
        )
    };
    let credentials = typed_login.parse::<LoginName>().and_then(|login| {
        parse_password(form.get("password").unwrap_or_default()).map(|password| (login, password))
    });
    let Ok((login, password)) = credentials else {
        return show_form(StatusCode::BAD_REQUEST, FormError::Missing, None);
    };

    let (email, bucket) =
        match check_login(&shared, ip, device.as_ref(), &login, &password, now).await {
            Ok(found) => found,
            Err((status, error, alert)) => return show_form(status, error, alert.as_ref()),
        };

    match issue_code(&shared, &login, password, &email, &pending).await {
        Ok((code, account)) => {
            // Only now: a login that matches another spelling and fails
            // its check must stay counted as a failure.
            shared.limiter.succeeded(ip, &bucket);
            shared.pending.remove(&id);
            // RFC 6749 §4.1.2: `state` only if the request had one.
            let mut params = vec![("code", code.expose())];
            if let Some(state) = &pending.state {
                params.push(("state", state));
            }
            let mut response = redirect(&shared, &pending.redirect_uri, &params);
            if let Some(cookie) = remember_device(&shared, account, device, https).await {
                response.headers_mut().append(header::SET_COOKIE, cookie);
            }
            response
        }
        Err(error) => match failed_login(&shared, ip, &bucket, &error) {
            Some((status, form_error)) => show_form(status, form_error, None),
            None => error_page(lang, ErrorPage::Internal),
        },
    }
}

/// What a login shows when it fails after the mail server accepted the
/// password, and how the attempt counts. `None`: the internal error page.
fn failed_login<B: Backend>(
    shared: &Shared<B>,
    ip: IpAddr,
    bucket: &LoginBucket,
    error: &HttpError,
) -> Option<(StatusCode, FormError)> {
    // The same answer as a wrong password, and the attempt counted by
    // `begin` stays a failure.
    if matches!(error, HttpError::LoginMismatch) {
        tracing::info!("login failed: wrong credentials for the stored spelling");
        return Some((StatusCode::BAD_REQUEST, FormError::WrongCredentials));
    }
    match error.log() {
        StatusCode::SERVICE_UNAVAILABLE => {
            shared.limiter.not_counted(bucket);
            Some((StatusCode::SERVICE_UNAVAILABLE, FormError::Unavailable))
        }
        // Accepted by `verify_login`, refused right after when starting:
        // counted.
        StatusCode::UNAUTHORIZED => Some((StatusCode::BAD_REQUEST, FormError::WrongCredentials)),
        // The server accepted the password: as before, not a failure of
        // the user.
        _ => {
            shared.limiter.succeeded(ip, bucket);
            None
        }
    }
}

/// The per-login window of this attempt: the browser's own if its cookie
/// says it logged in to this login before, the login's shared one if not.
async fn login_bucket<B: Backend>(
    shared: &Shared<B>,
    device: Option<&Token>,
    login: &LoginName,
    now: UnixTime,
) -> LoginBucket {
    let Some(token) = device else {
        return LoginBucket::login(login.as_str());
    };
    let hash = store_hash(token);
    match shared
        .store
        .is_known_device(hash, login.as_str(), now)
        .await
    {
        Ok(true) => LoginBucket::device(hash, login.as_str()),
        Ok(false) => LoginBucket::login(login.as_str()),
        Err(error) => {
            tracing::warn!(%error, "known browser check failed");
            LoginBucket::login(login.as_str())
        }
    }
}

/// Checks the credentials with the mail server, within the login limits.
/// The address of the account and the window to tell of a success, or
/// what to show in the form (with the server's alert, if it sent one).
async fn check_login<B: Backend>(
    shared: &Shared<B>,
    ip: IpAddr,
    device: Option<&Token>,
    login: &LoginName,
    password: &SecretString,
    now: UnixTime,
) -> Result<(String, LoginBucket), (StatusCode, FormError, Option<LoginAlert>)> {
    let bucket = login_bucket(shared, device, login, now).await;
    if shared.limiter.begin(ip, &bucket, now).is_err() {
        tracing::warn!("login refused: too many failed attempts");
        return Err((StatusCode::TOO_MANY_REQUESTS, FormError::RateLimited, None));
    }
    match shared.backend.verify_login(login, password).await {
        // Counted as a success only once the login is complete.
        LoginOutcome::Success { email } => Ok((email, bucket)),
        LoginOutcome::WrongCredentials { alert } => {
            tracing::info!("login failed: wrong credentials");
            Err((StatusCode::BAD_REQUEST, FormError::WrongCredentials, alert))
        }
        LoginOutcome::Unavailable => {
            shared.limiter.not_counted(&bucket);
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                FormError::Unavailable,
                None,
            ))
        }
        // An answer that isn't a known credentials failure: it counts, so
        // an unusual server reply can't give free tries.
        LoginOutcome::Rejected => {
            tracing::warn!("login refused by the mail server for another reason");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                FormError::Unavailable,
                None,
            ))
        }
    }
}

/// Stores the account and password, then a single-use code (only its
/// hash is stored).
async fn issue_code<B: Backend>(
    shared: &Shared<B>,
    login: &LoginName,
    password: SecretString,
    email: &str,
    pending: &Pending,
) -> Result<(Token, AccountId), HttpError> {
    let (account, _in_progress) = shared.complete_login(login, password, email).await?;
    let code = Token::generate()?;
    let now = shared.now()?;
    shared
        .store
        .insert_code(NewCode {
            hash: store_hash(&code),
            account: account.id,
            client_id: pending.client_id.as_str(),
            redirect_uri: pending.redirect_uri.as_str(),
            code_challenge: &pending.code_challenge.to_text(),
            scope: pending.scope.as_str(),
            expires_at: now.saturating_add(Lifetimes::CODE),
        })
        .await?;
    tracing::info!(account = account.id.get(), "login succeeded");
    Ok((code, account.id))
}

/// Remembers this browser as known for `account`: the
/// cookie it already has, so one browser is known for every account it
/// logs in to, or a new one. The `Set-Cookie` for it, if all went well; a
/// failure here doesn't fail the login.
async fn remember_device<B: Backend>(
    shared: &Shared<B>,
    account: AccountId,
    device: Option<Token>,
    https: bool,
) -> Option<HeaderValue> {
    let token = match device {
        Some(token) => token,
        None => Token::generate().ok()?,
    };
    let now = shared.now().ok()?;
    let lifetime = shared.config.lifetimes.session();
    let stored = shared
        .store
        .remember_device(
            account,
            store_hash(&token),
            now,
            now.saturating_add(lifetime),
        )
        .await;
    if let Err(error) = stored {
        tracing::warn!(%error, "known browser not stored");
        return None;
    }
    let secure = if https { "; Secure" } else { "" };
    let value = format!(
        "{}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict{secure}",
        device_cookie_name(https),
        token.expose(),
        lifetime.as_secs()
    );
    HeaderValue::from_str(&value).ok()
}
