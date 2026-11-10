// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! JMAP over HTTP (RFC 8620): session, API, blobs and push. Every
//! endpoint needs a Bearer access token from this bridge.

pub(crate) mod api;
pub(crate) mod blob;
pub(crate) mod event_id;
pub(crate) mod eventsource;
pub(crate) mod session;

use axum::body::Body;
use axum::response::{IntoResponse, Response};
use bytes::{Bytes, BytesMut};
use futures::StreamExt as _;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use nebula_crypto::Token;
use nebula_jmap_proto::error::LimitName;
use nebula_jmap_proto::{RequestError, RequestErrorType};
use nebula_store::GrantId;

use crate::backend::{AccountContext, Backend, BackendError};
use crate::error::{HttpError, problem, problem_response};
use crate::oauth::scope::grants_mail;
use crate::oauth::store_hash;
use crate::state::Shared;

/// Why a request has no usable Bearer token (RFC 6750 §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BearerProblem {
    /// No `Authorization`, or one of another scheme: the request has no
    /// Bearer authentication, so the challenge has no error code (RFC
    /// 6750 §3.1: it SHOULD NOT).
    Missing,
    /// `Authorization: Bearer` that isn't one `b64token` (RFC 6750 §2.1),
    /// or several `Authorization` fields: `invalid_request`, 400.
    Malformed,
    /// A token that is unknown, expired, revoked, or whose session can't
    /// be used any more: `invalid_token`, 401.
    Invalid,
}

/// The answer to a request without a usable Bearer token, with its
/// `WWW-Authenticate` challenge (RFC 6750 §3, §3.1). The web client asks
/// its BFF for a fresh token on any 401 (it doesn't read the header).
fn unauthorized(problem_kind: BearerProblem) -> Response {
    let (status, challenge, detail) = match problem_kind {
        BearerProblem::Missing => (
            StatusCode::UNAUTHORIZED,
            "Bearer realm=\"jmap\"",
            "An access token is required.",
        ),
        BearerProblem::Malformed => (
            StatusCode::BAD_REQUEST,
            "Bearer realm=\"jmap\", error=\"invalid_request\"",
            "The Authorization header is malformed.",
        ),
        BearerProblem::Invalid => (
            StatusCode::UNAUTHORIZED,
            "Bearer realm=\"jmap\", error=\"invalid_token\"",
            "A valid access token is required.",
        ),
    };
    let mut response = problem(status, detail);
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static(challenge),
    );
    response
}

/// `b64token` (RFC 6750 §2.1): `1*( ALPHA / DIGIT / "-" / "." / "_" /
/// "~" / "+" / "/" ) *"="`.
fn is_b64token(token: &str) -> bool {
    let body = token.trim_end_matches('=');
    !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/".contains(&b))
}

/// The Bearer token of a request (RFC 6750 §2.1: `Bearer 1*SP b64token`).
fn bearer_token(headers: &HeaderMap) -> Result<&str, BearerProblem> {
    let mut fields = headers.get_all(header::AUTHORIZATION).iter();
    let Some(field) = fields.next() else {
        return Err(BearerProblem::Missing);
    };
    // RFC 6750 §3.1: more than one way of sending a token is invalid.
    if fields.next().is_some() {
        return Err(BearerProblem::Malformed);
    }
    let text = field.to_str().map_err(|_| BearerProblem::Malformed)?.trim();
    let (scheme, token) = text.split_once(' ').unwrap_or((text, ""));
    if !scheme.eq_ignore_ascii_case("bearer") {
        // Another scheme (Basic…) carries no Bearer token.
        return Err(BearerProblem::Missing);
    }
    let token = token.trim_start_matches(' ');
    if is_b64token(token) {
        Ok(token)
    } else {
        Err(BearerProblem::Malformed)
    }
}

/// The `Retry-After` of a 503 (busy or unreachable mail server, or the
/// bridge busy with other large messages): the same 5 s as Stalwart, short
/// enough for `jmap-client` to retry (it waits 10 s at most).
pub(crate) const UNAVAILABLE_RETRY_AFTER: HeaderValue = HeaderValue::from_static("5");

/// A problem for an internal failure (logged, no details sent). A 503 says
/// when to try again.
pub(crate) fn internal(error: &HttpError) -> Response {
    let status = error.log();
    let mut response = problem(status, "The server could not complete the request.");
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, UNAVAILABLE_RETRY_AFTER);
    }
    response
}

/// 404 for anything the account can't see, including other accounts' ids
/// (no difference between "not yours" and "doesn't exist").
pub(crate) fn not_found() -> Response {
    problem(StatusCode::NOT_FOUND, "Not found.")
}

/// A request-level JMAP error as problem details.
pub(crate) fn request_error(error: &RequestError) -> Response {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::BAD_REQUEST);
    problem_response(status, error)
}

/// A JMAP `limit` problem.
pub(crate) fn limit_error(limit: LimitName, detail: &str) -> Response {
    request_error(&RequestError::new(RequestErrorType::Limit(limit), detail))
}

/// The account of the request's Bearer token, started in the backend.
pub(crate) async fn authenticate<B: Backend>(
    shared: &Shared<B>,
    headers: &HeaderMap,
) -> Result<AccountContext, Box<Response>> {
    authenticate_session(shared, headers)
        .await
        .map(|(account, _)| account)
}

/// Like [`authenticate`], with the token's login session.
pub(crate) async fn authenticate_session<B: Backend>(
    shared: &Shared<B>,
    headers: &HeaderMap,
) -> Result<(AccountContext, GrantId), Box<Response>> {
    let text = bearer_token(headers).map_err(|kind| Box::new(unauthorized(kind)))?;
    // Well-formed, but not one of this bridge's tokens: unknown.
    let token = Token::parse(text).map_err(|_| Box::new(unauthorized(BearerProblem::Invalid)))?;
    let (account, grant) = lookup(shared, &token)
        .await
        .map_err(|e| Box::new(internal(&e)))?
        .ok_or_else(|| Box::new(unauthorized(BearerProblem::Invalid)))?;
    match shared.ensure_started(&account).await {
        Ok(context) => Ok((context, grant)),
        // The session exists but its password is gone: log in again (also if
        // the mail server refused it while starting: the sessions were
        // just ended).
        Err(HttpError::NoSecret | HttpError::Backend(BackendError::PasswordRefused)) => {
            Err(Box::new(unauthorized(BearerProblem::Invalid)))
        }
        Err(error) => Err(Box::new(internal(&error))),
    }
}

async fn lookup<B: Backend>(
    shared: &Shared<B>,
    token: &Token,
) -> Result<Option<(nebula_store::Account, GrantId)>, HttpError> {
    let now = shared.now()?;
    let Some(owner) = shared.store.access_token(store_hash(token), now).await? else {
        return Ok(None);
    };
    if !grants_mail(&owner.scope) {
        return Ok(None);
    }
    Ok(shared
        .store
        .account(owner.account)
        .await?
        .map(|account| (account, owner.grant)))
}

/// A request body larger than allowed, or one that failed to arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BodyError {
    TooLarge,
    Read,
}

/// The request's `Content-Length`, if it has a valid one.
pub(crate) fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

/// Reads a body of at most `limit` bytes, failing as soon as it grows past
/// it (a declared `Content-Length` above the limit fails at once).
pub(crate) async fn read_limited(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<Bytes, BodyError> {
    if declared_length(headers)
        .is_some_and(|length| u64::try_from(limit).is_ok_and(|limit| length > limit))
    {
        return Err(BodyError::TooLarge);
    }
    let mut stream = body.into_data_stream();
    let mut buffer = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| BodyError::Read)?;
        if buffer.len().saturating_add(chunk.len()) > limit {
            return Err(BodyError::TooLarge);
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(buffer.freeze())
}

/// 400 for a body that broke off.
pub(crate) fn body_read_error() -> Response {
    (StatusCode::BAD_REQUEST, "request body could not be read").into_response()
}
