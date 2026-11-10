// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The mail methods added on 2026-11-08, against the real
//! servers:
//!
//! ```sh
//! NEBULA_TEST_IMAP=1 cargo test -p nebula-jmap --test server mail_methods
//! NEBULA_TEST_IMAP=1 NEBULA_TEST_SERVER=dovecot cargo test -p nebula-jmap --test server mail_methods
//! ```
//!
//! - `Email/parse` (RFC 8621 §4.9) of an upload, and of the message
//!   attached to it (`bn…` blob ids), whose parts download.
//! - Header forms (RFC 8621 §4.1.2): Raw keeps trailing white space, Text
//!   only loses the leading SP and is in NFC.
//! - `Mailbox/queryChanges` (RFC 8621 §2.4), with and without a tree.
//! - `Email/copy` and `Blob/copy` (RFC 8620 §5.4, §6.3) with one account.

use std::time::{SystemTime, UNIX_EPOCH};

use nebula_http::{Backend as _, UploadBody};
use nebula_store::Store;
use nebula_tests::{TestServer, User};
use serde_json::{Value, json};

use super::{call, downloaded, mailbox, new_backend, start};

fn message(stamp: u128) -> Vec<u8> {
    format!(
        "From: Ana <ana@example.com>\r\n\
To: bob@example.com\r\n\
Subject:  parse {stamp} cafe\u{301}  \r\n\
X-Folded: a\r\n  b  \r\n\
Message-ID: <parse-{stamp}@example.com>\r\n\
Date: Thu, 24 Oct 2026 10:00:00 +0200\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=outer\r\n\
\r\n\
--outer\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Fuera\r\n\
--outer\r\n\
Content-Type: message/rfc822\r\n\
Content-Disposition: attachment; filename=inner.eml\r\n\
\r\n\
From: Bob <bob@example.com>\r\n\
Subject: inner {stamp}\r\n\
Content-Type: multipart/mixed; boundary=inner\r\n\
\r\n\
--inner\r\n\
Content-Type: text/plain\r\n\
\r\n\
Dentro\r\n\
--inner\r\n\
Content-Type: application/octet-stream\r\n\
Content-Disposition: attachment; filename=x.bin\r\n\
\r\n\
XBIN\r\n\
--inner--\r\n\
--outer--\r\n"
    )
    .into_bytes()
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn parse_headers_query_changes_and_copies() {
    let Some(server) = TestServer::imap() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();
    let drafts = mailbox(&backend, &alice, "drafts").await;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let raw = message(stamp);
    let uploaded = backend
        .upload(
            &alice,
            "message/rfc822".parse().unwrap(),
            UploadBody::from_chunks(vec![bytes::Bytes::from(raw.clone())], 10_000_000),
        )
        .await
        .unwrap();
    let blob = uploaded.blob_id.as_str().to_owned();

    // Email/parse of the upload.
    let parsed = call(
        &backend,
        &alice,
        json!([["Email/parse", {"accountId": a, "blobIds": [blob, "bu999999", "bm999999"],
            "properties": ["id", "blobId", "subject", "attachments", "textBody",
                "header:X-Folded", "header:X-Folded:asText"],
            "bodyProperties": ["partId", "blobId", "type", "name"]}, "0"]]),
    )
    .await;
    assert_eq!(parsed[0][0], "Email/parse", "{parsed:?}");
    let email = &parsed[0][1]["parsed"][&blob];
    assert_eq!(email["id"], Value::Null);
    assert_eq!(email["blobId"], blob.as_str());
    assert_eq!(email["subject"], format!("parse {stamp} caf\u{e9}  "));
    assert_eq!(email["header:X-Folded"], " a\r\n  b  ");
    assert_eq!(email["header:X-Folded:asText"], "a  b  ");
    assert_eq!(
        parsed[0][1]["notFound"],
        json!(["bu999999", "bm999999"]),
        "{parsed:?}"
    );
    let attached = email["attachments"][0]["blobId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(attached, format!("bn2_{blob}"));
    assert_eq!(email["attachments"][0]["type"], "message/rfc822");

    // The attached message, parsed in turn; its attachment downloads.
    let inner = call(
        &backend,
        &alice,
        json!([["Email/parse", {"accountId": a, "blobIds": [attached],
            "properties": ["subject", "from", "attachments", "hasAttachment"]}, "0"]]),
    )
    .await;
    let inner_email = &inner[0][1]["parsed"][&attached];
    assert_eq!(
        inner_email["subject"],
        format!("inner {stamp}"),
        "{inner:?}"
    );
    assert_eq!(inner_email["hasAttachment"], true);
    let bin = inner_email["attachments"][0]["blobId"].as_str().unwrap();
    assert_eq!(bin, format!("bn2_{attached}"));
    let (_, media_type, bytes) = downloaded(&backend, &alice, bin).await;
    assert_eq!(media_type, "application/octet-stream");
    assert_eq!(bytes, b"XBIN");

    // Imported, the same header forms through Email/get, and the stored
    // message's parts keep their `bp…` ids in Email/parse.
    let imported = call(
        &backend,
        &alice,
        json!([["Email/import", {"accountId": a, "emails": {"m": {
            "blobId": blob, "mailboxIds": {drafts.clone(): true}, "keywords": {"$draft": true}
        }}}, "0"]]),
    )
    .await;
    let id = imported[0][1]["created"]["m"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{imported:?}"))
        .to_owned();
    let got = call(
        &backend,
        &alice,
        json!([["Email/get", {"accountId": a, "ids": [id],
            "properties": ["subject", "blobId", "header:Subject", "header:Subject:asText"]}, "0"]]),
    )
    .await;
    let got = &got[0][1]["list"][0];
    assert_eq!(
        got["header:Subject"],
        format!("  parse {stamp} cafe\u{301}  ")
    );
    assert_eq!(
        got["header:Subject:asText"],
        format!("parse {stamp} caf\u{e9}  ")
    );
    assert_eq!(got["subject"], got["header:Subject:asText"]);
    let message_blob = got["blobId"].as_str().unwrap().to_owned();
    let reparsed = call(
        &backend,
        &alice,
        json!([["Email/parse", {"accountId": a, "blobIds": [message_blob],
            "properties": ["textBody"]}, "0"]]),
    )
    .await;
    let part = reparsed[0][1]["parsed"][&message_blob]["textBody"][0]["blobId"]
        .as_str()
        .unwrap();
    assert!(part.starts_with("bp"), "{part}");

    // Mailbox/queryChanges.
    let query = call(
        &backend,
        &alice,
        json!([["Mailbox/query", {"accountId": a, "sortAsTree": true}, "0"]]),
    )
    .await;
    assert_eq!(query[0][1]["canCalculateChanges"], true);
    let state = query[0][1]["queryState"].as_str().unwrap().to_owned();
    let created = call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "create": {
            "p": {"name": format!("qc{stamp}")},
            "c": {"name": "child", "parentId": "#p"}
        }}, "0"]]),
    )
    .await;
    let parent = created[0][1]["created"]["p"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created:?}"))
        .to_owned();
    let child = created[0][1]["created"]["c"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let changes = call(
        &backend,
        &alice,
        json!([["Mailbox/queryChanges", {"accountId": a, "sinceQueryState": state,
            "sortAsTree": true, "calculateTotal": true}, "0"]]),
    )
    .await;
    assert_eq!(changes[0][0], "Mailbox/queryChanges", "{changes:?}");
    let added: Vec<&str> = changes[0][1]["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(added.contains(&parent.as_str()) && added.contains(&child.as_str()));
    // Renaming the parent moves the child in a tree: it must be there too.
    let state = changes[0][1]["newQueryState"].as_str().unwrap().to_owned();
    call(
        &backend,
        &alice,
        json!([["Mailbox/set", {"accountId": a, "update": {
            parent.clone(): {"name": format!("zz{stamp}")}
        }}, "0"]]),
    )
    .await;
    let changes = call(
        &backend,
        &alice,
        json!([["Mailbox/queryChanges", {"accountId": a, "sinceQueryState": state,
            "sortAsTree": true}, "0"]]),
    )
    .await;
    let removed = changes[0][1]["removed"].as_array().unwrap();
    assert!(removed.contains(&json!(parent)), "{changes:?}");
    assert!(removed.contains(&json!(child)), "{changes:?}");
    let bad = call(
        &backend,
        &alice,
        json!([["Mailbox/queryChanges", {"accountId": a, "sinceQueryState": "nope"}, "0"]]),
    )
    .await;
    assert_eq!(bad[0][1]["type"], "cannotCalculateChanges");

    // Email/copy and Blob/copy with one mail account.
    let copies = call(
        &backend,
        &alice,
        json!([
            ["Email/copy", {"accountId": a, "fromAccountId": a,
                "create": {"k": {"id": id.clone(), "mailboxIds": {drafts.clone(): true}}}}, "0"],
            ["Email/copy", {"accountId": a, "fromAccountId": "nobody",
                "create": {"k": {"id": id.clone()}}}, "1"],
            ["Blob/copy", {"accountId": a, "fromAccountId": a,
                "blobIds": [blob, "bu999999"]}, "2"],
            ["Blob/copy", {"accountId": a, "fromAccountId": "nobody", "blobIds": [blob]}, "3"]
        ]),
    )
    .await;
    assert_eq!(copies[0][1]["type"], "invalidArguments", "{copies:?}");
    assert_eq!(copies[1][1]["type"], "fromAccountNotFound");
    assert_eq!(copies[2][0], "Blob/copy");
    assert_eq!(copies[2][1]["copied"], json!({blob.clone(): blob}));
    assert_eq!(copies[2][1]["notCopied"]["bu999999"]["type"], "notFound");
    assert_eq!(copies[3][1]["type"], "fromAccountNotFound");

    // Clean up.
    call(
        &backend,
        &alice,
        json!([
            ["Email/set", {"accountId": a, "destroy": [id]}, "0"],
            ["Mailbox/set", {"accountId": a, "destroy": [child]}, "1"],
            ["Mailbox/set", {"accountId": a, "destroy": [parent]}, "2"]
        ]),
    )
    .await;
}
