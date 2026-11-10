// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Layers for every response: security headers, request logs, panics,
//! and the time limits of each group of endpoints.

use std::any::Any;
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::{HeaderMap, HeaderValue, StatusCode, header};

use crate::error::{oauth_unavailable, problem};
use crate::jmap::UNAVAILABLE_RETRY_AFTER;
use crate::oauth::page::{ErrorPage, Lang, error_page};

/// Adds the security headers a response lacks. Handlers set
/// their own `Content-Security-Policy` and `Cache-Control` when they need
/// something else (login page, download, discovery).
pub(crate) async fn security_headers(
    State(https): State<bool>,
    mut response: Response,
) -> Response {
    let headers = response.headers_mut();
    let defaults = [
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
        (header::CACHE_CONTROL, "no-store"),
        (
            http::HeaderName::from_static("cross-origin-opener-policy"),
            "same-origin",
        ),
        // CORS requests (the web app's fetches) are not affected by CORP;
        // it only stops other sites from embedding responses.
        (
            http::HeaderName::from_static("cross-origin-resource-policy"),
            "same-origin",
        ),
        (
            http::HeaderName::from_static("permissions-policy"),
            "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
        ),
    ];
    for (name, value) in defaults {
        if !headers.contains_key(&name) {
            headers.insert(name, HeaderValue::from_static(value));
        }
    }
    if https {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=63072000; includeSubDomains"),
        );
    }
    response
}

/// Logs method, route, status and latency. Never the query, headers or
/// body (they carry tokens, codes and passwords), and the route template
/// instead of the path (paths carry file names and ids).
pub(crate) async fn log_requests(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "(no route)".to_owned(), |path| path.as_str().to_owned());
    let started = Instant::now();
    let response = next.run(request).await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(
        method = %method,
        route = %route,
        status = response.status().as_u16(),
        latency_ms,
        "request"
    );
    response
}

/// The answer to a handler that panicked. The payload is not logged: it
/// could contain mail data.
pub(crate) fn panic_response(_payload: Box<dyn Any + Send + 'static>) -> Response {
    tracing::error!("request handler panicked");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

/// 404 for unknown paths.
pub(crate) async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// A time limit and the answer when it passes. The answer is a 503 with
/// `Retry-After` (RFC 9110 §15.6.4, §10.2.3) in each group's error format,
/// like its other 503s; the handler is dropped (nothing it started is
/// kept waiting for it).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Deadline {
    limit: Duration,
    answer: fn(&HeaderMap) -> Response,
}

impl Deadline {
    /// JMAP: problem details (RFC 8620 §3.6.1, RFC 7807), as `internal()`.
    pub(crate) const fn jmap(limit: Duration) -> Self {
        Self {
            limit,
            answer: jmap_timed_out,
        }
    }

    /// OAuth endpoints that answer JSON (token, revoke, discovery): 503
    /// problem details, `no-store`, with no OAuth `error` code (RFC 6749
    /// §5.2 has none for this; RFC 7009 §2.2.1). The web BFF
    /// only looks at the status: it keeps the session on a 5xx.
    pub(crate) const fn oauth(limit: Duration) -> Self {
        Self {
            limit,
            answer: oauth_timed_out,
        }
    }

    /// The sign-in page: an HTML "busy" page, as when logins are full. It
    /// is not redirected with `temporarily_unavailable` (RFC 6749
    /// §4.1.2.1): the redirect URI may not have been checked yet.
    pub(crate) const fn login_page(limit: Duration) -> Self {
        Self {
            limit,
            answer: login_timed_out,
        }
    }
}

fn with_retry_after(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, UNAVAILABLE_RETRY_AFTER);
    response
}

fn jmap_timed_out(_: &HeaderMap) -> Response {
    with_retry_after(problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "The server took too long; try again later.",
    ))
}

fn oauth_timed_out(_: &HeaderMap) -> Response {
    oauth_unavailable(UNAVAILABLE_RETRY_AFTER)
}

fn login_timed_out(headers: &HeaderMap) -> Response {
    with_retry_after(error_page(Lang::from_headers(headers), ErrorPage::Busy))
}

/// Ends a request that takes longer than its [`Deadline`]: the time to
/// the response's head; a body that streams afterwards (a download) is
/// not limited here.
pub(crate) async fn deadline(
    State(deadline): State<Deadline>,
    request: Request,
    next: Next,
) -> Response {
    // Only what the answer reads (the language of the login page).
    let mut headers = HeaderMap::new();
    if let Some(value) = request.headers().get(header::ACCEPT_LANGUAGE) {
        headers.insert(header::ACCEPT_LANGUAGE, value.clone());
    }
    if let Ok(response) = tokio::time::timeout(deadline.limit, next.run(request)).await {
        response
    } else {
        tracing::warn!(limit_s = deadline.limit.as_secs(), "request took too long");
        (deadline.answer)(&headers)
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::routing::get;
    use tower::ServiceExt as _;

    use super::*;

    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        "late"
    }

    async fn call(deadline: Deadline, language: &str) -> (Response, serde_json::Value, String) {
        let app = Router::new()
            .route("/slow", get(slow))
            .route("/fast", get(async || "ok"))
            .layer(axum::middleware::from_fn_with_state(
                deadline,
                self::deadline,
            ));
        let request = http::Request::get("/slow")
            .header(header::ACCEPT_LANGUAGE, language)
            .body(Body::empty())
            .unwrap();
        let (parts, body) = app.oneshot(request).await.unwrap().into_parts();
        let bytes = to_bytes(body, 1 << 16).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        (Response::from_parts(parts, Body::empty()), json, text)
    }

    /// A JMAP request that takes too long: 503 with problem details and
    /// `Retry-After` (RFC 8620 §3.6.1, RFC 9110 §10.2.3).
    #[tokio::test(start_paused = true)]
    async fn jmap_timeout_is_a_problem() {
        let (response, json, _) = call(Deadline::jmap(Duration::from_secs(300)), "en").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "5");
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        assert_eq!(json["type"], "about:blank");
        assert_eq!(json["status"], 503);
    }

    /// An OAuth one: 503, `no-store`, no OAuth `error` code (not a RFC 6749
    /// §5.2 error; RFC 7009 §2.2.1).
    #[tokio::test(start_paused = true)]
    async fn oauth_timeout_is_an_oauth_error() {
        let (response, json, _) = call(Deadline::oauth(Duration::from_secs(30)), "en").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "5");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(json["status"], 503);
        assert!(json.get("error").is_none(), "{json}");
    }

    /// The sign-in page: an HTML page in the browser's language.
    #[tokio::test(start_paused = true)]
    async fn login_timeout_is_a_page() {
        let (response, _, text) = call(Deadline::login_page(Duration::from_secs(30)), "es").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::RETRY_AFTER], "5");
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert!(text.contains("lang=\"es\""), "{text}");
    }

    #[tokio::test(start_paused = true)]
    async fn fast_requests_pass() {
        let app = Router::new().route("/fast", get(async || "ok")).layer(
            axum::middleware::from_fn_with_state(Deadline::jmap(Duration::from_secs(1)), deadline),
        );
        let response = app
            .oneshot(http::Request::get("/fast").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
