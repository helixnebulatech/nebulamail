// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Scheduled mail sent after sign-out (`[submission]
//! send_while_signed_out`): which ends of a session keep the password for
//! held messages marked `sendWhenSignedOut`, and when it goes.

use std::time::Duration;

use axum::body::Body;
use http::StatusCode;
use nebula_store::{
    AccountId, EmailId, HeldLimits, IdentityId, NewHeld, NewSubmission, SecretField, SubmissionId,
    ThreadId, UnixTime,
};
use serde_json::json;

use super::oauth::revoke_request;
use super::{Fake, Harness, START, config};
use crate::config::{HttpConfig, SignedOutSending};

const MAX_WAIT: Duration = Duration::from_hours(1);
const DOMAIN: &str = "example.com";

fn signed_out_config() -> HttpConfig {
    let mut config = config();
    config.signed_out_sending = SignedOutSending::On { max_wait: MAX_WAIT };
    config
}

/// Holds a message of `account` until `at`, marked or not to go out after
/// sign-out.
async fn hold(h: &Harness, account: AccountId, at: i64, flag: bool) -> SubmissionId {
    let at = UnixTime::from_secs(at);
    let (alice, bob) = (format!("alice@{DOMAIN}"), format!("bob@{DOMAIN}"));
    let sent = NewSubmission {
        identity: IdentityId::from_raw(1).unwrap(),
        email: EmailId::from_raw(1).unwrap(),
        thread: ThreadId::from_raw(1).unwrap(),
        envelope: serde_json::from_value(json!({
            "mailFrom": {"email": alice},
            "rcptTo": [{"email": bob}]
        }))
        .unwrap(),
        send_at: at,
        smtp_reply: String::new(),
        expires_at: at,
        message_id: None,
        envid: None,
    };
    h.store
        .add_held_submission(
            account,
            sent,
            NewHeld {
                size: 10,
                sent_copy: false,
                send_when_signed_out: flag,
            },
            HeldLimits {
                max_count: 10,
                max_bytes: 1000,
            },
            |_| Ok(b"sealed".to_vec()),
        )
        .await
        .unwrap()
        .0
}

async fn alice(h: &Harness) -> AccountId {
    h.store.account_by_login("alice").await.unwrap().unwrap().id
}

async fn has_password(h: &Harness, account: AccountId) -> bool {
    h.store
        .secret(account, SecretField::ImapPassword)
        .await
        .unwrap()
        .is_some()
}

fn stopped(h: &Harness) -> Vec<AccountId> {
    h.fake.seen.lock().unwrap().stopped.clone()
}

fn now(h: &Harness) -> UnixTime {
    UnixTime::from_secs(h.clock.load(std::sync::atomic::Ordering::SeqCst))
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_keeps_the_password_while_flagged_mail_waits() {
    let h = Harness::with_config(signed_out_config()).await;
    let (access, refresh) = h.login().await;
    let account = alice(&h).await;
    let first = hold(&h, account, START + 600, true).await;
    let second = hold(&h, account, START + 900, true).await;
    hold(&h, account, START + 600, false).await;

    let reply = h.send(revoke_request(&refresh)).await;
    assert_eq!(reply.status, StatusCode::OK);
    // The session is over, but the account keeps running with its password.
    let dead = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(dead.status, StatusCode::UNAUTHORIZED);
    assert!(stopped(&h).is_empty());
    assert!(has_password(&h, account).await);
    // What the session owned still goes.
    assert_eq!(h.fake.seen.lock().unwrap().sessions_ended, [account]);
    h.server.cleanup().await.unwrap();
    assert!(stopped(&h).is_empty());

    // One is canceled: the other still needs it.
    h.store
        .cancel_held(account, first, now(&h), MAX_WAIT)
        .await
        .unwrap();
    h.server.cleanup().await.unwrap();
    assert!(stopped(&h).is_empty());
    assert!(has_password(&h, account).await);
    // The last one is unflagged: the next cleanup deletes the password.
    h.store
        .set_held_send_when_signed_out(account, second, false)
        .await
        .unwrap();
    h.server.cleanup().await.unwrap();
    assert_eq!(stopped(&h), [account]);
    assert!(!has_password(&h, account).await);
    // Stopped once.
    h.server.cleanup().await.unwrap();
    assert_eq!(stopped(&h), [account]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_password_goes_once_flagged_mail_is_sent_or_too_late() {
    let h = Harness::with_config(signed_out_config()).await;
    let (_, refresh) = h.login().await;
    let account = alice(&h).await;
    let sent = hold(&h, account, START + 60, true).await;
    h.send(revoke_request(&refresh)).await;
    assert!(has_password(&h, account).await);
    // Sent by the scheduler: nothing needs the password any more.
    h.store.claim_held(account, sent).await.unwrap().unwrap();
    h.server.cleanup().await.unwrap();
    assert!(stopped(&h).is_empty(), "being sent still needs it");
    h.store
        .release_held(
            account,
            sent,
            nebula_store::Release::Sent {
                smtp_reply: "250 OK".to_owned(),
                envid: None,
            },
            now(&h),
            MAX_WAIT,
        )
        .await
        .unwrap();
    h.server.cleanup().await.unwrap();
    assert_eq!(stopped(&h), [account]);
    assert!(!has_password(&h, account).await);

    // One that is still pending `MAX_WAIT` past its time is given up.
    let h = Harness::with_config(signed_out_config()).await;
    let (_, refresh) = h.login().await;
    let account = alice(&h).await;
    hold(&h, account, START + 60, true).await;
    h.send(revoke_request(&refresh)).await;
    h.advance(60 + 3599);
    h.server.cleanup().await.unwrap();
    assert!(stopped(&h).is_empty());
    h.advance(1);
    h.server.cleanup().await.unwrap();
    assert_eq!(stopped(&h), [account]);
    assert!(!has_password(&h, account).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_flag_or_the_setting_the_password_goes_at_logout() {
    // Held, not flagged.
    let h = Harness::with_config(signed_out_config()).await;
    let (_, refresh) = h.login().await;
    let account = alice(&h).await;
    hold(&h, account, START + 600, false).await;
    h.send(revoke_request(&refresh)).await;
    assert_eq!(stopped(&h), [account]);
    assert!(!has_password(&h, account).await);

    // Flagged, but the bridge doesn't send after sign-out.
    let h = Harness::new().await;
    let (_, refresh) = h.login().await;
    let account = alice(&h).await;
    hold(&h, account, START + 600, true).await;
    h.send(revoke_request(&refresh)).await;
    assert_eq!(stopped(&h), [account]);
    assert!(!has_password(&h, account).await);
    assert!(
        h.server
            .accounts_held_while_signed_out()
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reused_refresh_token_deletes_the_password_at_once() {
    let h = Harness::with_config(signed_out_config()).await;
    let (_, refresh) = h.login().await;
    let account = alice(&h).await;
    hold(&h, account, START + 600, true).await;
    let rotated = h.refresh(&refresh).await;
    assert_eq!(rotated.status, StatusCode::OK);
    // The old token again, past the grace of a racing tab: it may be stolen.
    h.advance(120);
    let reused = h.refresh(&refresh).await;
    assert_eq!(reused.json(), json!({"error": "invalid_grant"}));
    assert_eq!(stopped(&h), [account]);
    assert!(!has_password(&h, account).await);
    // And it is not started again for its held messages.
    h.server.cleanup().await.unwrap();
    assert_eq!(h.fake.seen.lock().unwrap().started.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reused_code_deletes_the_password_at_once() {
    let h = Harness::with_config(signed_out_config()).await;
    let code = h.code().await;
    assert_eq!(h.exchange(&code).await.status, StatusCode::OK);
    let account = alice(&h).await;
    hold(&h, account, START + 600, true).await;
    let reused = h.exchange(&code).await;
    assert_eq!(reused.json(), json!({"error": "invalid_grant"}));
    assert_eq!(stopped(&h), [account]);
    assert!(!has_password(&h, account).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_password_goes_at_once() {
    let h = Harness::with_config(signed_out_config()).await;
    let (access, _) = h.login().await;
    let account = alice(&h).await;
    hold(&h, account, START + 600, true).await;
    // After a restart the mail server refuses the stored password.
    let Harness { store, dir, .. } = h;
    let fake = Fake::default();
    fake.seen.lock().unwrap().refuse_password = true;
    let h = Harness::on_store(signed_out_config(), store, dir, fake);
    let reply = h
        .with_bearer("GET", "/.well-known/jmap", &access, Body::empty())
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(!has_password(&h, account).await);
    assert_eq!(stopped(&h), [account]);
}

#[tokio::test(flavor = "multi_thread")]
async fn signed_out_accounts_with_flagged_mail_start_at_boot() {
    let h = Harness::with_config(signed_out_config()).await;
    let (_, refresh) = h.login().await;
    let account = alice(&h).await;
    hold(&h, account, START + 600, true).await;
    h.send(revoke_request(&refresh)).await;
    let Harness { store, dir, .. } = h;
    let h = Harness::on_store(signed_out_config(), store, dir, Fake::default());
    let accounts = h.server.accounts_held_while_signed_out().await.unwrap();
    assert_eq!(accounts, [account]);
    h.server.start_accounts(&accounts).await;
    let reasons = h.fake.seen.lock().unwrap().reasons.clone();
    assert_eq!(reasons, [crate::backend::StartReason::Restore]);
    // Not started twice by the cleanup.
    h.server.cleanup().await.unwrap();
    assert_eq!(h.fake.seen.lock().unwrap().started.len(), 1);
}

/// The session's bridge capability.
async fn bridge_capability(h: &Harness, access: &str) -> serde_json::Value {
    let reply = h
        .with_bearer("GET", "/.well-known/jmap", access, Body::empty())
        .await;
    reply.json()["capabilities"]["https://helixnebula.tech/jmap/bridge"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_session_announces_it_only_when_on() {
    let h = Harness::with_config(signed_out_config()).await;
    let (access, _) = h.login().await;
    assert_eq!(
        bridge_capability(&h, &access).await,
        json!({"sendWhenSignedOut": true})
    );
    let h = Harness::new().await;
    let (access, _) = h.login().await;
    assert_eq!(bridge_capability(&h, &access).await, json!({}));
}
