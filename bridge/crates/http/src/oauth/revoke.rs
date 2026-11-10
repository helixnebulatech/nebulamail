// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `POST /oauth/revoke` (RFC 7009): ends the login session of an access
//! or refresh token. If it was the account's last session, its password
//! is deleted and the backend stops.

use std::sync::Arc;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::StatusCode;
use http::request::Parts;
use nebula_crypto::Token;

use crate::backend::Backend;
use crate::error::{HttpError, OAuthErrorCode, no_store, oauth_error, oauth_internal};
use crate::oauth::store_hash;
use crate::oauth::token::authenticated_form;
use crate::state::{SessionEnd, Shared};

/// `POST /oauth/revoke`. Unknown or invalid tokens are a success
/// (RFC 7009 §2.2): the client can't tell whether a token existed.
pub(crate) async fn revoke<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    parts: Parts,
    body: Bytes,
) -> Response {
    let (client, form) = match authenticated_form(&shared, &parts, &body).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    let Some(token) = form.get("token") else {
        return oauth_error(OAuthErrorCode::InvalidRequest);
    };
    let Ok(token) = Token::parse(token) else {
        return revoked();
    };
    match revoke_token(&shared, client.id().as_str(), &token).await {
        Ok(()) => revoked(),
        Err(error) => oauth_internal(&error),
    }
}

fn revoked() -> Response {
    no_store(StatusCode::OK.into_response())
}

/// Revokes the grant of `token`, an access or a refresh token (used or
/// not). Only the token's own client can revoke it (RFC 7009 §2.1); for
/// others nothing happens, and the answer is the same 200.
async fn revoke_token<B: Backend>(
    shared: &Shared<B>,
    client_id: &str,
    token: &Token,
) -> Result<(), HttpError> {
    let now = shared.now()?;
    let hash = store_hash(token);
    let owner = match shared.store.access_token(hash, now).await? {
        Some(owner) => Some(owner),
        None => shared.store.refresh_token_owner(hash, now).await?,
    };
    if let Some(owner) = owner.filter(|owner| owner.client_id == client_id) {
        shared.store.revoke_grant(owner.grant, now).await?;
        shared
            .stop_if_idle(owner.account, SessionEnd::Ended)
            .await?;
        tracing::info!(
            account = owner.account.get(),
            "session revoked by its client"
        );
    }
    Ok(())
}
