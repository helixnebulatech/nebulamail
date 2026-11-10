// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Tests of uploads, identities and submissions.

use nebula_jmap_proto::object::Envelope;

use super::*;
use crate::account::NewAccount;
use crate::account::tests::alice;
use crate::db::test_support::store;
use crate::{
    AccountId, ChangesSince, DataType, EmailId, IdentityId, Seq, StoreError, SubmissionId, ThreadId,
};

const NOW: UnixTime = UnixTime::from_secs(2_000_000_000);

fn identity(email: &str) -> NewIdentity {
    NewIdentity {
        name: String::new(),
        email: email.to_owned(),
        reply_to: None,
        bcc: None,
        text_signature: String::new(),
        html_signature: String::new(),
        may_delete: false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_expire_and_have_a_quota() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let later = NOW.saturating_add(std::time::Duration::from_secs(60));
    let stored = store
        .put_upload(account, "image/png".into(), vec![1; 10], NOW, later, 15)
        .await
        .unwrap();
    let UploadOutcome::Stored(id) = stored else {
        panic!("not stored")
    };
    let upload = store.upload(account, id, NOW).await.unwrap().unwrap();
    assert_eq!(
        (upload.media_type.as_str(), upload.data.len()),
        ("image/png", 10)
    );
    // Bigger than the cap on its own: never fits.
    let over = store
        .put_upload(account, "image/png".into(), vec![1; 16], NOW, later, 15)
        .await
        .unwrap();
    assert_eq!(over, UploadOutcome::OverQuota);
    assert!(store.upload(account, id, NOW).await.unwrap().is_some());
    // Over the cap with the older one: the oldest is deleted to make room
    // (RFC 8620 §6).
    let UploadOutcome::Stored(second) = store
        .put_upload(account, "text/plain".into(), vec![2; 10], NOW, later, 15)
        .await
        .unwrap()
    else {
        panic!("not stored")
    };
    assert!(store.upload(account, id, NOW).await.unwrap().is_none());
    assert_eq!(
        store
            .upload(account, second, NOW)
            .await
            .unwrap()
            .unwrap()
            .data,
        vec![2; 10]
    );
    // Gone after expiry, and purged.
    assert!(
        store
            .upload(account, second, later)
            .await
            .unwrap()
            .is_none()
    );
    store.purge_expired(later).await.unwrap();
    assert!(store.upload(account, second, NOW).await.unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn uploads_are_written_and_read_in_chunks() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let pending_until = NOW.saturating_add(std::time::Duration::from_secs(60));
    let end = NOW.saturating_add(std::time::Duration::from_hours(1));
    let UploadOutcome::Stored(id) = store
        .begin_upload(account, "application/pdf".into(), NOW, pending_until)
        .await
        .unwrap()
    else {
        panic!("not begun")
    };
    for (seq, byte) in [(0, 1_u8), (1, 2), (2, 3)] {
        assert_eq!(
            store
                .append_upload_chunk(account, id, seq, vec![byte; 4], 100, NOW)
                .await
                .unwrap(),
            UploadOutcome::Stored(id)
        );
        // Unfinished: nobody can read it yet.
        assert!(store.upload_info(account, id, NOW).await.unwrap().is_none());
    }
    // Another upload can't push it out, even when the cap is reached: it
    // fails instead.
    let other = store
        .put_upload(account, "x/y".into(), vec![0; 90], NOW, end, 100)
        .await
        .unwrap();
    assert_eq!(other, UploadOutcome::OverQuota);

    assert_eq!(store.finish_upload(account, id, end).await.unwrap(), 12);
    let info = store.upload_info(account, id, NOW).await.unwrap().unwrap();
    assert_eq!((info.size, info.chunks), (12, 3));
    assert_eq!(
        store.upload_chunk(account, id, 1, NOW).await.unwrap(),
        Some(vec![2; 4])
    );
    assert_eq!(store.upload_chunk(account, id, 3, NOW).await.unwrap(), None);
    assert_eq!(
        store.upload(account, id, NOW).await.unwrap().unwrap().data,
        [[1_u8; 4], [2; 4], [3; 4]].concat()
    );
    // A finished upload takes no more chunks.
    assert!(
        store
            .append_upload_chunk(account, id, 3, vec![4], 100, NOW)
            .await
            .is_err()
    );

    // An abandoned upload is deleted at once, or when its short expiry
    // passes.
    let UploadOutcome::Stored(gone) = store
        .begin_upload(account, "x/y".into(), NOW, pending_until)
        .await
        .unwrap()
    else {
        panic!("not begun")
    };
    store.abort_upload(account, gone).await.unwrap();
    assert!(store.finish_upload(account, gone, end).await.is_err());
    let UploadOutcome::Stored(stale) = store
        .begin_upload(account, "x/y".into(), NOW, pending_until)
        .await
        .unwrap()
    else {
        panic!("not begun")
    };
    store.purge_expired(pending_until).await.unwrap();
    assert!(store.finish_upload(account, stale, end).await.is_err());
    // The finished one lives on.
    assert!(store.upload_info(account, id, NOW).await.unwrap().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn default_identity_once_and_changes() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let first = store
        .identities_or_default(account, identity("alice@example.com"))
        .await
        .unwrap();
    let again = store
        .identities_or_default(account, identity("other@example.com"))
        .await
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(first.len(), 1);
    let id = first[0].id;
    let before = store.state(account, DataType::Identity).await.unwrap();
    let after = store
        .update_identity(
            account,
            id,
            IdentityUpdate {
                name: Some("Alice".into()),
                ..IdentityUpdate::default()
            },
        )
        .await
        .unwrap();
    assert!(after > before);
    let ChangesSince::Changes(changes) = store
        .changes_since(account, DataType::Identity, before, 10)
        .await
        .unwrap()
    else {
        panic!("cannot calculate")
    };
    assert_eq!(changes.updated, vec![id.get()]);
    let missing = IdentityId::from_raw(999).unwrap();
    assert!(store.destroy_identity(account, missing).await.is_err());
    store.destroy_identity(account, id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn submissions_round_trip_and_expire() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let envelope: Envelope = serde_json::from_value(serde_json::json!({
        "mailFrom": {"email": "alice@example.com"},
        "rcptTo": [{"email": "bob@example.com"}]
    }))
    .unwrap();
    let sent = NewSubmission {
        identity: IdentityId::from_raw(1).unwrap(),
        email: EmailId::from_raw(5).unwrap(),
        thread: ThreadId::from_raw(2).unwrap(),
        envelope,
        send_at: NOW,
        smtp_reply: "250 OK".into(),
        expires_at: NOW.saturating_add(std::time::Duration::from_secs(10)),
        message_id: Some("m1@example.com".into()),
        envid: None,
    };
    let (id, seq) = store.add_submission(account, sent.clone()).await.unwrap();
    assert!(seq > Seq::ZERO);
    let list = store
        .submissions(account, Some(vec![id]), NOW)
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id, &list[0].sent), (id, &sent));
    assert_eq!(
        list[0].status["bob@example.com"].delivered,
        Delivered::Unknown
    );
    assert_eq!(list[0].status["bob@example.com"].smtp_reply, "250 OK");
    let later = NOW.saturating_add(std::time::Duration::from_secs(20));
    // Expired, not purged yet: hidden, and listed as expired.
    assert!(
        store
            .submissions(account, None, later)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .expired_submissions(account, NOW)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.expired_submissions(account, later).await.unwrap(),
        vec![id]
    );
    store.purge_expired(later).await.unwrap();
    assert!(
        store
            .expired_submissions(account, later)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .submissions(account, None, NOW)
            .await
            .unwrap()
            .is_empty()
    );
    let ChangesSince::Changes(changes) = store
        .changes_since(account, DataType::EmailSubmission, seq, 10)
        .await
        .unwrap()
    else {
        panic!("cannot calculate")
    };
    assert_eq!(changes.destroyed, vec![id.get()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reserved_submission_is_finished() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let reserved = NewSubmission {
        smtp_reply: String::new(),
        ..sent_to_bob("m9@x", None)
    };
    let (id, created) = store.add_submission(account, reserved).await.unwrap();
    let other = AccountId::from_raw(account.get() + 100).unwrap();
    assert!(matches!(
        store
            .finish_submission(other, id, "250 OK".into(), None)
            .await,
        Err(StoreError::NotFound(_))
    ));
    let finished = store
        .finish_submission(account, id, "250 2.0.0 queued".into(), Some("ENV-1".into()))
        .await
        .unwrap();
    assert!(finished > created);
    let list = store
        .submissions(account, Some(vec![id]), NOW)
        .await
        .unwrap();
    assert_eq!(list[0].sent.smtp_reply, "250 2.0.0 queued");
    assert_eq!(list[0].sent.envid.as_deref(), Some("ENV-1"));
    assert!(
        list[0]
            .status
            .values()
            .all(|status| status.smtp_reply == "250 2.0.0 queued")
    );
    let ChangesSince::Changes(changes) = store
        .changes_since(account, DataType::EmailSubmission, created, 10)
        .await
        .unwrap()
    else {
        panic!("cannot calculate")
    };
    assert_eq!(changes.updated, vec![id.get()]);
}

fn sent_to_bob(message_id: &str, envid: Option<&str>) -> NewSubmission {
    NewSubmission {
        identity: IdentityId::from_raw(1).unwrap(),
        email: EmailId::from_raw(5).unwrap(),
        thread: ThreadId::from_raw(2).unwrap(),
        envelope: serde_json::from_value(serde_json::json!({
            "mailFrom": {"email": "alice@example.com"},
            "rcptTo": [{"email": "bob@example.com"}, {"email": "carol@example.com"}]
        }))
        .unwrap(),
        send_at: NOW,
        smtp_reply: "250 OK".into(),
        expires_at: NOW.saturating_add(std::time::Duration::from_secs(3600)),
        message_id: Some(message_id.into()),
        envid: envid.map(str::to_owned),
    }
}

fn failed(email: i64, envid: Option<&str>, message_id: &str, at: UnixTime) -> DeliveryReport {
    DeliveryReport {
        email: EmailId::from_raw(email).unwrap(),
        received_at: at,
        envid: envid.map(str::to_owned),
        original_message_id: Some(message_id.into()),
        kind: ReportKind::Delivery(vec![RecipientReport {
            addresses: vec!["bob@example.com".into()],
            action: ReportAction::Failed,
            smtp_reply: Some("550 5.1.1 unknown user".into()),
        }]),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn reports_only_reach_their_own_submission() {
    let (store, _dir) = store().await;
    let alice = alice(&store).await.id;
    let mallory = store
        .get_or_create_account(
            NewAccount {
                login: "mallory",
                email: "mallory@example.com",
                wrapped_dek: b"dek-2",
            },
            UnixTime::from_secs(1000),
        )
        .await
        .unwrap()
        .id;
    let (plain, _) = store
        .add_submission(alice, sent_to_bob("m1@x", None))
        .await
        .unwrap();
    let (with_envid, _) = store
        .add_submission(alice, sent_to_bob("m2@x", Some("ENV-SECRET")))
        .await
        .unwrap();
    let (theirs, _) = store
        .add_submission(mallory, sent_to_bob("m1@x", None))
        .await
        .unwrap();
    let later = NOW.saturating_add(std::time::Duration::from_secs(60));
    let status = |list: Vec<Submission>, id| {
        list.into_iter()
            .find(|s| s.id == id)
            .map(|s| {
                (
                    s.status["bob@example.com"].delivered,
                    s.status["carol@example.com"].delivered,
                    s.dsn_emails,
                )
            })
            .unwrap()
    };

    // A report in mallory's mail never touches alice's submission with
    // the same Message-ID.
    let recorded = store
        .apply_report(mallory, failed(70, None, "m1@x", later), later)
        .await
        .unwrap();
    assert!(!recorded.is_empty());
    let alice_list = store.submissions(alice, None, later).await.unwrap();
    assert_eq!(status(alice_list.clone(), plain).0, Delivered::Unknown);
    assert_eq!(
        status(
            store.submissions(mallory, None, later).await.unwrap(),
            theirs
        )
        .0,
        Delivered::No
    );

    // By Message-ID, when the submission had no ENVID.
    let recorded = store
        .apply_report(alice, failed(71, None, "m1@x", later), later)
        .await
        .unwrap();
    assert!(recorded.state(DataType::EmailSubmission).is_some());
    let (bob, carol, reports) = status(store.submissions(alice, None, later).await.unwrap(), plain);
    assert_eq!((bob, carol), (Delivered::No, Delivered::Unknown));
    assert_eq!(reports, vec![EmailId::from_raw(71).unwrap()]);
    // The same report again: nothing changes, no new state.
    assert!(
        store
            .apply_report(alice, failed(71, None, "m1@x", later), later)
            .await
            .unwrap()
            .is_empty()
    );

    // Sent with an ENVID: the Message-ID alone is not enough, nor another
    // ENVID; the right one is.
    for report in [
        failed(72, None, "m2@x", later),
        failed(73, Some("ENV-GUESS"), "m2@x", later),
        failed(74, Some("ENV-SECRET"), "other@x", later),
    ] {
        assert!(
            store
                .apply_report(alice, report, later)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        !store
            .apply_report(alice, failed(75, Some("ENV-SECRET"), "m2@x", later), later)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        status(
            store.submissions(alice, None, later).await.unwrap(),
            with_envid
        )
        .0,
        Delivered::No
    );

    // A report dated well before the message was sent is not about it.
    let (early, _) = store
        .add_submission(alice, sent_to_bob("m3@x", None))
        .await
        .unwrap();
    let before = UnixTime::from_secs(NOW.secs() - 3600);
    assert!(
        store
            .apply_report(alice, failed(76, None, "m3@x", before), later)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        status(store.submissions(alice, None, later).await.unwrap(), early).0,
        Delivered::Unknown
    );
    assert!(store.expects_reports(alice, later, later).await.unwrap());
    assert!(!store.expects_reports(alice, before, later).await.unwrap());
}

/// A read receipt has no ENVID (RFC 8098): it matches by Message-ID also a
/// submission sent with one, but only in the account's own mail.
#[tokio::test(flavor = "multi_thread")]
async fn read_receipts_match_submissions_with_envid() {
    let (store, _dir) = store().await;
    let alice = alice(&store).await.id;
    let (id, _) = store
        .add_submission(alice, sent_to_bob("m4@x", Some("ENV-OURS")))
        .await
        .unwrap();
    let later = NOW.saturating_add(std::time::Duration::from_secs(60));
    let receipt = DeliveryReport {
        email: EmailId::from_raw(80).unwrap(),
        received_at: later,
        envid: None,
        original_message_id: Some("m4@x".into()),
        kind: ReportKind::Displayed(vec!["bob@example.com".into()]),
    };
    let recorded = store.apply_report(alice, receipt, later).await.unwrap();
    assert!(recorded.state(DataType::EmailSubmission).is_some());
    let submission = store
        .submissions(alice, None, later)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
        .unwrap();
    assert!(submission.status["bob@example.com"].displayed);
    assert!(!submission.status["carol@example.com"].displayed);
    assert_eq!(submission.mdn_emails, vec![EmailId::from_raw(80).unwrap()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn identities_have_a_cap_and_size_limits() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    for n in 0..MAX_IDENTITIES {
        store
            .create_identity(account, identity(&format!("a{n}@example.com")))
            .await
            .unwrap();
    }
    assert!(matches!(
        store.create_identity(account, identity("[EMAIL]")).await,
        Err(StoreError::OverQuota("identities"))
    ));

    let (store, _dir) = crate::db::test_support::store().await;
    let account = alice(&store).await.id;
    let long_name = NewIdentity {
        name: "n".repeat(MAX_IDENTITY_NAME + 1),
        ..identity("[EMAIL]")
    };
    assert!(matches!(
        store.create_identity(account, long_name).await,
        Err(StoreError::Invalid("identity name"))
    ));
    let long_signature = NewIdentity {
        html_signature: "s".repeat(MAX_SIGNATURE + 1),
        ..identity("[EMAIL]")
    };
    assert!(matches!(
        store.create_identity(account, long_signature).await,
        Err(StoreError::Invalid("signature"))
    ));
    let address = nebula_jmap_proto::object::email::EmailAddress {
        name: None,
        email: "[EMAIL]".to_owned(),
    };
    let many = NewIdentity {
        bcc: Some(vec![address; MAX_IDENTITY_ADDRESSES + 1]),
        ..identity("[EMAIL]")
    };
    assert!(matches!(
        store.create_identity(account, many).await,
        Err(StoreError::Invalid("identity addresses"))
    ));
    let at_limit = NewIdentity {
        name: "n".repeat(MAX_IDENTITY_NAME),
        text_signature: "s".repeat(MAX_SIGNATURE),
        ..identity("[EMAIL]")
    };
    let (id, _) = store.create_identity(account, at_limit).await.unwrap();
    let update = IdentityUpdate {
        text_signature: Some("s".repeat(MAX_SIGNATURE + 1)),
        ..IdentityUpdate::default()
    };
    assert!(matches!(
        store.update_identity(account, id, update).await,
        Err(StoreError::Invalid("signature"))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_default_identity_is_read_without_writing() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    // Two requests at once on an empty account: one default.
    let (a, b) = tokio::join!(
        store.identities_or_default(account, identity("[EMAIL]")),
        store.identities_or_default(account, identity("[EMAIL]")),
    );
    assert_eq!(a.unwrap().len(), 1);
    assert_eq!(b.unwrap().len(), 1);
    let state = store.state(account, DataType::Identity).await.unwrap();
    let list = store
        .identities_or_default(account, identity("[EMAIL]"))
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        store.state(account, DataType::Identity).await.unwrap(),
        state
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_uploads_count_against_a_row_limit() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let later = NOW.saturating_add(std::time::Duration::from_hours(1));
    let mut first = None;
    for n in 0..MAX_UPLOADS_PER_ACCOUNT {
        let UploadOutcome::Stored(id) = store
            .put_upload(account, "x/y".into(), Vec::new(), NOW, later, 100)
            .await
            .unwrap()
        else {
            panic!("upload {n} not stored")
        };
        first.get_or_insert(id);
    }
    let first = first.unwrap();
    // One more: the oldest finished one makes room.
    let UploadOutcome::Stored(newest) = store
        .put_upload(account, "x/y".into(), Vec::new(), NOW, later, 100)
        .await
        .unwrap()
    else {
        panic!("not stored")
    };
    assert!(
        store
            .upload_info(account, first, NOW)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .upload_info(account, newest, NOW)
            .await
            .unwrap()
            .is_some()
    );

    // All pending: nothing can make room.
    let (store, _dir) = crate::db::test_support::store().await;
    let account = alice(&store).await.id;
    let pending_until = NOW.saturating_add(std::time::Duration::from_secs(60));
    for _ in 0..MAX_UPLOADS_PER_ACCOUNT {
        let begun = store
            .begin_upload(account, "x/y".into(), NOW, pending_until)
            .await
            .unwrap();
        assert!(matches!(begun, UploadOutcome::Stored(_)));
    }
    assert_eq!(
        store
            .begin_upload(account, "x/y".into(), NOW, pending_until)
            .await
            .unwrap(),
        UploadOutcome::OverQuota
    );
    // Expired ones don't count.
    let begun = store
        .begin_upload(account, "x/y".into(), pending_until, later)
        .await
        .unwrap();
    assert!(matches!(begun, UploadOutcome::Stored(_)));
}

const LIMITS: HeldLimits = HeldLimits {
    max_count: 2,
    max_bytes: 100,
};

fn held_to_bob(at: UnixTime) -> NewSubmission {
    NewSubmission {
        send_at: at,
        smtp_reply: String::new(),
        expires_at: at.saturating_add(std::time::Duration::from_hours(1)),
        ..sent_to_bob("held@x", None)
    }
}

/// Adds a held submission whose "sealed" message is its id's text.
async fn hold(
    store: &Store,
    account: AccountId,
    at: UnixTime,
    size: u64,
) -> Result<SubmissionId, StoreError> {
    store
        .add_held_submission(
            account,
            held_to_bob(at),
            NewHeld {
                size,
                sent_copy: true,
                send_when_signed_out: false,
            },
            LIMITS,
            |id| Ok(format!("sealed {}", id.get()).into_bytes()),
        )
        .await
        .map(|(id, _)| id)
}

const LIFETIME: std::time::Duration = std::time::Duration::from_hours(1);

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one life of a held message")]
async fn held_submissions_wait_retry_and_go_out() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let at = NOW.saturating_add(std::time::Duration::from_secs(60));
    let id = hold(&store, account, at, 40).await.unwrap();
    // The limits: count, then bytes.
    assert!(matches!(
        hold(&store, account, at, 61).await,
        Err(StoreError::OverQuota(_))
    ));
    let second = hold(&store, account, at, 60).await.unwrap();
    assert!(matches!(
        hold(&store, account, at, 1).await,
        Err(StoreError::OverQuota(_))
    ));
    let listed = store
        .submissions(account, Some(vec![id]), NOW)
        .await
        .unwrap();
    assert_eq!(listed[0].undo, UndoState::Pending);
    assert_eq!(listed[0].sent.send_at, at);
    // Never hidden or purged while held, even past `expires_at`.
    let much_later = at.saturating_add(std::time::Duration::from_hours(2));
    store.purge_expired(much_later).await.unwrap();
    assert_eq!(
        store
            .submissions(account, None, much_later)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .expired_submissions(account, much_later)
            .await
            .unwrap()
            .is_empty()
    );
    // A report can't touch a held message.
    let report = store
        .apply_report(account, failed(9, None, "held@x", much_later), much_later)
        .await
        .unwrap();
    assert!(report.is_empty());
    // Not due before its time.
    assert_eq!(store.next_held_attempt().await.unwrap(), Some(at));
    assert!(store.due_held(NOW, 10).await.unwrap().is_empty());
    let due = store.due_held(at, 10).await.unwrap();
    assert_eq!(due.iter().map(|d| d.id).collect::<Vec<_>>(), [id, second]);
    assert_eq!((due[0].account, due[0].attempts), (account, 0));
    // Claimed: too late to cancel, not due again.
    let claimed = store.claim_held(account, id).await.unwrap().unwrap();
    assert_eq!(
        claimed.sealed_message,
        format!("sealed {}", id.get()).into_bytes()
    );
    assert!(claimed.sent_copy);
    assert_eq!(claimed.submission.undo, UndoState::Sending);
    assert!(store.claim_held(account, id).await.unwrap().is_none());
    assert_eq!(
        store.cancel_held(account, id, at, LIFETIME).await.unwrap(),
        Cancel::TooLate
    );
    assert_eq!(store.due_held(at, 10).await.unwrap().len(), 1);
    // A temporary failure: pending again, later.
    let retry_at = at.saturating_add(std::time::Duration::from_mins(2));
    assert_eq!(
        store
            .release_held(
                account,
                id,
                Release::Retry {
                    at: retry_at,
                    failed: true
                },
                at,
                LIFETIME
            )
            .await
            .unwrap(),
        None
    );
    let listed = store
        .submissions(account, Some(vec![id]), at)
        .await
        .unwrap();
    assert_eq!(listed[0].undo, UndoState::Pending);
    assert_eq!(store.due_held(at, 10).await.unwrap().len(), 1);
    let due = store.due_held(retry_at, 10).await.unwrap();
    assert_eq!(due.len(), 2);
    assert_eq!(due.iter().find(|d| d.id == id).unwrap().attempts, 1);
    // Sent: final, its message gone, the change recorded.
    let before = store
        .state(account, DataType::EmailSubmission)
        .await
        .unwrap();
    store.claim_held(account, id).await.unwrap().unwrap();
    let seq = store
        .release_held(
            account,
            id,
            Release::Sent {
                smtp_reply: "250 2.0.0 OK".into(),
                envid: Some("E1".into()),
            },
            retry_at,
            LIFETIME,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(seq > before);
    let listed = store
        .submissions(account, Some(vec![id]), retry_at)
        .await
        .unwrap();
    assert_eq!(listed[0].undo, UndoState::Final);
    assert_eq!(listed[0].sent.send_at, retry_at);
    assert_eq!(listed[0].sent.smtp_reply, "250 2.0.0 OK");
    assert_eq!(listed[0].sent.envid.as_deref(), Some("E1"));
    assert!(store.claim_held(account, id).await.unwrap().is_none());
    assert!(matches!(
        store
            .release_held(account, id, Release::Unknown, retry_at, LIFETIME)
            .await,
        Err(StoreError::NotFound(_))
    ));
    // Its room is free again.
    hold(&store, account, at, 40).await.unwrap();
    // It expires `LIFETIME` after it went out.
    let gone = retry_at.saturating_add(LIFETIME);
    store.purge_expired(gone).await.unwrap();
    assert!(
        store
            .submissions(account, Some(vec![id]), NOW)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn held_submissions_cancel_once() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let at = NOW.saturating_add(std::time::Duration::from_secs(60));
    let id = hold(&store, account, at, 10).await.unwrap();
    let other = AccountId::from_raw(account.get() + 100).unwrap();
    assert!(matches!(
        store.cancel_held(other, id, NOW, LIFETIME).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.cancel_held(account, id, NOW, LIFETIME).await.unwrap(),
        Cancel::Canceled(_)
    ));
    assert_eq!(
        store.cancel_held(account, id, NOW, LIFETIME).await.unwrap(),
        Cancel::AlreadyCanceled
    );
    assert!(store.claim_held(account, id).await.unwrap().is_none());
    assert!(store.due_held(at, 10).await.unwrap().is_empty());
    assert_eq!(store.next_held_attempt().await.unwrap(), None);
    let listed = store.submissions(account, None, NOW).await.unwrap();
    assert_eq!(listed[0].undo, UndoState::Canceled);
    // A sent one can't be canceled.
    let (sent, _) = store
        .add_submission(account, sent_to_bob("now@x", None))
        .await
        .unwrap();
    assert_eq!(
        store
            .cancel_held(account, sent, NOW, LIFETIME)
            .await
            .unwrap(),
        Cancel::TooLate
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn held_submissions_after_a_stop_and_a_failure() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let first = hold(&store, account, NOW, 10).await.unwrap();
    let second = hold(&store, account, NOW, 10).await.unwrap();
    // The bridge stopped while sending the first: it may have gone out.
    store.claim_held(account, first).await.unwrap().unwrap();
    assert_eq!(
        store.settle_interrupted_held(NOW, LIFETIME).await.unwrap(),
        1
    );
    let listed = store
        .submissions(account, Some(vec![first]), NOW)
        .await
        .unwrap();
    assert_eq!(listed[0].undo, UndoState::Final);
    assert_eq!(listed[0].sent.smtp_reply, "");
    // The second waits for the account (not signed in), far ahead...
    store.claim_held(account, second).await.unwrap().unwrap();
    let far = NOW.saturating_add(std::time::Duration::from_hours(24));
    store
        .release_held(
            account,
            second,
            Release::Retry {
                at: far,
                failed: false,
            },
            NOW,
            LIFETIME,
        )
        .await
        .unwrap();
    let later = NOW.saturating_add(std::time::Duration::from_secs(5));
    assert!(store.due_held(later, 10).await.unwrap().is_empty());
    // ...until the account starts.
    store.wake_held(account, later).await.unwrap();
    let due = store.due_held(later, 10).await.unwrap();
    assert_eq!((due[0].id, due[0].attempts), (second, 0));
    // Then it fails for good: every recipient says why.
    store.claim_held(account, second).await.unwrap().unwrap();
    let status: std::collections::BTreeMap<String, RecipientStatus> =
        ["bob@example.com", "carol@example.com"]
            .into_iter()
            .map(|rcpt| {
                (
                    rcpt.to_owned(),
                    RecipientStatus {
                        smtp_reply: "550 5.7.1 not sent".into(),
                        delivered: Delivered::No,
                        displayed: false,
                    },
                )
            })
            .collect();
    store
        .release_held(
            account,
            second,
            Release::Failed {
                status: status.clone(),
            },
            later,
            LIFETIME,
        )
        .await
        .unwrap()
        .unwrap();
    let listed = store
        .submissions(account, Some(vec![second]), later)
        .await
        .unwrap();
    assert_eq!(listed[0].undo, UndoState::Final);
    assert_eq!(listed[0].status, status);
    assert_eq!(listed[0].sent.smtp_reply, "550 5.7.1 not sent");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "every way a flagged message stops counting"
)]
async fn held_while_signed_out_counts_flagged_messages_until_settled() {
    let (store, _dir) = store().await;
    let account = alice(&store).await.id;
    let bob_email = format!("bob@{}", "example.com");
    let bob = store
        .get_or_create_account(
            NewAccount {
                login: "bob",
                email: &bob_email,
                wrapped_dek: b"dek",
            },
            NOW,
        )
        .await
        .unwrap()
        .id;
    let wait = std::time::Duration::from_hours(1);
    let at = NOW.saturating_add(std::time::Duration::from_secs(60));
    let limits = HeldLimits {
        max_count: 10,
        max_bytes: 1000,
    };
    let add = |account: AccountId, flag: bool| {
        let store = store.clone();
        async move {
            store
                .add_held_submission(
                    account,
                    held_to_bob(at),
                    NewHeld {
                        size: 1,
                        sent_copy: false,
                        send_when_signed_out: flag,
                    },
                    limits,
                    |_| Ok(b"sealed".to_vec()),
                )
                .await
                .unwrap()
                .0
        }
    };
    let count = |now: UnixTime| {
        let store = store.clone();
        async move {
            store
                .held_while_signed_out(account, now, wait)
                .await
                .unwrap()
        }
    };
    let plain = add(account, false).await;
    assert_eq!(count(NOW).await, 0);
    assert!(
        store
            .accounts_held_while_signed_out(NOW, wait)
            .await
            .unwrap()
            .is_empty()
    );
    let first = add(account, true).await;
    let second = add(account, true).await;
    add(bob, true).await;
    assert_eq!(count(NOW).await, 2);
    assert_eq!(
        store
            .accounts_held_while_signed_out(NOW, wait)
            .await
            .unwrap(),
        [account, bob]
    );
    // Shown by `submissions`, only on the flagged ones.
    let listed = store.submissions(account, None, NOW).await.unwrap();
    let flag = |id: SubmissionId| {
        listed
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .send_when_signed_out
    };
    assert!(!flag(plain) && flag(first) && flag(second));
    // Past its last chance it no longer counts (the scheduler fails it).
    let past = at.saturating_add(wait);
    assert_eq!(count(past).await, 0);
    assert_eq!(count(UnixTime::from_secs(past.secs() - 1)).await, 2);
    // Unflagged while pending: changes the state once.
    let before = store
        .state(account, DataType::EmailSubmission)
        .await
        .unwrap();
    assert!(matches!(
        store
            .set_held_send_when_signed_out(account, plain, true)
            .await
            .unwrap(),
        SetSignedOut::Set(Some(_))
    ));
    assert_eq!(
        store
            .set_held_send_when_signed_out(account, plain, true)
            .await
            .unwrap(),
        SetSignedOut::Set(None)
    );
    assert_ne!(
        store
            .state(account, DataType::EmailSubmission)
            .await
            .unwrap(),
        before
    );
    assert_eq!(count(NOW).await, 3);
    store
        .set_held_send_when_signed_out(account, plain, false)
        .await
        .unwrap();
    assert_eq!(count(NOW).await, 2);
    // Another account's submission is not found.
    assert!(matches!(
        store.set_held_send_when_signed_out(bob, first, false).await,
        Err(StoreError::NotFound(_))
    ));
    // Canceled: gone, and it can no longer be changed.
    store
        .cancel_held(account, first, NOW, LIFETIME)
        .await
        .unwrap();
    assert_eq!(count(NOW).await, 1);
    assert_eq!(
        store
            .set_held_send_when_signed_out(account, first, true)
            .await
            .unwrap(),
        SetSignedOut::NotPending
    );
    // Being sent still needs the password; sent, it no longer does.
    store.claim_held(account, second).await.unwrap().unwrap();
    assert_eq!(count(NOW).await, 1);
    assert_eq!(
        store
            .set_held_send_when_signed_out(account, second, false)
            .await
            .unwrap(),
        SetSignedOut::NotPending
    );
    store
        .release_held(
            account,
            second,
            Release::Sent {
                smtp_reply: "250 OK".into(),
                envid: None,
            },
            at,
            LIFETIME,
        )
        .await
        .unwrap();
    assert_eq!(count(NOW).await, 0);
    let listed = store.submissions(account, None, NOW).await.unwrap();
    assert!(listed.iter().all(|s| !s.send_when_signed_out));
    assert_eq!(
        store
            .accounts_held_while_signed_out(NOW, wait)
            .await
            .unwrap(),
        [bob]
    );
}
