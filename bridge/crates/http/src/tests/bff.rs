// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Replays the web BFF's requests exactly as `apps/web/src/server/auth`
//! (`oidc.ts`, `handlers.ts`) sends them, and checks the answers the way it
//! parses them, so the web works unchanged against the bridge.

use std::fmt::Write as _;

use axum::body::Body;
use data_encoding::BASE64URL_NOPAD;
use http::{Request, StatusCode, header};
use sha2::{Digest, Sha256};

use super::*;

/// JavaScript's `encodeURIComponent`.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            write!(out, "%{byte:02X}").unwrap();
        }
    }
    out
}

/// `formEncode` of `oidc.ts`.
fn form_encode(text: &str) -> String {
    encode_uri_component(text).replace("%20", "+")
}

/// `new URLSearchParams(record).toString()`: spaces as `+`.
fn url_search_params(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(name, value)| format!("{}={}", form_encode(name), form_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// `randomToken()` of `seal.ts`: 32 random bytes as base64url.
fn random_token() -> String {
    nebula_crypto::Token::generate()
        .unwrap()
        .expose()
        .to_owned()
}

/// `discover()`, then `handleLogin()`, the user's form post and
/// `handleCallback()` up to the code: (metadata, code, code verifier).
async fn bff_authorize(h: &Harness) -> (Value, String, String) {
    // discover(): GET {JMAP_SERVER}/.well-known/oauth-authorization-server (RFC 8414 §3).
    let discovery = h
        .send(
            Request::get("/.well-known/oauth-authorization-server")
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(discovery.status, StatusCode::OK);
    let metadata = discovery.json();
    // `issuer` without trailing slash must equal JMAP_SERVER without one.
    assert_eq!(
        metadata["issuer"].as_str().unwrap().trim_end_matches('/'),
        ISSUER
    );
    let authorization_endpoint =
        Url::parse(metadata["authorization_endpoint"].as_str().unwrap()).unwrap();
    let token_endpoint = Url::parse(metadata["token_endpoint"].as_str().unwrap()).unwrap();
    assert_eq!(authorization_endpoint.scheme(), "https");
    assert_eq!(token_endpoint.scheme(), "https");
    assert!(
        metadata["code_challenge_methods_supported"]
            .as_array()
            .unwrap()
            .contains(&"S256".into())
    );
    assert_eq!(
        metadata["authorization_response_iss_parameter_supported"],
        true
    );

    // handleLogin(): redirect to authorizationUrl(...).
    let code_verifier = random_token();
    let state = random_token();
    let code_challenge = BASE64URL_NOPAD.encode(&Sha256::digest(code_verifier.as_bytes()));
    let search = url_search_params(&[
        ("client_id", CLIENT_ID),
        ("code_challenge", &code_challenge),
        ("code_challenge_method", "S256"),
        ("redirect_uri", REDIRECT),
        ("response_type", "code"),
        ("scope", "urn:ietf:params:oauth:scope:mail offline_access"),
        ("state", &state),
    ]);
    let page = h
        .get(&format!("{}?{search}", authorization_endpoint.path()))
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text());
    let html = page.text();
    let marker = "name=\"pending\" value=\"";
    let start = html.find(marker).unwrap() + marker.len();
    let pending = html.get(start..start + 43).unwrap();
    let cookie = page
        .header(header::SET_COOKIE)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    // The user submits the form (a browser form post).
    let submitted = h
        .send(
            Request::post("/oauth/authorize")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, "null")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(url_search_params(&[
                    ("pending", pending),
                    ("login", "alice"),
                    ("password", "right password"),
                    ("action", "login"),
                ])))
                .unwrap(),
        )
        .await;
    assert_eq!(submitted.status, StatusCode::SEE_OTHER);

    // handleCallback(): state, then `iss` must be exactly metadata.issuer.
    let callback = submitted.location();
    assert_eq!(
        format!(
            "{}{}",
            callback.origin().ascii_serialization(),
            callback.path()
        ),
        REDIRECT
    );
    let params = query(&callback);
    assert_eq!(params["state"], state);
    assert_eq!(params["iss"], metadata["issuer"].as_str().unwrap());
    assert!(!params.contains_key("error"));
    (metadata, params["code"].clone(), code_verifier)
}

#[tokio::test(flavor = "multi_thread")]
async fn web_bff_login_refresh_and_errors() {
    let h = Harness::new().await;
    let (metadata, code, code_verifier) = bff_authorize(&h).await;
    let code = &code;
    let token_endpoint = Url::parse(metadata["token_endpoint"].as_str().unwrap()).unwrap();

    // exchangeCode(): client_secret_basic with form-encoded parts.
    let authorization = format!(
        "Basic {}",
        BASE64.encode(
            format!("{}:{}", form_encode(CLIENT_ID), form_encode(CLIENT_SECRET)).as_bytes()
        )
    );
    assert_eq!(
        authorization,
        format!("Basic {}", BASE64.encode(b"nebulamail:s3cret+value"))
    );
    let token_call = |body: String| {
        Request::post(token_endpoint.path())
            .header(header::ACCEPT, "application/json")
            .header(header::AUTHORIZATION, &authorization)
            .header(
                header::CONTENT_TYPE,
                "application/x-www-form-urlencoded;charset=UTF-8",
            )
            .body(Body::from(body))
            .unwrap()
    };
    let exchanged = h
        .send(token_call(url_search_params(&[
            ("code", code),
            ("code_verifier", &code_verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", REDIRECT),
        ])))
        .await;
    assert_eq!(exchanged.status, StatusCode::OK, "{}", exchanged.text());
    let tokens = check_token_response(&exchanged.json());
    // The BFF fails the login without a refresh token.
    let refresh_token = tokens.1.unwrap();

    // jmapSessionUrl = new URL("/.well-known/jmap", issuer).
    let session = h
        .with_bearer("GET", "/.well-known/jmap", &tokens.0, Body::empty())
        .await;
    assert_eq!(session.status, StatusCode::OK);
    let api_url = Url::parse(session.json()["apiUrl"].as_str().unwrap()).unwrap();
    // The jmap-client origin check: every URL on the session's origin.
    assert_eq!(api_url.origin().ascii_serialization(), ISSUER);

    // refresh(): only grant_type and refresh_token.
    h.advance(800);
    let refreshed = h
        .send(token_call(url_search_params(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
        ])))
        .await;
    assert_eq!(refreshed.status, StatusCode::OK);
    let (_, new_refresh) = check_token_response(&refreshed.json());
    assert!(new_refresh.is_some());

    // A second tab with the same (old) refresh token: 5xx → "unavailable",
    // the BFF keeps the session.
    let raced = h
        .send(token_call(url_search_params(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
        ])))
        .await;
    assert!(raced.status.is_server_error());

    // A dead refresh token: 4xx with {error} → "rejected", log out.
    h.advance(60);
    let rejected = h
        .send(token_call(url_search_params(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
        ])))
        .await;
    assert!(rejected.status.is_client_error());
    assert!(rejected.json()["error"].is_string());
}

/// The checks of `TokenResponse` in `oidc.ts` (zod): access token,
/// positive integer `expires_in`, optional refresh token, bearer type.
fn check_token_response(json: &Value) -> (String, Option<String>) {
    let access = json["access_token"].as_str().unwrap();
    assert!(!access.is_empty());
    assert!(json["expires_in"].as_u64().unwrap() > 0);
    assert!(
        json["token_type"]
            .as_str()
            .unwrap()
            .eq_ignore_ascii_case("bearer")
    );
    let refresh = json
        .get("refresh_token")
        .map(|r| r.as_str().unwrap().to_owned());
    (access.to_owned(), refresh)
}

#[tokio::test(flavor = "multi_thread")]
async fn web_bff_cancel_and_mixup_protection() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let denied = h.post_form(&form, &[("action", "deny")]).await;
    let params = query(&denied.location());
    // handleCallback checks `iss` before `error`: it must be there.
    assert_eq!(params["iss"], ISSUER);
    assert_eq!(params["error"], "access_denied");
}
