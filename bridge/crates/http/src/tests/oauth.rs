// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use axum::body::Body;
use http::{Request, StatusCode, header};
use serde_json::json;

use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn rotated_master_keys_rewrap_the_data_keys() {
    let h = Harness::new().await;
    h.login().await;
    let account = h.store.account_by_login("alice").await.unwrap().unwrap();
    let key = |version: u16, byte: u8| {
        MasterKey::from_bytes(KeyVersion::new(version).unwrap(), &[byte; 32]).unwrap()
    };
    // Nothing to do with the key it was made with.
    let report = crate::rewrap_data_keys(&h.store, &keyring()).await.unwrap();
    assert_eq!((report.rewrapped, report.current, report.failed), (0, 1, 0));

    // Rotated: wrapped again with version 2, same data key.
    let rotated = Keyring::new(key(2, 9), [key(1, 7)]).unwrap();
    let report = crate::rewrap_data_keys(&h.store, &rotated).await.unwrap();
    assert_eq!((report.rewrapped, report.failed), (1, 0));
    assert!(report.all_current());
    let after = h.store.account(account.id).await.unwrap().unwrap();
    assert_ne!(after.wrapped_dek, account.wrapped_dek);
    let id = crate::state::crypto_account("alice").unwrap();
    let new = nebula_crypto::WrappedDataKey::from_bytes(&after.wrapped_dek).unwrap();
    assert_eq!(new.key_version().get(), 2);
    let only_new = Keyring::new(key(2, 9), []).unwrap();
    assert!(only_new.unwrap(&id, &new).is_ok());

    // A key whose master key is gone is reported, not changed.
    let other = Keyring::new(key(3, 5), []).unwrap();
    let report = crate::rewrap_data_keys(&h.store, &other).await.unwrap();
    assert_eq!((report.rewrapped, report.failed), (0, 1));
    assert!(!report.all_current());
    let kept = h.store.account(account.id).await.unwrap().unwrap();
    assert_eq!(kept.wrapped_dek, after.wrapped_dek);
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_document() {
    let h = Harness::new().await;
    // Not an OpenID Connect provider: no OIDC discovery document.
    assert_eq!(
        h.get("/.well-known/openid-configuration").await.status,
        StatusCode::NOT_FOUND
    );
    for path in ["/.well-known/oauth-authorization-server"] {
        let reply = h.get(path).await;
        assert_eq!(reply.status, StatusCode::OK);
        let json = reply.json();
        assert_eq!(json["issuer"], ISSUER);
        assert_eq!(
            json["authorization_endpoint"],
            format!("{ISSUER}/oauth/authorize")
        );
        assert_eq!(json["token_endpoint"], format!("{ISSUER}/oauth/token"));
        assert_eq!(
            json["revocation_endpoint"],
            format!("{ISSUER}/oauth/revoke")
        );
        assert_eq!(json["response_types_supported"], json!(["code"]));
        assert_eq!(json["code_challenge_methods_supported"], json!(["S256"]));
        assert_eq!(
            json["token_endpoint_auth_methods_supported"],
            json!(["client_secret_basic"])
        );
        assert_eq!(
            json["grant_types_supported"],
            json!(["authorization_code", "refresh_token"])
        );
        assert_eq!(json["authorization_response_iss_parameter_supported"], true);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn login_form_is_locked_down() {
    let h = Harness::new().await;
    let reply = h
        .get(&format!("/oauth/authorize?{}", encode(&authorize_query())))
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    let csp = reply.header(header::CONTENT_SECURITY_POLICY).unwrap();
    assert!(
        csp.starts_with("default-src 'none'; style-src 'sha256-"),
        "{csp}"
    );
    assert!(
        csp.contains("form-action 'self' https://mail.example.com"),
        "{csp}"
    );
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(!csp.contains("unsafe"));
    assert_eq!(reply.header(header::CACHE_CONTROL), Some("no-store"));
    assert_eq!(reply.header(header::REFERRER_POLICY), Some("no-referrer"));
    assert_eq!(reply.header(header::X_FRAME_OPTIONS), Some("DENY"));
    let cookie = reply.header(header::SET_COOKIE).unwrap();
    assert!(cookie.starts_with("__Host-nebula-authz="), "{cookie}");
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/"] {
        assert!(cookie.contains(attribute), "{cookie}");
    }
    let html = reply.text();
    assert!(html.contains("Nebula Mail"));
    assert!(!html.contains("<script"));
    // Request parameters are not in the form, only an opaque id.
    assert!(!html.contains(CHALLENGE));
    assert!(!html.contains("the-state"));

    let spanish = h
        .send(
            Request::get(format!("/oauth/authorize?{}", encode(&authorize_query())))
                .header(header::ACCEPT_LANGUAGE, "es-ES,es;q=0.9")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert!(spanish.text().contains("Iniciar sesión"));
    assert!(spanish.text().contains("lang=\"es\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_client_or_redirect_is_a_page_never_a_redirect() {
    let h = Harness::new().await;
    let with = |name: &'static str, value: &'static str| {
        let mut params: Vec<_> = authorize_query()
            .into_iter()
            .filter(|(n, _)| *n != name)
            .collect();
        if !value.is_empty() {
            params.push((name, value));
        }
        format!("/oauth/authorize?{}", encode(&params))
    };
    for uri in [
        with("redirect_uri", "https://evil.example/callback"),
        with("redirect_uri", "https://mail.example.com/auth/callback/"),
        with("redirect_uri", ""),
        with("client_id", "other"),
        with("client_id", ""),
        format!(
            "/oauth/authorize?{}&client_id=nebulamail",
            encode(&authorize_query())
        ),
    ] {
        let reply = h.get(&uri).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
        assert!(reply.header(header::LOCATION).is_none(), "{uri}");
        assert!(reply.text().contains("not valid"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn other_errors_go_back_to_the_client() {
    let h = Harness::new().await;
    let cases = [
        ("code_challenge_method", "plain", "invalid_request"),
        ("code_challenge_method", "", "invalid_request"),
        ("code_challenge", "", "invalid_request"),
        ("code_challenge", "short", "invalid_request"),
        ("response_type", "token", "unsupported_response_type"),
    ];
    for (name, value, error) in cases {
        let mut params: Vec<_> = authorize_query()
            .into_iter()
            .filter(|(n, _)| *n != name)
            .collect();
        if !value.is_empty() {
            params.push((name, value));
        }
        let reply = h
            .get(&format!("/oauth/authorize?{}", encode(&params)))
            .await;
        assert_eq!(reply.status, StatusCode::SEE_OTHER, "{name}={value}");
        let location = reply.location();
        assert!(location.as_str().starts_with(REDIRECT));
        let params = query(&location);
        assert_eq!(params["error"], error);
        assert_eq!(params["state"], "the-state");
        assert_eq!(params["iss"], ISSUER);
    }
}

/// `state` is RECOMMENDED, not required (RFC 6749 §4.1.1): without one
/// (or with an empty one, RFC 6749 §3.1) the flow works and the answer
/// has none (§4.1.2, §4.1.2.1); one longer than the bridge echoes is
/// refused.
#[tokio::test(flavor = "multi_thread")]
async fn state_is_optional() {
    let h = Harness::new().await;
    for state in [None, Some("")] {
        let mut params: Vec<_> = authorize_query()
            .into_iter()
            .filter(|(n, _)| *n != "state")
            .collect();
        if let Some(state) = state {
            params.push(("state", state));
        }
        let form = h.open_form_with(&params).await;
        let reply = h
            .post_form(
                &form,
                &[
                    ("login", "alice"),
                    ("password", "right password"),
                    ("action", "login"),
                ],
            )
            .await;
        assert_eq!(reply.status, StatusCode::SEE_OTHER, "{state:?}");
        let answer = query(&reply.location());
        assert!(!answer.contains_key("state"), "{state:?}");
        assert_eq!(answer["iss"], ISSUER);
        assert_eq!(h.exchange(&answer["code"]).await.status, StatusCode::OK);

        // Errors and a refusal have no `state` either.
        let mut bad = params.clone();
        bad.retain(|(n, _)| *n != "code_challenge_method");
        bad.push(("code_challenge_method", "plain"));
        let reply = h.get(&format!("/oauth/authorize?{}", encode(&bad))).await;
        let answer = query(&reply.location());
        assert_eq!(answer["error"], "invalid_request");
        assert!(!answer.contains_key("state"));
        let form = h.open_form_with(&params).await;
        let reply = h.post_form(&form, &[("action", "deny")]).await;
        let answer = query(&reply.location());
        assert_eq!(answer["error"], "access_denied");
        assert!(!answer.contains_key("state"));
    }

    let long = "s".repeat(513);
    let mut params: Vec<_> = authorize_query()
        .into_iter()
        .filter(|(n, _)| *n != "state")
        .collect();
    params.push(("state", &long));
    let reply = h
        .get(&format!("/oauth/authorize?{}", encode(&params)))
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let answer = query(&reply.location());
    assert_eq!(answer["error"], "invalid_request");
    assert!(!answer.contains_key("state"));
}

#[tokio::test(flavor = "multi_thread")]
async fn passwords_are_stored_sealed() {
    let h = Harness::new().await;
    h.code().await;
    let account = h.store.account_by_login("alice").await.unwrap().unwrap();
    let sealed = h
        .store
        .secret(account.id, nebula_store::SecretField::ImapPassword)
        .await
        .unwrap()
        .unwrap();
    assert!(!sealed.windows(14).any(|w| w == b"right password"));
}

#[tokio::test(flavor = "multi_thread")]
async fn full_flow_with_rotation_race_and_reuse() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let reply = h
        .post_form(
            &form,
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    assert_eq!(reply.header(header::CACHE_CONTROL), Some("no-store"));
    let location = reply.location();
    assert!(location.as_str().starts_with(REDIRECT));
    let params = query(&location);
    assert_eq!(params["state"], "the-state");
    assert_eq!(params["iss"], ISSUER);
    assert_eq!(params["code"].len(), 43);
    assert_eq!(
        h.fake.seen.lock().unwrap().started,
        [("alice".to_owned(), "right password".to_owned())]
    );
    let account = h.store.account_by_login("alice").await.unwrap().unwrap();

    let reply = h.exchange(&params["code"]).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.header(header::CACHE_CONTROL), Some("no-store"));
    assert_eq!(reply.header(header::PRAGMA), Some("no-cache"));
    let tokens = reply.json();
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 900);
    assert_eq!(
        tokens["scope"],
        "urn:ietf:params:oauth:scope:mail offline_access"
    );
    let access = tokens["access_token"].as_str().unwrap();
    let refresh = tokens["refresh_token"].as_str().unwrap();

    let session = h
        .with_bearer("GET", "/.well-known/jmap", access, Body::empty())
        .await;
    assert_eq!(session.status, StatusCode::OK);

    // A form works once.
    let again = h
        .post_form(
            &form,
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(again.status, StatusCode::FORBIDDEN);

    h.advance(600);
    let rotated = h.refresh(refresh).await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.text());
    let rotated = rotated.json();
    let new_access = rotated["access_token"].as_str().unwrap().to_owned();
    let new_refresh = rotated["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(new_refresh, refresh);
    assert_eq!(
        rotated["scope"],
        "urn:ietf:params:oauth:scope:mail offline_access"
    );

    // Another tab with the old token a moment later: 503, session kept.
    h.advance(2);
    let raced = h.refresh(refresh).await;
    assert_eq!(raced.status, StatusCode::SERVICE_UNAVAILABLE);
    // Not a RFC 6749 §5.2 error: the status says it, no OAuth `error`.
    assert!(raced.json().get("error").is_none());
    assert!(raced.header(header::RETRY_AFTER).is_some());
    let ok = h
        .with_bearer("GET", "/.well-known/jmap", &new_access, Body::empty())
        .await;
    assert_eq!(ok.status, StatusCode::OK);

    // The old token much later: stolen. Everything is revoked.
    h.advance(120);
    let reused = h.refresh(refresh).await;
    assert_eq!(reused.status, StatusCode::BAD_REQUEST);
    assert_eq!(reused.json(), json!({"error": "invalid_grant"}));
    let dead = h
        .with_bearer("GET", "/.well-known/jmap", &new_access, Body::empty())
        .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);
    assert!(
        dead.header(header::WWW_AUTHENTICATE)
            .unwrap()
            .starts_with("Bearer")
    );
    assert_eq!(
        h.refresh(&new_refresh).await.status,
        StatusCode::BAD_REQUEST
    );

    // Its last session: the password is deleted at once, not at cleanup.
    assert_eq!(h.fake.seen.lock().unwrap().stopped, [account.id]);
    assert!(
        h.store
            .secret(account.id, nebula_store::SecretField::ImapPassword)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn codes_are_single_use_and_bound() {
    let h = Harness::new().await;
    let code = h.code().await;
    let first = h.exchange(&code).await;
    assert_eq!(first.status, StatusCode::OK);
    let access = first.json()["access_token"].as_str().unwrap().to_owned();
    let refresh = first.json()["refresh_token"].as_str().unwrap().to_owned();
    let reused = h.exchange(&code).await;
    assert_eq!(reused.json(), json!({"error": "invalid_grant"}));
    // RFC 6749 §4.1.2: the tokens issued from the code are revoked, and
    // the account's password goes with its last session.
    let dead = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.refresh(&refresh).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(h.fake.seen.lock().unwrap().stopped.len(), 1);

    // Wrong PKCE verifier: the code is consumed anyway.
    let code = h.code().await;
    let wrong = h
        .token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[
                ("code", &code),
                ("code_verifier", &"a".repeat(43)),
                ("grant_type", "authorization_code"),
                ("redirect_uri", REDIRECT),
            ],
        )
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    assert_eq!(wrong.json(), json!({"error": "invalid_grant"}));
    assert_eq!(h.exchange(&code).await.status, StatusCode::BAD_REQUEST);

    // Another redirect URI than the authorization request's.
    let code = h.code().await;
    let other = h
        .token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[
                ("code", &code),
                ("code_verifier", VERIFIER),
                ("grant_type", "authorization_code"),
                ("redirect_uri", "https://mail.example.com/other"),
            ],
        )
        .await;
    assert_eq!(other.json(), json!({"error": "invalid_grant"}));

    // Expired after a minute.
    let code = h.code().await;
    h.advance(61);
    assert_eq!(
        h.exchange(&code).await.json(),
        json!({"error": "invalid_grant"})
    );

    // Missing parameters and unknown grant types.
    let missing = h
        .token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[("grant_type", "authorization_code")],
        )
        .await;
    assert_eq!(missing.json(), json!({"error": "invalid_request"}));
    let unknown = h
        .token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[("grant_type", "password")],
        )
        .await;
    assert_eq!(unknown.json(), json!({"error": "unsupported_grant_type"}));
    let duplicate = h
        .token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[
                ("grant_type", "refresh_token"),
                ("grant_type", "refresh_token"),
            ],
        )
        .await;
    assert_eq!(duplicate.json(), json!({"error": "invalid_request"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn client_authentication() {
    let h = Harness::new().await;
    let code = h.code().await;
    for authorization in [
        basic(CLIENT_ID, "wrong"),
        basic("other", CLIENT_SECRET),
        format!("Bearer {CLIENT_SECRET}"),
        // Not form-encoded: "s3cret value" with a raw space is fine, but
        // "+" would decode to a space.
        format!("Basic {}", BASE64.encode(b"nebulamail:s3cret%2Bvalue")),
    ] {
        let reply = h
            .token_request(
                &authorization,
                &[
                    ("code", &code),
                    ("code_verifier", VERIFIER),
                    ("grant_type", "authorization_code"),
                    ("redirect_uri", REDIRECT),
                ],
            )
            .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{authorization}");
        assert_eq!(reply.json(), json!({"error": "invalid_client"}));
        assert!(
            reply
                .header(header::WWW_AUTHENTICATE)
                .unwrap()
                .starts_with("Basic")
        );
    }
    // A failed client authentication doesn't consume the code.
    assert_eq!(h.exchange(&code).await.status, StatusCode::OK);

    // A client_id in the body must match.
    let code = h.code().await;
    let mismatch = h
        .token_request(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[
                ("client_id", "other"),
                ("code", &code),
                ("code_verifier", VERIFIER),
                ("grant_type", "authorization_code"),
                ("redirect_uri", REDIRECT),
            ],
        )
        .await;
    assert_eq!(mismatch.json(), json!({"error": "invalid_request"}));

    // Only form bodies.
    let json_body = h
        .send(
            Request::post("/oauth/token")
                .header(header::AUTHORIZATION, basic(CLIENT_ID, CLIENT_SECRET))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(json_body.json(), json!({"error": "invalid_request"}));
}

#[tokio::test(flavor = "multi_thread")]
async fn csrf_and_cross_site_posts_are_refused() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let fields = encode(&[
        ("pending", form.pending.as_str()),
        ("login", "alice"),
        ("password", "right password"),
        ("action", "login"),
    ]);
    let post = |cookie: Option<&str>, site: Option<&str>| {
        let mut request = Request::post("/oauth/authorize")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        if let Some(site) = site {
            request = request.header("sec-fetch-site", site);
        }
        request.body(Body::from(fields.clone())).unwrap()
    };
    let other_cookie = format!("__Host-nebula-authz={}", "A".repeat(43));
    for request in [
        post(None, Some("same-origin")),
        post(Some(&other_cookie), Some("same-origin")),
        post(Some(&form.cookie), Some("cross-site")),
        post(Some(&form.cookie), Some("same-site")),
    ] {
        let reply = h.send(request).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
        assert!(reply.header(header::LOCATION).is_none());
    }
    assert!(h.fake.seen.lock().unwrap().started.is_empty());
    // Unknown or expired pending ids.
    let unknown = h
        .post_form(
            &OpenForm {
                pending: "B".repeat(43),
                cookie: form.cookie.clone(),
            },
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(unknown.status, StatusCode::FORBIDDEN);
    h.advance(601);
    let expired = h
        .post_form(
            &form,
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(expired.status, StatusCode::FORBIDDEN);
    // Without Sec-Fetch-Site (old browsers) the cookie is enough.
    let fresh = h.open_form().await;
    let reply = h
        .send(
            Request::post("/oauth/authorize")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, &fresh.cookie)
                .body(Body::from(encode(&[
                    ("pending", fresh.pending.as_str()),
                    ("login", "alice"),
                    ("password", "right password"),
                    ("action", "login"),
                ])))
                .unwrap(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
}

#[tokio::test(flavor = "multi_thread")]
async fn second_form_reuses_the_cookie() {
    let h = Harness::new().await;
    let first = h.open_form().await;
    let second = h
        .send(
            Request::get(format!("/oauth/authorize?{}", encode(&authorize_query())))
                .header(header::COOKIE, &first.cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    // The same token, sent again so the cookie outlives the newest form.
    let renewed = second.header(header::SET_COOKIE).unwrap();
    assert!(
        renewed.starts_with(&format!("{};", first.cookie)),
        "{renewed}"
    );
    assert!(renewed.contains("Max-Age=3600"), "{renewed}");
    // The first tab's form still works.
    let reply = h
        .post_form(
            &first,
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_sends_access_denied() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let reply = h.post_form(&form, &[("action", "deny")]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let params = query(&reply.location());
    assert_eq!(params["error"], "access_denied");
    assert_eq!(params["state"], "the-state");
    assert_eq!(params["iss"], ISSUER);
    assert!(!params.contains_key("code"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_password_refused_when_starting_is_a_wrong_password() {
    let h = Harness::new().await;
    h.fake.seen.lock().unwrap().refuse_password = true;
    let form = h.open_form().await;
    let reply = h
        .post_form(
            &form,
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(reply.text().contains("Wrong username or password."));
}

/// RFC 3501 §7.1 / RFC 9051 §7.1: the `[ALERT]` of a refused login is
/// shown in the form, as escaped plain text.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_with_an_alert_shows_it() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let reply = h
        .post_form(
            &form,
            &[
                ("login", "locked"),
                ("password", "any"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    let text = reply.text();
    assert!(text.contains("Wrong username or password."));
    assert!(text.contains("Message from your mail server:"));
    assert!(text.contains("Account locked &lt;b&gt;call&lt;/b&gt; us"));
    assert!(!text.contains("<b>call</b>"));
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_and_rate_limits() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let attempt = |login: &'static str, password: &'static str| {
        let h = &h;
        let form = &form;
        async move {
            h.post_form(
                form,
                &[
                    ("login", login),
                    ("password", password),
                    ("action", "login"),
                ],
            )
            .await
        }
    };
    let missing = attempt("alice", "").await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert!(missing.text().contains("Enter your username and password"));

    for _ in 0..5 {
        let reply = attempt("alice", "wrong").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
        // Same message whether or not the user exists.
        assert!(reply.text().contains("Wrong username or password."));
        assert!(reply.text().contains("value=\"alice\""));
    }
    let unknown = attempt("nobody", "wrong").await;
    assert!(unknown.text().contains("Wrong username or password."));
    // Sixth try for alice: refused even with the right password.
    let limited = attempt("Alice", "right password").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.text().contains("Too many attempts"));
    assert!(h.fake.seen.lock().unwrap().started.is_empty());

    // After the window it works again.
    h.advance(15 * 60 + 1);
    let form = h.open_form().await;
    let reply = h
        .post_form(
            &form,
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
}

#[tokio::test(flavor = "multi_thread")]
async fn per_ip_limit_and_unavailable_server() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let down = [("login", "down"), ("password", "x"), ("action", "login")];
    // "down" is unavailable: shown as such, and not counted against the
    // login, but still against the IP (20 per IP).
    for _ in 0..20 {
        let reply = h.post_form_from(&form, "198.51.100.1", &down).await;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(reply.text().contains("not answering"));
    }
    let reply = h.post_form_from(&form, "198.51.100.1", &down).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    // The login's own window is untouched (it would be full after 5).
    for _ in 0..5 {
        let reply = h.post_form_from(&form, "198.51.100.2", &down).await;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    }
    // 20 failures from one IP across logins, then refused.
    for n in 0..20 {
        let login = format!("user{n}");
        let reply = h
            .post_form_from(
                &form,
                "198.51.100.3",
                &[("login", &login), ("password", "x"), ("action", "login")],
            )
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }
    let reply = h
        .post_form_from(
            &form,
            "198.51.100.3",
            &[("login", "user99"), ("password", "x"), ("action", "login")],
        )
        .await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    // Alice, from elsewhere, still gets in.
    let reply = h
        .post_form_from(
            &form,
            "198.51.100.4",
            &[
                ("login", "alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_answers_count_as_failures() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let odd = [("login", "odd"), ("password", "x"), ("action", "login")];
    for n in 0..5 {
        let reply = h
            .post_form_from(&form, &format!("198.51.100.{n}"), &odd)
            .await;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    }
    // Five answers that weren't "wrong password" still used the login's
    // five tries.
    let reply = h.post_form_from(&form, "198.51.100.9", &odd).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test(flavor = "multi_thread")]
async fn revocation_ends_the_session() {
    let h = Harness::new().await;
    let (access, refresh) = h.login().await;
    let revoke = |token: String| {
        let h = &h;
        async move {
            h.send(
                Request::post("/oauth/revoke")
                    .header(header::AUTHORIZATION, basic(CLIENT_ID, CLIENT_SECRET))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(encode(&[("token", token.as_str())])))
                    .unwrap(),
            )
            .await
        }
    };
    let reply = revoke(refresh.clone()).await;
    assert_eq!(reply.status, StatusCode::OK);
    let dead = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.refresh(&refresh).await.status, StatusCode::BAD_REQUEST);
    // Last session: password deleted, backend stopped at once.
    assert_eq!(h.fake.seen.lock().unwrap().stopped.len(), 1);

    // Revoking by access token, and unknown tokens, also answer 200.
    let (access, _) = h.login().await;
    assert_eq!(revoke(access.clone()).await.status, StatusCode::OK);
    let dead = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        revoke("not-a-token".to_owned()).await.status,
        StatusCode::OK
    );
    assert_eq!(revoke("C".repeat(43)).await.status, StatusCode::OK);

    // A refresh token rotated a moment ago still ends its session.
    let (_, refresh) = h.login().await;
    let rotated = h.refresh(&refresh).await.json();
    let new_access = rotated["access_token"].as_str().unwrap().to_owned();
    assert_eq!(revoke(refresh.clone()).await.status, StatusCode::OK);
    let dead = h
        .with_bearer("GET", "/.well-known/jmap", &new_access, Body::empty())
        .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_one_of_two_sessions_only_ends_that_one() {
    let h = Harness::new().await;
    let (_, first) = h.login().await;
    let (access, _) = h.login().await;
    let reply = h
        .send(
            Request::post("/oauth/revoke")
                .header(header::AUTHORIZATION, basic(CLIENT_ID, CLIENT_SECRET))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(encode(&[("token", first.as_str())])))
                .unwrap(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    // The other session goes on; the backend drops what was the first's.
    let alive = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(alive.status, StatusCode::OK);
    let seen = h.fake.seen.lock().unwrap();
    assert!(seen.stopped.is_empty());
    assert_eq!(seen.sessions_ended.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_end_at_the_maximum() {
    let mut config = config();
    config.lifetimes = crate::Lifetimes::new(
        std::time::Duration::from_mins(15),
        std::time::Duration::from_hours(24 * 30),
        std::time::Duration::from_hours(1),
    )
    .unwrap();
    let h = Harness::with_config(config).await;
    let (_, refresh) = h.login().await;
    h.advance(3000);
    let rotated = h.refresh(&refresh).await;
    assert_eq!(rotated.status, StatusCode::OK);
    let refresh = rotated.json()["refresh_token"].as_str().unwrap().to_owned();
    h.advance(700);
    assert_eq!(
        h.refresh(&refresh).await.json(),
        json!({"error": "invalid_grant"})
    );
    h.server.cleanup().await.unwrap();
    assert_eq!(h.fake.seen.lock().unwrap().stopped.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn cleanup_stops_a_running_account_whose_sessions_ended_elsewhere() {
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    let started = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(started.status, StatusCode::OK);
    let account = h.store.account_by_login("alice").await.unwrap().unwrap();
    // What the backend does when the server refuses the password.
    let now = UnixTime::from_secs(h.clock.load(Ordering::SeqCst));
    h.store
        .revoke_account_grants(account.id, now)
        .await
        .unwrap();
    // Past the grace of a recent login.
    h.advance(300);
    h.server.cleanup().await.unwrap();
    assert_eq!(h.fake.seen.lock().unwrap().stopped, [account.id]);
    // No longer started: the next request with a new session starts it.
    h.server.cleanup().await.unwrap();
    assert_eq!(h.fake.seen.lock().unwrap().stopped.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn cleanup_keeps_a_login_waiting_for_its_code_exchange() {
    let h = Harness::new().await;
    let code = h.code().await;
    h.server.cleanup().await.unwrap();
    assert!(h.fake.seen.lock().unwrap().stopped.is_empty());
    assert_eq!(h.exchange(&code).await.status, StatusCode::OK);
    // A login whose code is never exchanged is cleaned up later.
    let h = Harness::new().await;
    let _code = h.code().await;
    h.advance(300);
    h.server.cleanup().await.unwrap();
    assert_eq!(h.fake.seen.lock().unwrap().stopped.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn oauth_bodies_are_limited() {
    let h = Harness::new().await;
    let big = "x".repeat(20 * 1024);
    let reply = h
        .send(
            Request::post("/oauth/token")
                .header(header::AUTHORIZATION, basic(CLIENT_ID, CLIENT_SECRET))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "grant_type=refresh_token&refresh_token={big}"
                )))
                .unwrap(),
        )
        .await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    let form = h.open_form().await;
    let reply = h
        .post_form(
            &form,
            &[("login", &big), ("password", "x"), ("action", "login")],
        )
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_near_session_end_announces_the_real_lifetime() {
    let mut config = config();
    config.lifetimes = crate::Lifetimes::new(
        std::time::Duration::from_mins(15),
        std::time::Duration::from_hours(24 * 30),
        std::time::Duration::from_hours(1),
    )
    .unwrap();
    let h = Harness::with_config(config).await;
    let (_, refresh) = h.login().await;
    // 100 s before the session ends: the new access token lasts 100 s.
    h.advance(3500);
    let rotated = h.refresh(&refresh).await.json();
    assert_eq!(rotated["expires_in"], 100);
    // 1 s before the end: still a positive lifetime (the BFF requires it).
    h.advance(99);
    let refresh = rotated["refresh_token"].as_str().unwrap();
    let last = h.refresh(refresh).await;
    assert_eq!(last.status, StatusCode::OK, "{}", last.text());
    assert_eq!(last.json()["expires_in"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn password_deleted_when_refresh_token_expires() {
    let mut config = config();
    let day = std::time::Duration::from_hours(24);
    config.lifetimes =
        crate::Lifetimes::new(std::time::Duration::from_mins(15), day * 7, day * 90).unwrap();
    let h = Harness::with_config(config).await;
    let _tokens = h.login().await;
    let account = h.store.account_by_login("alice").await.unwrap().unwrap().id;
    h.advance(6 * 24 * 3600);
    h.server.cleanup().await.unwrap();
    assert!(h.fake.seen.lock().unwrap().stopped.is_empty());
    // Unused for 8 days: the refresh token is gone, and so is the session.
    h.advance(2 * 24 * 3600);
    h.server.cleanup().await.unwrap();
    assert_eq!(h.fake.seen.lock().unwrap().stopped, vec![account]);
    assert!(
        h.store
            .secret(account, nebula_store::SecretField::ImapPassword)
            .await
            .unwrap()
            .is_none()
    );
}

/// A refresh with a wrong client secret from `ip`.
async fn wrong_client(h: &Harness, ip: &str) -> Reply {
    h.token_request_with(
        &basic(CLIENT_ID, "wrong"),
        &[(FROM, ip)],
        &[("grant_type", "refresh_token"), ("refresh_token", "x")],
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn client_auth_failures_are_limited_per_ip() {
    let h = Harness::new().await;
    for _ in 0..10 {
        let reply = wrong_client(&h, "198.51.100.1").await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(h.server.shared.client_auth.hashes_run(), 10);
    // The 11th doesn't reach argon2, and it is a 503: a 4xx would make the
    // BFF end its sessions if it shares the address.
    let reply = wrong_client(&h, "198.51.100.1").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(reply.json().get("error").is_none());
    assert!(reply.header(header::RETRY_AFTER).is_some());
    assert_eq!(h.server.shared.client_auth.hashes_run(), 10);
    // Another address is not affected.
    let reply = wrong_client(&h, "198.51.100.2").await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.server.shared.client_auth.hashes_run(), 11);
}

#[tokio::test(flavor = "multi_thread")]
async fn remembered_client_skips_the_limit() {
    let h = Harness::new().await;
    // The BFF's secret passes argon2 once and is remembered.
    let (_, refresh) = h.login().await;
    for _ in 0..11 {
        wrong_client(&h, "198.51.100.1").await;
    }
    assert_eq!(
        wrong_client(&h, "198.51.100.1").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let reply = h
        .token_request_with(
            &basic(CLIENT_ID, CLIENT_SECRET),
            &[(FROM, "198.51.100.1")],
            &[("grant_type", "refresh_token"), ("refresh_token", &refresh)],
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
}

/// The `name=value` of a `Set-Cookie` for the known-browser cookie.
fn device_cookie(reply: &Reply) -> String {
    reply
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.starts_with("__Host-nebula-device="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

const RIGHT: [(&str, &str); 3] = [
    ("login", "alice"),
    ("password", "right password"),
    ("action", "login"),
];
const WRONG: [(&str, &str); 3] = [
    ("login", "alice"),
    ("password", "wrong"),
    ("action", "login"),
];

/// Five failures for alice from browsers without her cookie, from five
/// addresses: her login's shared window is full.
async fn lock_alice_out(h: &Harness) {
    for n in 0..5 {
        let form = h.open_form().await;
        let reply = h
            .post_form_from(&form, &format!("203.0.113.{n}"), &WRONG)
            .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }
    let form = h.open_form().await;
    let reply = h.post_form_from(&form, "203.0.113.50", &RIGHT).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test(flavor = "multi_thread")]
async fn device_cookie_survives_targeted_lockout() {
    let h = Harness::new().await;
    let form = h.open_form().await;
    let reply = h.post_form_from(&form, "198.51.100.7", &RIGHT).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let device = device_cookie(&reply);
    let set = reply
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.starts_with("__Host-nebula-device="))
        .unwrap();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains("Secure"));
    assert!(
        set.contains(&format!("Max-Age={}", 30 * 24 * 3600)),
        "{set}"
    );

    lock_alice_out(&h).await;
    // Her own browser still gets in with the right password...
    let form = h.open_form().await;
    let reply = h
        .post_form_with(
            &form,
            &[(FROM, "198.51.100.7"), ("cookie", &device)],
            &RIGHT,
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    // ...and keeps the same cookie, renewed.
    assert_eq!(device_cookie(&reply), device);
}

#[tokio::test(flavor = "multi_thread")]
async fn forged_device_cookie_is_ignored() {
    let h = Harness::new().await;
    // Alice has a known browser, but not this one.
    let form = h.open_form().await;
    assert_eq!(
        h.post_form_from(&form, "198.51.100.7", &RIGHT).await.status,
        StatusCode::SEE_OTHER
    );
    lock_alice_out(&h).await;
    let forged = format!(
        "__Host-nebula-device={}",
        nebula_crypto::Token::generate().unwrap().expose()
    );
    let form = h.open_form().await;
    let reply = h
        .post_form_with(
            &form,
            &[(FROM, "198.51.100.8"), ("cookie", &forged)],
            &RIGHT,
        )
        .await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
}

pub(crate) fn revoke_request(token: &str) -> Request<Body> {
    Request::post("/oauth/revoke")
        .header(header::AUTHORIZATION, basic(CLIENT_ID, CLIENT_SECRET))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(encode(&[("token", token)])))
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_during_first_start_after_restart_stops_the_account() {
    let h = Harness::new().await;
    let (access, refresh) = h.login().await;
    let account = h.store.account_by_login("alice").await.unwrap().unwrap().id;
    let Harness { store, dir, .. } = h;
    // After a restart, the first request starts the account; it is held
    // halfway while the last session is revoked.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let fake = Fake {
        start_gate: Some(Arc::clone(&gate)),
        ..Fake::default()
    };
    let h = Harness::on_store(config(), store, dir, fake);
    let first = Request::get("/.well-known/jmap")
        .header(header::AUTHORIZATION, format!("Bearer {access}"))
        .body(Body::empty())
        .unwrap();
    let jmap = tokio::spawn(h.router.clone().oneshot(first));
    h.fake.entered.notified().await;
    let revoke = tokio::spawn(h.router.clone().oneshot(revoke_request(&refresh)));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    gate.add_permits(1);
    jmap.await.unwrap().unwrap();
    assert_eq!(revoke.await.unwrap().unwrap().status(), StatusCode::OK);

    // The stop came after the start: nothing runs, no password is left.
    assert_eq!(
        h.fake.seen.lock().unwrap().lifecycle,
        ["started", "stopped"]
    );
    assert!(
        h.store
            .secret(account, nebula_store::SecretField::ImapPassword)
            .await
            .unwrap()
            .is_none()
    );
    let reply = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_of_another_session_keeps_a_fresh_login() {
    let h = Harness::new().await;
    let (_, old_refresh) = h.login().await;
    // A new login whose code isn't exchanged yet...
    let code = h.code().await;
    // ...while the old session is revoked.
    let reply = h.send(revoke_request(&old_refresh)).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(h.fake.seen.lock().unwrap().stopped.is_empty());
    let exchanged = h.exchange(&code).await;
    assert_eq!(exchanged.status, StatusCode::OK);
    let access = exchanged.json()["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let reply = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn other_case_needs_the_password_of_the_stored_spelling() {
    let h = Harness::new().await;
    let attempt = |login: &'static str, password: &'static str| {
        let h = &h;
        async move {
            let form = h.open_form().await;
            h.post_form(
                &form,
                &[
                    ("login", login),
                    ("password", password),
                    ("action", "login"),
                ],
            )
            .await
        }
    };
    let first = attempt("carol", "carol password").await;
    assert_eq!(first.status, StatusCode::SEE_OTHER, "{}", first.text());
    let account = h.store.account_by_login("carol").await.unwrap().unwrap();

    // Carol is another user on this server: her password is not carol's.
    let other = attempt("Carol", "other carol").await;
    assert_eq!(other.status, StatusCode::BAD_REQUEST);
    assert!(other.text().contains("Wrong username or password."));
    assert!(!query_has_code(&other));
    let again = h.store.account_by_login("carol").await.unwrap().unwrap();
    assert_eq!(again.login, "carol");
    assert_eq!(again.id, account.id);
    // carol's stored password is still hers, and only she was started.
    assert_eq!(
        h.fake.seen.lock().unwrap().started,
        vec![("carol".to_owned(), "carol password".to_owned())]
    );

    // Each mismatch counts as a failed attempt: then refused.
    for _ in 0..4 {
        let reply = attempt("Carol", "other carol").await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    }
    let limited = attempt("Carol", "other carol").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test(flavor = "multi_thread")]
async fn other_case_on_a_case_blind_server_is_the_same_account() {
    let h = Harness::new().await;
    h.code().await;
    let account = h.store.account_by_login("alice").await.unwrap().unwrap();
    let form = h.open_form().await;
    let reply = h
        .post_form(
            &form,
            &[
                ("login", "Alice"),
                ("password", "right password"),
                ("action", "login"),
            ],
        )
        .await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.text());
    assert!(query(&reply.location()).contains_key("code"));
    let again = h.store.account_by_login("ALICE").await.unwrap().unwrap();
    assert_eq!(again.id, account.id);
    // The stored spelling stays, and the backend runs with it.
    assert_eq!(again.login, "alice");
    let started = h.fake.seen.lock().unwrap().started.clone();
    assert!(
        started.iter().all(|(login, _)| login == "alice"),
        "{started:?}"
    );
}

fn query_has_code(reply: &Reply) -> bool {
    reply
        .header(header::LOCATION)
        .is_some_and(|location| location.contains("code="))
}
