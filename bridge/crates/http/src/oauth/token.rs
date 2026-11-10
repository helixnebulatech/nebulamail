// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `POST /oauth/token` (RFC 6749 §4.1.3, §6): codes and refresh tokens.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::request::Parts;
use http::{HeaderMap, HeaderValue, header};
use nebula_crypto::{CodeChallenge, CodeVerifier, Token, verify_s256};
use nebula_store::{CodeOutcome, NewGrant, NewTokens, RefreshOutcome, UnixTime};
use serde::Serialize;

use crate::backend::Backend;
use crate::config::OAuthClient;
use crate::error::{
    HttpError, OAuthErrorCode, no_store, oauth_error, oauth_internal, oauth_unavailable,
};
use crate::form::Form;
use crate::limits::client_ip;
use crate::oauth::client_auth::ClientAuthError;
use crate::oauth::store_hash;
use crate::state::{SessionEnd, Shared};

/// A successful token response (RFC 6749 §5.1).
#[derive(Serialize)]
struct TokenResponse<'a> {
    access_token: &'a str,
    token_type: &'static str,
    expires_in: u64,
    refresh_token: &'a str,
    scope: &'a str,
}

/// A new access and refresh token, and when they expire.
pub(crate) struct Issued {
    access: Token,
    refresh: Token,
    pub(crate) hashes: NewTokens,
}

/// New tokens starting at `now`; nothing lives past `session_end`.
pub(crate) fn issue<B>(
    shared: &Shared<B>,
    now: UnixTime,
    session_end: UnixTime,
) -> Result<Issued, HttpError> {
    let lifetimes = shared.config.lifetimes;
    let access = Token::generate()?;
    let refresh = Token::generate()?;
    let access_expires_at = now
        .saturating_add(lifetimes.access_token())
        .min(session_end);
    let hashes = NewTokens {
        access: store_hash(&access),
        access_expires_at,
        refresh: store_hash(&refresh),
        refresh_expires_at: now
            .saturating_add(lifetimes.refresh_token())
            .min(session_end),
    };
    Ok(Issued {
        access,
        refresh,
        hashes,
    })
}

/// The token response. `access_expires_at` is when the stored access token
/// really expires (a refresh near the session's end gets a shorter one).
fn respond(issued: &Issued, access_expires_at: UnixTime, now: UnixTime, scope: &str) -> Response {
    let expires_in =
        u64::try_from(access_expires_at.secs().saturating_sub(now.secs())).unwrap_or(0);
    no_store(
        Json(TokenResponse {
            access_token: issued.access.expose(),
            token_type: "Bearer",
            expires_in,
            refresh_token: issued.refresh.expose(),
            scope,
        })
        .into_response(),
    )
}

/// Whether the body is declared as a form (RFC 6749 §4.1.3).
pub(crate) fn is_form(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|essence| {
            essence
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
}

/// The answer to a failed client check. Busy and limited are a 503 with
/// `Retry-After`: the BFF keeps its sessions on a 5xx, and ends them on a
/// 4xx with an `error`.
fn client_auth_error(error: ClientAuthError) -> Response {
    match error {
        ClientAuthError::Invalid => {
            tracing::warn!("OAuth client authentication failed");
            oauth_error(OAuthErrorCode::InvalidClient)
        }
        ClientAuthError::Busy | ClientAuthError::Limited => {
            let wait = match error {
                ClientAuthError::Limited => "60",
                _ => "1",
            };
            oauth_unavailable(HeaderValue::from_static(wait))
        }
    }
}

/// Client authentication and form parsing shared with revocation.
pub(crate) async fn authenticated_form<'s, B: Backend>(
    shared: &'s Shared<B>,
    parts: &Parts,
    body: &Bytes,
) -> Result<(&'s OAuthClient, Form), Box<Response>> {
    let invalid_request = || Box::new(oauth_error(OAuthErrorCode::InvalidRequest));
    if !is_form(&parts.headers) {
        return Err(invalid_request());
    }
    let ip = client_ip(shared.config.client_ip, &parts.headers, &parts.extensions);
    let now = shared
        .now()
        .map_err(|error| Box::new(oauth_internal(&error)))?;
    let client = shared
        .client_auth
        .authenticate(&shared.config, &parts.headers, ip, now)
        .await
        .map_err(|error| Box::new(client_auth_error(error)))?;
    let form = Form::parse(body).map_err(|_| invalid_request())?;
    // A `client_id` in the body, if any, must be the authenticated one.
    if form
        .get("client_id")
        .is_some_and(|id| id != client.id().as_str())
    {
        return Err(invalid_request());
    }
    Ok((client, form))
}

/// `POST /oauth/token`.
pub(crate) async fn token<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    parts: Parts,
    body: Bytes,
) -> Response {
    let (client, form) = match authenticated_form(&shared, &parts, &body).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    let result = match form.get("grant_type") {
        Some("authorization_code") => authorization_code(&shared, client, &form).await,
        Some("refresh_token") => refresh_token(&shared, client, &form).await,
        Some(_) => Ok(oauth_error(OAuthErrorCode::UnsupportedGrantType)),
        None => Ok(oauth_error(OAuthErrorCode::InvalidRequest)),
    };
    result.unwrap_or_else(|error| oauth_internal(&error))
}

async fn authorization_code<B: Backend>(
    shared: &Shared<B>,
    client: &OAuthClient,
    form: &Form,
) -> Result<Response, HttpError> {
    let (Some(code), Some(verifier)) = (form.get("code"), form.get("code_verifier")) else {
        return Ok(oauth_error(OAuthErrorCode::InvalidRequest));
    };
    let Ok(code) = Token::parse(code) else {
        return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
    };
    let now = shared.now()?;
    let code_hash = store_hash(&code);
    // Taking the code marks it used: whatever happens next, it works once.
    let stored = match shared.store.take_code(code_hash, now).await? {
        CodeOutcome::Valid(stored) => stored,
        CodeOutcome::Reused { account } => {
            // RFC 6749 §4.1.2: the tokens issued from it are revoked.
            tracing::warn!("authorization code reused; its session is revoked");
            shared
                .stop_if_idle(account, SessionEnd::Compromised)
                .await?;
            return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
        }
        CodeOutcome::Invalid => {
            tracing::warn!("unknown or expired authorization code");
            return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
        }
    };
    if stored.client_id != client.id().as_str()
        || form.get("redirect_uri") != Some(stored.redirect_uri.as_str())
    {
        return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
    }
    let challenge = CodeChallenge::parse(&stored.code_challenge)
        .map_err(|_| HttpError::Corrupt("code challenge"))?;
    let pkce_ok = CodeVerifier::parse(verifier)
        .is_ok_and(|verifier| verify_s256(&verifier, &challenge).is_ok());
    if !pkce_ok {
        tracing::warn!("PKCE verification failed");
        return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
    }

    let session_end = now.saturating_add(shared.config.lifetimes.session());
    let issued = issue(shared, now, session_end)?;
    let grant = NewGrant {
        account: stored.account,
        client_id: client.id().as_str(),
        scope: &stored.scope,
        now,
        expires_at: session_end,
    };
    let created = shared
        .store
        .create_grant_for_code(code_hash, grant, issued.hashes)
        .await?;
    if created.is_none() {
        // Presented again while this exchange ran: no session at all.
        tracing::warn!("authorization code reused during its exchange");
        return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
    }
    Ok(respond(
        &issued,
        issued.hashes.access_expires_at,
        now,
        &stored.scope,
    ))
}

async fn refresh_token<B: Backend>(
    shared: &Shared<B>,
    client: &OAuthClient,
    form: &Form,
) -> Result<Response, HttpError> {
    let Some(refresh) = form.get("refresh_token") else {
        return Ok(oauth_error(OAuthErrorCode::InvalidRequest));
    };
    let Ok(refresh) = Token::parse(refresh) else {
        return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
    };
    let now = shared.now()?;
    // The store caps the new tokens at the grant's end.
    let issued = issue(shared, now, UnixTime::from_secs(i64::MAX))?;
    match shared
        .store
        .rotate_refresh_token(store_hash(&refresh), issued.hashes, now)
        .await?
    {
        RefreshOutcome::Rotated(rotated) => {
            let owner = rotated.owner;
            if owner.client_id != client.id().as_str() {
                // Another client holds this session's token: stolen.
                tracing::warn!("refresh token used by another client; session revoked");
                shared.store.revoke_grant(owner.grant, now).await?;
                shared
                    .stop_if_idle(owner.account, SessionEnd::Compromised)
                    .await?;
                return Ok(oauth_error(OAuthErrorCode::InvalidGrant));
            }
            Ok(respond(
                &issued,
                rotated.access_expires_at,
                now,
                &owner.scope,
            ))
        }
        // Another tab rotated it a moment ago. A 5xx makes the BFF keep the
        // session (it already has the new token from the other request).
        RefreshOutcome::Raced => Ok(oauth_unavailable(HeaderValue::from_static("1"))),
        RefreshOutcome::Reused(owner) => {
            tracing::warn!("old refresh token reused; session revoked");
            // The token may be stolen: delete the password now, not at the
            // next cleanup, if this was the account's last session (held
            // messages don't keep it).
            shared
                .stop_if_idle(owner.account, SessionEnd::Compromised)
                .await?;
            Ok(oauth_error(OAuthErrorCode::InvalidGrant))
        }
        RefreshOutcome::Invalid => Ok(oauth_error(OAuthErrorCode::InvalidGrant)),
    }
}
