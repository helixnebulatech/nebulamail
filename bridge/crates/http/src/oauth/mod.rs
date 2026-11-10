// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The bridge's own OAuth 2.1 authorization server
//! code + PKCE S256, confidential clients from the configuration,
//! rotating refresh tokens, revocation.

pub(crate) mod authorize;
pub(crate) mod client_auth;
pub(crate) mod page;
pub(crate) mod pending;
pub(crate) mod revoke;
pub(crate) mod scope;
pub(crate) mod token;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, header};
use nebula_crypto::Token;
use serde::Serialize;

use crate::backend::Backend;
use crate::state::Shared;

/// The store's form of a token hash.
pub(crate) fn store_hash(token: &Token) -> nebula_store::TokenHash {
    nebula_store::TokenHash(*token.hash().as_bytes())
}

/// Authorization server metadata (RFC 8414 §2), at its well-known path
/// `oauth-authorization-server` (RFC 8414 §3).
///
/// Not at `openid-configuration`: the bridge is not an `OpenID` Connect
/// provider (no ID tokens), and OIDC Discovery §3 requires fields it can't
/// give (`jwks_uri`…). The web BFF reads RFC 8414's path first.
#[derive(Serialize)]
struct Metadata<'a> {
    issuer: &'a str,
    authorization_endpoint: String,
    token_endpoint: String,
    revocation_endpoint: String,
    response_types_supported: [&'a str; 1],
    response_modes_supported: [&'a str; 1],
    grant_types_supported: [&'a str; 2],
    code_challenge_methods_supported: [&'a str; 1],
    token_endpoint_auth_methods_supported: [&'a str; 1],
    revocation_endpoint_auth_methods_supported: [&'a str; 1],
    scopes_supported: [&'a str; 2],
    authorization_response_iss_parameter_supported: bool,
}

/// `GET /.well-known/oauth-authorization-server`.
pub(crate) async fn discovery<B: Backend>(State(shared): State<Arc<Shared<B>>>) -> Response {
    let url = shared.config.public_url();
    let metadata = Metadata {
        issuer: url.issuer(),
        authorization_endpoint: url.join("/oauth/authorize"),
        token_endpoint: url.join("/oauth/token"),
        revocation_endpoint: url.join("/oauth/revoke"),
        response_types_supported: ["code"],
        response_modes_supported: ["query"],
        grant_types_supported: ["authorization_code", "refresh_token"],
        code_challenge_methods_supported: ["S256"],
        token_endpoint_auth_methods_supported: ["client_secret_basic"],
        revocation_endpoint_auth_methods_supported: ["client_secret_basic"],
        scopes_supported: scope::SUPPORTED,
        authorization_response_iss_parameter_supported: true,
    };
    let mut response = Json(metadata).into_response();
    // Public and stable: the BFF caches it for an hour anyway.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}
