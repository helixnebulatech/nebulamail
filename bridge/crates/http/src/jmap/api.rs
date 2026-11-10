// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `POST /jmap/api` (RFC 8620 §3): request-level checks here, method
//! dispatch in the backend.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use nebula_jmap_proto::Request as JmapRequest;
use nebula_jmap_proto::error::LimitName;

use crate::backend::{AcceptLanguage, ApiCall, Backend};
use crate::error::HttpError;
use crate::jmap::session::build_session;
use crate::jmap::{
    BodyError, authenticate_session, body_read_error, internal, limit_error, read_limited,
    request_error,
};
use crate::state::Shared;

/// `POST /jmap/api`.
pub(crate) async fn api<B: Backend>(
    State(shared): State<Arc<Shared<B>>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let (account, grant) = match authenticate_session(&shared, &parts.headers).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    // Held until the response is built.
    let Some(_slot) = shared.api_slots.try_acquire(account.id) else {
        return limit_error(
            LimitName::MaxConcurrentRequests,
            "Too many concurrent requests for this account.",
        );
    };
    let limit = shared.config.jmap.max_size_request();
    let body = match read_limited(&parts.headers, body, limit).await {
        Ok(body) => body,
        Err(BodyError::TooLarge) => {
            return limit_error(
                LimitName::MaxSizeRequest,
                &format!("The request is larger than {limit} bytes."),
            );
        }
        Err(BodyError::Read) => return body_read_error(),
    };
    let session = match build_session(&shared, &account).await {
        Ok(session) => session,
        Err(error) => return internal(&error),
    };
    let core = shared.config.jmap.core();
    let parsed = match JmapRequest::parse(
        &body,
        &core.request_limits(),
        &session.capabilities.supported(),
    ) {
        Ok(parsed) => parsed,
        Err(error) => return request_error(&error),
    };
    let languages = parts
        .headers
        .get(http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .map(AcceptLanguage::parse)
        .unwrap_or_default();
    let call = ApiCall {
        request: parsed,
        session_state: session.state,
        limits: core.method_limits(),
        languages,
        session: grant,
    };
    match shared.backend.jmap(&account, call).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => internal(&HttpError::from(error)),
    }
}
