// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Sieve scripts (RFC 9661) and the vacation response through the bridge's
//! ManageSieve side, against Stalwart or Dovecot
//! (Pigeonhole). Skipped unless `NEBULA_TEST_SIEVE=1` (and the IMAP tests'
//! servers, `nebula-tests`):
//!
//! ```sh
//! NEBULA_TEST_SIEVE=1 NEBULA_TEST_SERVER=dovecot cargo test -p nebula-jmap --test sieve
//! ```
//!
//! With `NEBULA_TEST_SERVER=postfix` or `dovecot-prefix`, a filter also
//! files a message that Postfix delivers into Dovecot; with
//! `dovecot-prefix`, a vacation response really answers too (nothing goes
//! through Stalwart). Each test puts the account's scripts back as they
//! were.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration test helpers"
)]

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt as _;
use nebula_http::{
    AccountContext, ApiCall, Backend, BlobBody, LoginOutcome, PushEvent, StartReason, UploadBody,
    jmap_account_id,
};
use nebula_jmap::{JmapBackend, JmapConfig, SieveSettings, SmtpSettings};
use nebula_jmap_proto::object::IMAP_NAME_PROPERTY;
use nebula_jmap_proto::types::DataType as JmapDataType;
use nebula_jmap_proto::{Capability, MethodLimits, Request, RequestLimits, State};
use nebula_mail::{CertificateCheck, ImapServer, Mail, MailConfig, TlsMode};
use nebula_managesieve::proto::ScriptName;
use nebula_managesieve::{
    Authenticated, CertificateCheck as SieveCertificates, Client, Connector, Credentials,
    SieveServer, Tls,
};
use nebula_smtp::{CertificateCheck as SmtpCertificates, Security, SmtpClient, SmtpServer};
use nebula_store::{NewAccount, Store, UnixTime};
use nebula_tests::{ServerKind, TestServer, User};
use net_guard::{NetworkAccess, Policy, Port, PortPolicy, SystemResolver, Target};
use serde_json::{Value, json};

fn connector(server: TestServer) -> Connector {
    let sieve = SieveServer::new(
        nebula_tests::HOST.parse().unwrap(),
        Port::try_from(server.sieve.port).unwrap(),
        Tls::StartTls,
    )
    .with_certificates(SieveCertificates::InsecureLoopbackOnly)
    .with_private_network(true);
    Connector::new(sieve, Arc::new(SystemResolver::new().unwrap())).unwrap()
}

fn new_backend(store: &Store, server: TestServer) -> JmapBackend {
    let mut sieve = SieveSettings::new(connector(server));
    sieve.max_script_size = 64 * 1024;
    backend_with(store, server, sieve)
}

/// Whether mail can be delivered into this server and Sieve can answer.
fn delivers(server: TestServer) -> bool {
    matches!(server.kind, ServerKind::Postfix | ServerKind::DovecotPrefix)
}

fn backend_with(store: &Store, server: TestServer, sieve: SieveSettings) -> JmapBackend {
    let imap_port = Port::try_from(server.imap.port).unwrap();
    let smtp_port = Port::try_from(server.smtp.port).unwrap();
    let host: net_guard::Host = nebula_tests::HOST.parse().unwrap();
    let imap = ImapServer::new(host.clone(), imap_port, TlsMode::Implicit)
        .with_certificates(CertificateCheck::InsecureLoopbackOnly)
        .with_private_network(true)
        .with_ports(PortPolicy::only(vec![imap_port]));
    let resolver: Arc<dyn net_guard::Resolver> = Arc::new(SystemResolver::new().unwrap());
    let mail = Mail::new(store.clone(), MailConfig::new(imap), Arc::clone(&resolver)).unwrap();
    let smtp = SmtpServer::new("localhost".parse().unwrap(), Security::ImplicitTls)
        .with_certificates(SmtpCertificates::InsecureLoopbackOnly);
    let settings = SmtpSettings::new(
        SmtpClient::new(smtp).unwrap(),
        Target::new(host, smtp_port),
        Policy::new(
            PortPolicy::only(vec![smtp_port]),
            NetworkAccess::PrivateAllowed,
        ),
    );
    let mut config = JmapConfig::new(settings);
    config.email_domain = Some(nebula_tests::DOMAIN.to_owned());
    config.sieve = Some(sieve);
    JmapBackend::new(store.clone(), mail, resolver, config)
}

async fn start(
    backend: &JmapBackend,
    store: &Store,
    server: TestServer,
    user: User,
) -> AccountContext {
    let login = user.login();
    let outcome = backend
        .verify_login(&login.parse().unwrap(), &server.password(user))
        .await;
    let LoginOutcome::Success { email } = outcome else {
        panic!("login failed: {outcome:?}")
    };
    let account = store
        .get_or_create_account(
            NewAccount {
                login,
                email: &email,
                wrapped_dek: b"test",
            },
            UnixTime::now().unwrap(),
        )
        .await
        .unwrap();
    let context = AccountContext {
        id: account.id,
        jmap_id: jmap_account_id(account.id).unwrap(),
        login: login.to_owned(),
        email,
    };
    let key = nebula_crypto::AccountKey::new(
        nebula_crypto::AccountId::new(format!("login:{login}").as_bytes()).unwrap(),
        nebula_crypto::DataKey::generate().unwrap(),
    );
    backend
        .account_started(&context, server.password(user), key, StartReason::Login)
        .await
        .unwrap();
    context
}

async fn call(backend: &JmapBackend, account: &AccountContext, calls: Value) -> Vec<Value> {
    let using = [
        Capability::Core,
        Capability::Mail,
        Capability::Submission,
        Capability::Sieve,
        Capability::VacationResponse,
    ];
    let body = json!({
        "using": using.iter().map(Capability::as_str).collect::<Vec<_>>(),
        "methodCalls": calls,
    });
    let request = Request::parse(
        body.to_string().as_bytes(),
        &RequestLimits {
            max_size_request: 10_000_000,
            max_calls_in_request: 32,
        },
        &using,
    )
    .unwrap();
    let response = backend
        .jmap(
            account,
            ApiCall {
                request,
                // Push subscriptions are per session; nothing else looks at it.
                session: nebula_store::GrantId::from_raw(1).unwrap(),
                session_state: State::new("s"),
                languages: nebula_http::AcceptLanguage::default(),
                limits: MethodLimits {
                    max_objects_in_get: 500,
                    max_objects_in_set: 500,
                    max_size_request: 10_000_000,
                },
            },
        )
        .await
        .unwrap();
    let value = serde_json::to_value(response).unwrap();
    value["methodResponses"].as_array().unwrap().clone()
}

async fn upload(backend: &JmapBackend, account: &AccountContext, script: &str) -> String {
    let uploaded = backend
        .upload(
            account,
            "application/sieve".parse().unwrap(),
            UploadBody::from_chunks(vec![bytes::Bytes::from(script.to_owned())], 1_000_000),
        )
        .await
        .unwrap();
    uploaded.blob_id.as_str().to_owned()
}

async fn download(backend: &JmapBackend, account: &AccountContext, blob: &str) -> Option<Vec<u8>> {
    let blob = backend
        .download(account, &blob.parse().unwrap())
        .await
        .unwrap()?;
    assert_eq!(blob.media_type.as_str(), "application/sieve");
    match blob.body {
        BlobBody::Bytes(bytes) => Some(bytes.to_vec()),
        _ => panic!("scripts come whole"),
    }
}

/// Waits for the background probe: the capability shows up once the
/// ManageSieve login worked.
async fn capabilities(backend: &JmapBackend, account: &AccountContext) -> Value {
    for _ in 0..100 {
        let capabilities = backend.account_capabilities(account).await.unwrap();
        if capabilities.sieve.is_some() {
            return serde_json::to_value(capabilities).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no Sieve capability");
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// Held by every test that changes Alice's scripts: they run in parallel
/// in this binary and would see each other's scripts (the active one, the
/// `vacation` script, the count).
static ALICE_SCRIPTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The account's scripts as they were, to put back at the end.
struct Saved {
    scripts: Vec<(ScriptName, bool, Vec<u8>)>,
}

async fn direct(server: TestServer, user: User) -> Client<Authenticated> {
    connector(server)
        .login(&Credentials::new(user.login(), server.password(user)))
        .await
        .unwrap()
}

async fn save(server: TestServer, user: User) -> Saved {
    let mut client = direct(server, user).await;
    let mut scripts = Vec::new();
    for script in client.list_scripts().await.unwrap() {
        let content = client.get_script(&script.name).await.unwrap();
        scripts.push((script.name, script.active, content));
    }
    client.logout().await;
    Saved { scripts }
}

async fn restore(server: TestServer, user: User, saved: Saved) {
    let mut client = direct(server, user).await;
    client.set_active(None).await.unwrap();
    for script in client.list_scripts().await.unwrap() {
        if !saved.scripts.iter().any(|(name, ..)| *name == script.name) {
            client.delete_script(&script.name).await.unwrap();
        }
    }
    for (name, active, content) in &saved.scripts {
        // Unchanged ones stay (Stalwart refuses to write `vacation`).
        if client.get_script(name).await.ok().as_ref() != Some(content) {
            client.put_script(name, content).await.unwrap();
        }
        if *active {
            client.set_active(Some(name)).await.unwrap();
        }
    }
    client.logout().await;
}

/// Alice sends to Bob through Postfix.
async fn send_to_bob(backend: &JmapBackend, alice: &AccountContext, subject: &str) {
    let a = alice.jmap_id.as_str();
    let identity = call(
        backend,
        alice,
        json!([["Identity/get", {"accountId": a}, "0"]]),
    )
    .await;
    let identity = identity[0][1]["list"][0]["id"].as_str().unwrap().to_owned();
    let drafts = call(
        backend,
        alice,
        json!([["Mailbox/query", {"accountId": a, "filter": {"role": "drafts"}}, "0"]]),
    )
    .await;
    let drafts = drafts[0][1]["ids"][0].as_str().unwrap().to_owned();
    let sent = call(
        backend,
        alice,
        json!([
            ["Email/set", {"accountId": a, "create": {"m": {
                "mailboxIds": {drafts: true},
                "from": [{"email": User::Alice.address()}],
                "to": [{"email": User::Bob.address()}],
                "subject": subject,
                "textBody": [{"partId": "t", "type": "text/plain"}],
                "bodyValues": {"t": {"value": "Filtered by Sieve."}}
            }}}, "0"],
            ["EmailSubmission/set", {"accountId": a, "create": {"s": {"emailId": "#m", "identityId": identity}},
                "onSuccessDestroyEmail": ["#s"]}, "1"]
        ]),
    )
    .await;
    assert!(sent[1][1]["created"]["s"]["id"].is_string(), "{sent:?}");
}

/// Waits for an email in `filter` and returns it, or `Null` after 30 s.
async fn wait_for_email(backend: &JmapBackend, account: &AccountContext, filter: Value) -> Value {
    let id = account.jmap_id.as_str();
    for _ in 0..60 {
        let query = call(
            backend,
            account,
            json!([
                ["Email/query", {"accountId": id, "filter": filter}, "0"],
                ["Email/get", {"accountId": id, "#ids": {"resultOf": "0", "name": "Email/query", "path": "/ids"},
                    "properties": ["subject", "from", "keywords", "mailboxIds"]}, "1"]
            ]),
        )
        .await;
        if let Some(email) = query[1][1]["list"].as_array().and_then(|l| l.first()) {
            return email.clone();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Value::Null
}

const FILTER: &str = "require [\"fileinto\"];\r\nif header :contains \"subject\" \"nebula\" {\r\n  fileinto \"INBOX\";\r\n}\r\n";

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn scripts_and_vacation() {
    let Some(server) = TestServer::sieve() else {
        return;
    };
    let _alice = ALICE_SCRIPTS.lock().await;
    let saved = save(server, User::Alice).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();

    let caps = capabilities(&backend, &alice).await;
    let sieve = &caps["urn:ietf:params:jmap:sieve"];
    let extensions: Vec<&str> = sieve["sieveExtensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert!(extensions.contains(&"vacation") && extensions.contains(&"include"));
    assert!(!extensions.contains(&"mailboxid"), "{extensions:?}");
    assert_eq!(sieve["maxSizeScript"], 65536);
    assert_eq!(sieve["maxSizeScriptName"], 512);
    // Stalwart's ManageSieve reserves the name `vacation` for its own JMAP:
    // no vacation response through the bridge there.
    let has_vacation = caps["urn:ietf:params:jmap:vacationresponse"].is_object();
    assert_eq!(has_vacation, server.kind != ServerKind::Stalwart);
    if server.kind == ServerKind::Stalwart {
        assert_eq!(sieve["maxNumberRedirects"], 1);
    }

    // Validate: a good script, a bad one (with the server's text).
    let good = upload(&backend, &alice, FILTER).await;
    let bad = upload(
        &backend,
        &alice,
        "require [\"fileinto\"];\r\nnot_a_command;\r\n",
    )
    .await;
    let checked = call(
        &backend,
        &alice,
        json!([
            ["SieveScript/validate", {"accountId": a, "blobId": good}, "0"],
            ["SieveScript/validate", {"accountId": a, "blobId": bad}, "1"],
            ["SieveScript/validate", {"accountId": a, "blobId": "bu999999"}, "2"]
        ]),
    )
    .await;
    assert_eq!(checked[0][1]["error"], Value::Null, "{checked:?}");
    assert_eq!(checked[1][1]["error"]["type"], "invalidSieve");
    assert!(
        checked[1][1]["error"]["description"]
            .as_str()
            .unwrap()
            .contains("not_a_command")
    );
    assert_eq!(checked[2][1]["error"]["type"], "blobNotFound");

    // Create and activate in one call, as the web does (`#script`).
    let name = format!("nebula-test-{}", stamp());
    let created = call(
        &backend,
        &alice,
        json!([
            ["SieveScript/set", {"accountId": a,
                "create": {"script": {"name": name, "blobId": good}},
                "onSuccessActivateScript": "#script"}, "0"],
            ["SieveScript/get", {"accountId": a}, "1"]
        ]),
    )
    .await;
    let made = &created[0][1]["created"]["script"];
    let id = made["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created:?}"))
        .to_owned();
    assert_eq!(made["isActive"], true);
    let old_state = created[0][1]["newState"].as_str().unwrap().to_owned();
    let listed = created[1][1]["list"].as_array().unwrap();
    let ours = listed.iter().find(|s| s["id"] == id.as_str()).unwrap();
    assert_eq!(ours["isActive"], true);
    assert_eq!(ours["name"], name.as_str());
    assert_eq!(listed.iter().filter(|s| s["isActive"] == true).count(), 1);
    let blob = ours["blobId"].as_str().unwrap().to_owned();
    assert_eq!(
        download(&backend, &alice, &blob).await.unwrap(),
        FILTER.as_bytes()
    );

    // Refusals: the reserved name, a name taken, an invalid script, the
    // active script destroyed.
    let refused = call(
        &backend,
        &alice,
        json!([["SieveScript/set", {"accountId": a,
            "create": {
                "v": {"name": "Vacation", "blobId": good},
                "dup": {"name": name, "blobId": good},
                "bad": {"name": format!("{name}-bad"), "blobId": bad},
                "ctl": {"name": "a\u{7}b", "blobId": good}
            },
            "destroy": [id]}, "0"]]),
    )
    .await;
    let not_created = &refused[0][1]["notCreated"];
    assert_eq!(not_created["v"]["type"], "forbidden");
    assert_eq!(not_created["dup"]["type"], "alreadyExists");
    assert_eq!(not_created["bad"]["type"], "invalidSieve");
    assert_eq!(not_created["ctl"]["type"], "invalidProperties");
    assert_eq!(
        refused[0][1]["notDestroyed"][id.as_str()]["type"],
        "sieveIsActive"
    );

    // Rename and new content: the id stays, the blob id changes, the old
    // blob is gone; `/changes` and `/query` see it.
    let second = FILTER.replace("nebula", "nebula-2");
    let second_blob = upload(&backend, &alice, &second).await;
    let renamed = format!("{name}-renamed");
    let updated = call(
        &backend,
        &alice,
        json!([
            ["SieveScript/set", {"accountId": a, "update": {id.clone(): {"name": renamed, "blobId": second_blob}}}, "0"],
            ["SieveScript/changes", {"accountId": a, "sinceState": old_state}, "1"],
            ["SieveScript/query", {"accountId": a, "filter": {"isActive": true}}, "2"],
            ["SieveScript/get", {"accountId": a, "ids": [id.clone()]}, "3"]
        ]),
    )
    .await;
    let new_blob = updated[0][1]["updated"][id.as_str()]["blobId"]
        .as_str()
        .unwrap_or_else(|| panic!("{updated:?}"))
        .to_owned();
    assert_ne!(new_blob, blob);
    assert!(
        updated[1][1]["updated"]
            .as_array()
            .unwrap()
            .contains(&json!(id))
    );
    assert_eq!(updated[2][1]["ids"], json!([id]));
    assert_eq!(updated[3][1]["list"][0]["name"], renamed.as_str());
    assert_eq!(updated[3][1]["list"][0]["isActive"], true);
    assert_eq!(
        download(&backend, &alice, &new_blob).await.unwrap(),
        second.as_bytes()
    );
    assert_eq!(download(&backend, &alice, &blob).await, None);

    if has_vacation {
        // Vacation on: its script becomes the active one (ours stops).
        let on = call(
            &backend,
            &alice,
            json!([
                ["VacationResponse/set", {"accountId": a, "update": {"singleton": {
                    "isEnabled": true, "fromDate": "2026-01-01T00:00:00Z", "toDate": "2099-12-31T23:59:59Z",
                    "subject": "Away \"now\" ${x}", "textBody": "Back soon.", "htmlBody": "<p>Back soon.</p>"
                }}}, "0"],
                ["VacationResponse/get", {"accountId": a}, "1"],
                ["SieveScript/get", {"accountId": a}, "2"]
            ]),
        )
        .await;
        assert!(
            on[0][1]["updated"]
                .as_object()
                .is_some_and(|u| u.contains_key("singleton")),
            "{on:?}"
        );
        let vacation = &on[1][1]["list"][0];
        assert_eq!(vacation["id"], "singleton");
        assert_eq!(vacation["isEnabled"], true);
        assert_eq!(vacation["subject"], "Away \"now\" ${x}");
        assert_eq!(vacation["textBody"], "Back soon.");
        assert_eq!(vacation["fromDate"], "2026-01-01T00:00:00Z");
        let scripts = on[2][1]["list"].as_array().unwrap();
        let vacation_script = scripts.iter().find(|s| s["name"] == "vacation").unwrap();
        assert_eq!(vacation_script["isActive"], true);
        assert_eq!(
            scripts.iter().find(|s| s["id"] == id.as_str()).unwrap()["isActive"],
            false
        );
        // Its script can't be edited as a script.
        let edit = call(
            &backend,
            &alice,
            json!([["SieveScript/set", {"accountId": a, "update": {vacation_script["id"].as_str().unwrap(): {"blobId": good}}}, "0"]]),
        )
        .await;
        assert_eq!(
            edit[0][1]["notUpdated"][vacation_script["id"].as_str().unwrap()]["type"],
            "forbidden"
        );

        // The web's way with its filters active: vacation saved but not
        // enabled, and our script includes it.
        let include =
            "require [\"fileinto\", \"include\"];\r\ninclude :personal :optional \"vacation\";\r\n";
        let include_blob = upload(&backend, &alice, include).await;
        let with_filters = call(
            &backend,
            &alice,
            json!([
                ["VacationResponse/set", {"accountId": a, "update": {"singleton": {"isEnabled": false}}}, "0"],
                ["SieveScript/validate", {"accountId": a, "blobId": include_blob}, "1"],
                ["SieveScript/set", {"accountId": a, "update": {id.clone(): {"blobId": include_blob}}, "onSuccessActivateScript": id}, "2"],
                ["VacationResponse/get", {"accountId": a}, "3"],
                ["SieveScript/get", {"accountId": a}, "4"]
            ]),
        )
        .await;
        assert_eq!(with_filters[1][1]["error"], Value::Null, "{with_filters:?}");
        assert!(
            with_filters[2][1]["notUpdated"].is_null(),
            "{with_filters:?}"
        );
        assert_eq!(with_filters[3][1]["list"][0]["isEnabled"], false);
        // Off keeps the text (the include still finds it).
        assert_eq!(
            with_filters[3][1]["list"][0]["subject"],
            "Away \"now\" ${x}"
        );
        let scripts = with_filters[4][1]["list"].as_array().unwrap();
        assert!(
            scripts
                .iter()
                .any(|s| s["name"] == "vacation" && s["isActive"] == false)
        );
        assert!(
            scripts
                .iter()
                .any(|s| s["id"] == id.as_str() && s["isActive"] == true)
        );
    }

    // Deactivate and destroy, as the web's delete does.
    let gone = call(
        &backend,
        &alice,
        json!([
            ["SieveScript/set", {"accountId": a, "onSuccessDeactivateScript": true}, "0"],
            ["SieveScript/set", {"accountId": a, "destroy": [id.clone()]}, "1"],
            ["SieveScript/get", {"accountId": a, "ids": [id.clone()]}, "2"]
        ]),
    )
    .await;
    assert_eq!(gone[1][1]["destroyed"], json!([id]), "{gone:?}");
    assert_eq!(gone[2][1]["notFound"], json!([id]));

    backend.account_stopped(alice.id).await;
    restore(server, User::Alice, saved).await;
}

/// A rule really files a message: Postfix delivers into Dovecot, whose
/// Sieve runs the script the bridge stored.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines, reason = "one scenario")]
async fn filter_files_delivered_mail() {
    let Some(server) = TestServer::sieve() else {
        return;
    };
    if !delivers(server) {
        return;
    }
    let saved = save(server, User::Bob).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let bob = start(&backend, &store, server, User::Bob).await;
    let b = bob.jmap_id.as_str();
    capabilities(&backend, &bob).await;

    // Bob files mail with this subject into a new subfolder, by the name
    // the server knows it by (the bridge doesn't announce `mailboxid`),
    // and flags it.
    let name = format!("Nebula Filter {}", stamp());
    let made = call(
        &backend,
        &bob,
        json!([["Mailbox/set", {"accountId": b, "create": {
            // In this order: the keys come out sorted.
            "a": {"name": format!("{name} parent")},
            "b": {"name": name, "parentId": "#a"}
        }}, "0"]]),
    )
    .await;
    let created = |key: &str| {
        made[0][1]["created"][key]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{made:?}"))
            .to_owned()
    };
    let (parent_id, folder_id) = (created("a"), created("b"));
    let names = call(
        &backend,
        &bob,
        json!([["Mailbox/get", {"accountId": b, "ids": [folder_id],
            "properties": ["name", IMAP_NAME_PROPERTY]}, "0"]]),
    )
    .await;
    let folder = names[0][1]["list"][0][IMAP_NAME_PROPERTY]
        .as_str()
        .unwrap_or_else(|| panic!("{names:?}"))
        .to_owned();
    // Exactly as the server keeps it. Never without being asked for.
    let expected = match server.kind {
        ServerKind::DovecotPrefix => format!("INBOX.{name} parent.{name}"),
        _ => format!("{name} parent/{name}"),
    };
    assert_eq!(folder, expected);
    let plain = call(
        &backend,
        &bob,
        json!([["Mailbox/get", {"accountId": b, "ids": [folder_id]}, "0"]]),
    )
    .await;
    assert!(plain[0][1]["list"][0].get(IMAP_NAME_PROPERTY).is_none());
    let subject = format!("nebula-sieve-{}", stamp());
    // The same date tests as the bridge's vacation script: one range
    // around now (applies), one in the future (doesn't).
    let date = |op: &str, when: &str| {
        format!("currentdate :zone \"+0000\" :value \"{op}\" \"iso8601\" \"{when}\"")
    };
    let script = format!(
        "require [\"fileinto\", \"imap4flags\", \"date\", \"relational\"];\r\n\
         if allof ({}, {}) {{ addflag \"nebula-in-range\"; }}\r\n\
         if {} {{ addflag \"nebula-future\"; }}\r\n\
         if header :is \"subject\" \"{subject}\" {{\r\n  addflag \"\\\\Flagged\";\r\n  fileinto \"{folder}\";\r\n}}\r\n",
        date("ge", "2000-01-01T00:00:00"),
        date("le", "2099-12-31T23:59:59"),
        date("ge", "2099-01-01T00:00:00"),
    );
    let script_blob = upload(&backend, &bob, &script).await;
    let stored = call(
        &backend,
        &bob,
        json!([["SieveScript/set", {"accountId": b,
            "create": {"s": {"name": format!("nebula-test-{}", stamp()), "blobId": script_blob}},
            "onSuccessActivateScript": "#s"}, "0"]]),
    )
    .await;
    assert!(stored[0][1]["created"]["s"]["id"].is_string(), "{stored:?}");

    send_to_bob(&backend, &alice, &subject).await;

    // It lands in the folder, flagged, and not in the inbox.
    let found = wait_for_email(
        &backend,
        &bob,
        json!({"inMailbox": folder_id, "subject": subject}),
    )
    .await;
    assert_eq!(found["keywords"]["$flagged"], true, "not filed: {found:?}");
    assert_eq!(found["keywords"]["nebula-in-range"], true, "{found:?}");
    assert!(
        found["keywords"].get("nebula-future").is_none(),
        "{found:?}"
    );
    assert_eq!(found["mailboxIds"].as_object().unwrap().len(), 1);

    let cleanup = call(
        &backend,
        &bob,
        json!([["Mailbox/set", {"accountId": b, "destroy": [folder_id, parent_id],
            "onDestroyRemoveEmails": true}, "0"]]),
    )
    .await;
    assert_eq!(
        cleanup[0][1]["destroyed"].as_array().unwrap().len(),
        2,
        "{cleanup:?}"
    );
    backend.account_stopped(alice.id).await;
    backend.account_stopped(bob.id).await;
    restore(server, User::Bob, saved).await;
}

/// The vacation response the bridge writes really answers: Bob turns it
/// on, Alice writes, and the answer comes back to her. The subject is new
/// each time: Sieve answers a sender once per subject and period.
#[tokio::test(flavor = "multi_thread")]
async fn vacation_answers_delivered_mail() {
    let Some(server) = TestServer::sieve() else {
        return;
    };
    // Its Postfix takes mail from Dovecot; the shared pair may be older.
    if server.kind != ServerKind::DovecotPrefix {
        return;
    }
    let saved = save(server, User::Bob).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let backend = new_backend(&store, server);
    let alice = start(&backend, &store, server, User::Alice).await;
    let bob = start(&backend, &store, server, User::Bob).await;
    let b = bob.jmap_id.as_str();
    capabilities(&backend, &bob).await;

    let answer = format!("nebula-away-{}", stamp());
    let turned_on = call(
        &backend,
        &bob,
        json!([["VacationResponse/set", {"accountId": b, "update": {"singleton": {
            "isEnabled": true, "subject": answer, "textBody": "Back on Monday."
        }}}, "0"]]),
    )
    .await;
    assert!(
        turned_on[0][1]["updated"].get("singleton").is_some(),
        "{turned_on:?}"
    );

    send_to_bob(&backend, &alice, &format!("nebula-question-{}", stamp())).await;
    let reply = wait_for_email(&backend, &alice, json!({"subject": answer})).await;
    assert_eq!(reply["subject"], answer, "no answer: {reply:?}");
    assert_eq!(reply["from"][0]["email"], User::Bob.address(), "{reply:?}");

    let id = reply["id"].as_str().unwrap_or_default().to_owned();
    let a = alice.jmap_id.as_str();
    call(
        &backend,
        &alice,
        json!([["Email/set", {"accountId": a, "destroy": [id]}, "0"]]),
    )
    .await;
    backend.account_stopped(alice.id).await;
    backend.account_stopped(bob.id).await;
    restore(server, User::Bob, saved).await;
}

/// A change another client makes reaches a push stream: ManageSieve can't
/// announce it, so the bridge reads the scripts while someone listens.
#[tokio::test(flavor = "multi_thread")]
async fn script_changes_are_pushed() {
    let Some(server) = TestServer::sieve() else {
        return;
    };
    let _alice = ALICE_SCRIPTS.lock().await;
    let saved = save(server, User::Alice).await;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let mut settings = SieveSettings::new(connector(server));
    settings.poll_interval = Some(Duration::from_millis(200));
    let backend = backend_with(&store, server, settings);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str().to_owned();
    capabilities(&backend, &alice).await;

    let mut events = backend.events(&alice, true).await.unwrap();
    let name = ScriptName::new(format!("nebula-push-{}", stamp())).unwrap();
    let mut other = direct(server, User::Alice).await;
    other.put_script(&name, FILTER.as_bytes()).await.unwrap();

    let wanted: JmapDataType = "SieveScript".parse().unwrap();
    let pushed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match events.next().await {
                Some(PushEvent::State(change)) => {
                    let key = change.changed.keys().find(|id| id.as_str() == a).cloned();
                    if let Some(states) = key.and_then(|key| change.changed.get(&key).cloned())
                        && states.contains_key(&wanted)
                    {
                        return true;
                    }
                }
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await;
    assert_eq!(pushed, Ok(true), "no SieveScript push");

    other.delete_script(&name).await.unwrap();
    other.logout().await;
    backend.account_stopped(alice.id).await;
    restore(server, User::Alice, saved).await;
}

/// The first vacation response is a new script: with `maxNumberScripts`
/// already reached it is `overQuota`, and the scripts can still be read
/// (one more than the cap would make every later sync refuse the listing).
#[tokio::test(flavor = "multi_thread")]
async fn vacation_respects_max_scripts() {
    let Some(server) = TestServer::sieve() else {
        return;
    };
    // Stalwart keeps `vacation` for its own JMAP (no vacation here).
    if server.kind == ServerKind::Stalwart {
        return;
    }
    let _alice = ALICE_SCRIPTS.lock().await;
    let saved = save(server, User::Alice).await;
    let vacation = ScriptName::new("vacation").unwrap();
    let mut other = direct(server, User::Alice).await;
    other.set_active(None).await.unwrap();
    if other
        .list_scripts()
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == vacation)
    {
        other.delete_script(&vacation).await.unwrap();
    }
    let extra = ScriptName::new(format!("nebula-cap-{}", stamp())).unwrap();
    other.put_script(&extra, FILTER.as_bytes()).await.unwrap();
    let count = other.list_scripts().await.unwrap().len();
    other.logout().await;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db")).await.unwrap();
    let mut settings = SieveSettings::new(connector(server));
    settings.max_script_size = 64 * 1024;
    settings.max_scripts = Some(count);
    let backend = backend_with(&store, server, settings);
    let alice = start(&backend, &store, server, User::Alice).await;
    let a = alice.jmap_id.as_str();

    let refused = call(
        &backend,
        &alice,
        json!([
            ["VacationResponse/set", {"accountId": a, "update": {"singleton": {
                "isEnabled": true, "textBody": "Back soon."
            }}}, "0"],
            ["SieveScript/get", {"accountId": a}, "1"],
            ["VacationResponse/get", {"accountId": a}, "2"]
        ]),
    )
    .await;
    assert_eq!(
        refused[0][1]["notUpdated"]["singleton"]["type"], "overQuota",
        "{refused:?}"
    );
    assert_eq!(refused[1][0], "SieveScript/get", "{refused:?}");
    assert_eq!(
        refused[1][1]["list"].as_array().map(Vec::len),
        Some(count),
        "{refused:?}"
    );
    assert_eq!(refused[2][1]["list"][0]["isEnabled"], false, "{refused:?}");

    backend.account_stopped(alice.id).await;
    restore(server, User::Alice, saved).await;
}

/// A backend logged in as Alice with two new scripts `one` and `two`
/// (none active), for the `SieveScript/set` rules below. Holds
/// [`ALICE_SCRIPTS`]; [`Fixture::done`] puts her scripts back.
struct Fixture {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    server: TestServer,
    saved: Saved,
    _dir: tempfile::TempDir,
    backend: JmapBackend,
    alice: AccountContext,
    has_vacation: bool,
    good: String,
    one: String,
    two: String,
}

impl Fixture {
    async fn new() -> Option<Self> {
        let server = TestServer::sieve()?;
        let lock = ALICE_SCRIPTS.lock().await;
        let saved = save(server, User::Alice).await;
        let mut client = direct(server, User::Alice).await;
        client.set_active(None).await.unwrap();
        client.logout().await;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).await.unwrap();
        let backend = new_backend(&store, server);
        let alice = start(&backend, &store, server, User::Alice).await;
        let caps = capabilities(&backend, &alice).await;
        let has_vacation = caps["urn:ietf:params:jmap:vacationresponse"].is_object();
        let good = upload(&backend, &alice, FILTER).await;
        let a = alice.jmap_id.as_str();
        let made = call(
            &backend,
            &alice,
            json!([["SieveScript/set", {"accountId": a, "create": {
                "one": {"name": format!("nebula-one-{}", stamp()), "blobId": good},
                "two": {"name": format!("nebula-two-{}", stamp()), "blobId": good}
            }}, "0"]]),
        )
        .await;
        let id = |key: &str| {
            made[0][1]["created"][key]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("{made:?}"))
                .to_owned()
        };
        let (one, two) = (id("one"), id("two"));
        Some(Self {
            _lock: lock,
            server,
            saved,
            _dir: dir,
            backend,
            alice,
            has_vacation,
            good,
            one,
            two,
        })
    }

    async fn call(&self, calls: Value) -> Vec<Value> {
        call(&self.backend, &self.alice, calls).await
    }

    fn account(&self) -> &str {
        self.alice.jmap_id.as_str()
    }

    async fn done(self) {
        self.backend.account_stopped(self.alice.id).await;
        restore(self.server, User::Alice, self.saved).await;
    }
}

/// RFC 9661 §2.4: an activated script is reported with `isActive: true`
/// (in `created` if new, else `updated`) and a deactivated one in
/// `updated` with `false`.
#[tokio::test(flavor = "multi_thread")]
async fn activation_changes_are_reported() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let a = f.account();
    let (one, two) = (f.one.clone(), f.two.clone());
    let on = f
        .call(json!([["SieveScript/set", {"accountId": a, "onSuccessActivateScript": one}, "0"]]))
        .await;
    assert_eq!(
        on[0][1]["updated"],
        json!({one.clone(): {"isActive": true}}),
        "{on:?}"
    );
    let switched = f
        .call(json!([["SieveScript/set", {"accountId": a, "onSuccessActivateScript": two}, "0"]]))
        .await;
    assert_eq!(
        switched[0][1]["updated"],
        json!({one.clone(): {"isActive": false}, two.clone(): {"isActive": true}}),
        "{switched:?}"
    );
    // A new script activated: in `created` with true, the old one in
    // `updated` with false.
    let created = f
        .call(json!([["SieveScript/set", {"accountId": a,
            "create": {"new": {"name": format!("nebula-new-{}", stamp()), "blobId": f.good}},
            "onSuccessActivateScript": "#new"}, "0"]]))
        .await;
    assert_eq!(
        created[0][1]["created"]["new"]["isActive"], true,
        "{created:?}"
    );
    let new = created[0][1]["created"]["new"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        created[0][1]["updated"],
        json!({two.clone(): {"isActive": false}}),
        "{created:?}"
    );
    let off = f
        .call(
            json!([["SieveScript/set", {"accountId": a, "onSuccessDeactivateScript": true}, "0"]]),
        )
        .await;
    assert_eq!(
        off[0][1]["updated"],
        json!({new: {"isActive": false}}),
        "{off:?}"
    );
    f.done().await;
}

/// RFC 9661 §2.4: an invalid or nonexistent `onSuccessActivateScript`
/// "MUST be ignored" (the changes stand, no error) and
/// `onSuccessDeactivateScript` still runs, first.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_activation_target_is_ignored() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let a = f.account();
    let one = f.one.clone();
    f.call(json!([["SieveScript/set", {"accountId": a, "onSuccessActivateScript": one}, "0"]]))
        .await;
    for target in ["z999999999", "not an id!", "#nothing"] {
        let ignored = f
            .call(json!([
                ["SieveScript/set", {"accountId": a,
                    "update": {one.clone(): {"name": format!("nebula-one-{}", stamp())}},
                    "onSuccessActivateScript": target}, "0"],
                ["SieveScript/get", {"accountId": a, "ids": [one.clone()], "properties": ["isActive"]}, "1"]
            ]))
            .await;
        assert_eq!(ignored[0][0], "SieveScript/set", "{target}: {ignored:?}");
        assert!(
            ignored[0][1]["updated"]
                .as_object()
                .unwrap()
                .contains_key(&one),
            "{target}: {ignored:?}"
        );
        assert_eq!(ignored[1][1]["list"][0]["isActive"], true, "{ignored:?}");
    }
    let off = f
        .call(
            json!([["SieveScript/set", {"accountId": a, "onSuccessActivateScript": "z999999999",
            "onSuccessDeactivateScript": true}, "0"]]),
        )
        .await;
    assert_eq!(off[0][0], "SieveScript/set", "{off:?}");
    assert_eq!(
        off[0][1]["updated"],
        json!({one: {"isActive": false}}),
        "{off:?}"
    );
    f.done().await;
}

/// RFC 9661 §2.4 / RFC 8620 §5.3: `alreadyExists` always has the
/// `existingId`, also for a script created earlier in the same call.
#[tokio::test(flavor = "multi_thread")]
async fn already_exists_names_the_script() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let a = f.account();
    let names = f
        .call(json!([["SieveScript/get", {"accountId": a, "ids": [f.one], "properties": ["name"]}, "0"]]))
        .await;
    let one_name = names[0][1]["list"][0]["name"].as_str().unwrap().to_owned();
    let four = format!("nebula-four-{}", stamp());
    let taken = f
        .call(json!([["SieveScript/set", {"accountId": a,
            "create": {"a": {"name": four, "blobId": f.good}, "b": {"name": four, "blobId": f.good}},
            "update": {f.two.clone(): {"name": one_name}}}, "0"]]))
        .await;
    let set = &taken[0][1];
    let id_four = set["created"]["a"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{taken:?}"));
    assert_eq!(set["notCreated"]["b"]["type"], "alreadyExists", "{taken:?}");
    assert_eq!(set["notCreated"]["b"]["existingId"], id_four, "{taken:?}");
    let refused = &set["notUpdated"][f.two.as_str()];
    assert_eq!(refused["type"], "alreadyExists", "{taken:?}");
    assert_eq!(refused["existingId"], f.one.as_str(), "{taken:?}");
    f.done().await;
}

/// RFC 9661 §4: the vacation response's script can be activated through
/// SieveScript/set but not destroyed, renamed or have its name taken.
#[tokio::test(flavor = "multi_thread")]
async fn vacation_script_is_protected() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    if !f.has_vacation {
        f.done().await;
        return;
    }
    let a = f.account();
    let made = f
        .call(json!([
            ["VacationResponse/set", {"accountId": a, "update": {"singleton": {
                "isEnabled": false, "textBody": "Back soon."}}}, "0"],
            ["SieveScript/query", {"accountId": a, "filter": {"name": "vacation"}}, "1"],
            ["SieveScript/get", {"accountId": a, "#ids": {"resultOf": "1", "name": "SieveScript/query", "path": "/ids"}, "properties": ["name"]}, "2"]
        ]))
        .await;
    let vacation = made[2][1]["list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "vacation")
        .unwrap_or_else(|| panic!("{made:?}"))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = f
        .call(json!([
            ["SieveScript/set", {"accountId": a, "destroy": [vacation.clone()]}, "0"],
            ["SieveScript/set", {"accountId": a, "update": {
                vacation.clone(): {"name": "holiday"}, f.one.clone(): {"name": "vacation"}}}, "1"],
            ["SieveScript/set", {"accountId": a, "onSuccessActivateScript": vacation.clone()}, "2"]
        ]))
        .await;
    assert_eq!(
        refused[0][1]["notDestroyed"][vacation.as_str()]["type"],
        "forbidden",
        "{refused:?}"
    );
    assert_eq!(
        refused[1][1]["notUpdated"][vacation.as_str()]["type"],
        "forbidden"
    );
    assert_eq!(
        refused[1][1]["notUpdated"][f.one.as_str()]["type"],
        "forbidden"
    );
    assert_eq!(
        refused[2][1]["updated"],
        json!({vacation: {"isActive": true}}),
        "{refused:?}"
    );
    f.done().await;
}
