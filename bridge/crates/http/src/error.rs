// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Internal errors and the JSON error bodies.

use axum::Json;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use nebula_crypto::CryptoError;
use nebula_store::StoreError;
use serde::Serialize;

use crate::backend::BackendError;

/// Something failed inside the server. No variant carries user data, so
/// they are safe to log; none is shown to clients.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpError {
    /// The database failed.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// Encryption or token handling failed.
    #[error("crypto: {0}")]
    Crypto(#[from] CryptoError),
    /// The backend failed.
    #[error("backend: {0}")]
    Backend(#[from] BackendError),
    /// A session whose password is gone (should not happen: passwords are
    /// only deleted when the last session ends).
    #[error("the account has no stored password")]
    NoSecret,
    /// A stored value is not what it should be.
    #[error("invalid stored value: {0}")]
    Corrupt(&'static str),
    /// The login matches an account stored with other case (`Alice` and
    /// `alice`), and the password does not work for the stored spelling:
    /// the server may tell them apart, so they are not taken as one. Shown
    /// as wrong credentials, never as "this account exists".
    #[error("the login does not match the stored account")]
    LoginMismatch,
}

impl HttpError {
    /// The status to answer with: 503 if trying later may help.
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::Backend(BackendError::Unavailable) => StatusCode::SERVICE_UNAVAILABLE,
            // The sessions were ended: log in again.
            Self::Backend(BackendError::PasswordRefused) => StatusCode::UNAUTHORIZED,
            Self::Store(_)
            | Self::Crypto(_)
            | Self::Backend(_)
            | Self::NoSecret
            | Self::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::LoginMismatch => StatusCode::BAD_REQUEST,
        }
    }

    /// Logs the error (it has no user data) and returns its status.
    pub(crate) fn log(&self) -> StatusCode {
        tracing::error!(error = %self, "request failed");
        self.status()
    }
}

/// `Cache-Control: no-store` and `Pragma: no-cache`, for anything that
/// carries tokens (RFC 6749 §5.1).
pub(crate) fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// An error code of the token and revocation endpoints: the closed list
/// of RFC 6749 §5.2 (RFC 7009 §2.2.1 uses the same, plus
/// `unsupported_token_type`, never needed: both token types can be
/// revoked). All are the client's fault (400, or 401 for
/// `invalid_client`).
///
/// `server_error` and `temporarily_unavailable` are deliberately absent:
/// RFC 6749 defines them only for the authorization endpoint's redirect
/// (§4.1.2.1), they are not registered for the token endpoint (§8.5,
/// §11.4) and a server failure is not a §5.2 error response. The token and
/// revocation endpoints answer it with the HTTP status alone
/// ([`oauth_unavailable`], [`oauth_internal`]), as RFC 7009 §2.2.1 does
/// for 503.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OAuthErrorCode {
    InvalidRequest,
    InvalidClient,
    InvalidGrant,
    UnsupportedGrantType,
}

impl OAuthErrorCode {
    pub(crate) fn status(self) -> StatusCode {
        match self {
            Self::InvalidClient => StatusCode::UNAUTHORIZED,
            Self::InvalidRequest | Self::InvalidGrant | Self::UnsupportedGrantType => {
                StatusCode::BAD_REQUEST
            }
        }
    }
}

#[derive(Serialize)]
struct OAuthErrorBody {
    error: OAuthErrorCode,
}

/// A JSON OAuth error. The web BFF ends the session on a 4xx with an
/// `error` and keeps it on a 5xx, so the status matters.
pub(crate) fn oauth_error(code: OAuthErrorCode) -> Response {
    let mut response = (code.status(), Json(OAuthErrorBody { error: code })).into_response();
    if code == OAuthErrorCode::InvalidClient {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"oauth\""),
        );
    }
    no_store(response)
}

/// "Try again later" from an OAuth JSON endpoint: 503 with `Retry-After`
/// (RFC 9110 §15.6.4, §10.2.3; RFC 7009 §2.2.1: the token still exists)
/// and a problem details body without an OAuth `error` code (see
/// [`OAuthErrorCode`]), `no-store`.
pub(crate) fn oauth_unavailable(retry_after: HeaderValue) -> Response {
    let mut response = problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "The server is busy; try again later.",
    );
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, retry_after);
    no_store(response)
}

/// The answer of an OAuth JSON endpoint to an internal failure: 503 if
/// trying later may help, else 500, never an OAuth `error` code.
pub(crate) fn oauth_internal(error: &HttpError) -> Response {
    match error.log() {
        StatusCode::SERVICE_UNAVAILABLE => oauth_unavailable(crate::jmap::UNAVAILABLE_RETRY_AFTER),
        status => no_store(problem(status, "Internal server error.")),
    }
}

/// A problem details body (RFC 7807) for non-JMAP-specific errors.
#[derive(Serialize)]
struct Problem<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    status: u16,
    detail: &'a str,
}

/// `application/problem+json` with `about:blank` as type.
pub(crate) fn problem(status: StatusCode, detail: &str) -> Response {
    problem_response(
        status,
        &Problem {
            kind: "about:blank",
            status: status.as_u16(),
            detail,
        },
    )
}

/// Any serializable problem body with the right content type.
pub(crate) fn problem_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(nebula_jmap_proto::error::PROBLEM_JSON_CONTENT_TYPE),
    );
    response
}
