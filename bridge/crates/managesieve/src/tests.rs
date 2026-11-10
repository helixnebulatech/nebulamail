// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The command logic against a scripted server over an in-memory stream
//! (TLS and STARTTLS are covered by `tests/fake.rs` with real sockets).

use std::time::Duration;

use nebula_managesieve_proto::{Command, CommandDecoder, Limits as FrameLimits, ScriptName};
use secrecy::SecretString;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _, DuplexStream};

use crate::{CheckOutcome, Client, Error, Limits};

const GREETING: &[u8] = b"\"IMPLEMENTATION\" \"Fake\"\r\n\"SASL\" \"PLAIN\"\r\n\
\"SIEVE\" \"fileinto vacation include\"\r\n\"VERSION\" \"1.0\"\r\nOK \"ready\"\r\n";

fn limits() -> Limits {
    Limits {
        read_timeout: Duration::from_secs(2),
        command_timeout: Duration::from_secs(4),
        greeting_timeout: Duration::from_secs(2),
        ..Limits::default()
    }
}

/// A server that answers each command with the next reply of `script`
/// (bytes), and records the commands.
fn server(
    mut stream: DuplexStream,
    greeting: &'static [u8],
    replies: Vec<&'static [u8]>,
) -> tokio::task::JoinHandle<Vec<Command>> {
    tokio::spawn(async move {
        let mut seen = Vec::new();
        if stream.write_all(greeting).await.is_err() {
            return seen;
        }
        let mut decoder = CommandDecoder::new(FrameLimits::default());
        let mut replies = replies.into_iter().peekable();
        let mut buffer = [0_u8; 4096];
        loop {
            let command = loop {
                if let Ok(Some(command)) = decoder.decode_command() {
                    break command;
                }
                match stream.read(&mut buffer).await {
                    Ok(0) | Err(_) => return seen,
                    Ok(n) => decoder.feed(&buffer[..n]),
                }
            };
            seen.push(command);
            let Some(reply) = replies.next() else {
                return seen;
            };
            // After the last reply the server hangs up.
            if stream.write_all(reply).await.is_err() || replies.peek().is_none() {
                return seen;
            }
        }
    })
}

async fn logged_in(
    replies: Vec<&'static [u8]>,
) -> (
    Result<Client<crate::Authenticated>, Error>,
    tokio::task::JoinHandle<Vec<Command>>,
) {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let mut all = vec![&b"OK \"Logged in.\"\r\n"[..], GREETING];
    all.extend(replies);
    let task = server(theirs, GREETING, all);
    let client = Client::connect_test(ours, limits()).await.unwrap();
    let client = client.login("alice", &SecretString::from("secret")).await;
    (client, task)
}

fn name(text: &str) -> ScriptName {
    ScriptName::new(text).unwrap()
}

#[tokio::test]
async fn login_and_commands() {
    let (client, task) = logged_in(vec![
        b"\"a\"\r\n\"vacation\" ACTIVE\r\n\"bad\x01name\"\r\nOK\r\n",
        b"{13}\r\nrequire [];\r\n\r\nOK\r\n",
        b"OK (WARNINGS) \"line 1: unused\"\r\n",
        b"NO \"line 3: unknown command 'foo'\"\r\n",
        b"NO (NONEXISTENT) \"no such script\"\r\n",
        b"OK\r\n",
        b"NO (QUOTA/MAXSIZE) \"too big\"\r\n",
        b"OK\r\n",
    ])
    .await;
    let mut client = client.unwrap();
    assert!(client.capabilities().has_extension("vacation"));
    let scripts = client.list_scripts().await.unwrap();
    assert_eq!(
        scripts
            .iter()
            .map(|s| (s.name.as_str(), s.active))
            .collect::<Vec<_>>(),
        [("a", false), ("vacation", true)]
    );
    assert_eq!(
        client.get_script(&name("a")).await.unwrap(),
        b"require [];\r\n"
    );
    assert_eq!(
        client.put_script(&name("a"), b"x").await.unwrap(),
        Some("line 1: unused".to_owned())
    );
    assert_eq!(
        client.check_script(b"foo;").await.unwrap(),
        CheckOutcome::Invalid("line 3: unknown command 'foo'".to_owned())
    );
    let error = client.set_active(Some(&name("zzz"))).await.unwrap_err();
    assert!(matches!(
        error.code(),
        Some(nebula_managesieve_proto::ResponseCode::Nonexistent)
    ));
    client.set_active(None).await.unwrap();
    assert!(!client.have_space(&name("a"), 1 << 30).await.unwrap());
    client.logout().await;
    let seen = task.await.unwrap();
    assert!(matches!(seen.first(), Some(Command::Authenticate { .. })));
    assert!(matches!(seen.get(1), Some(Command::Capability)));
    assert!(matches!(
        seen.get(7),
        Some(Command::SetActive { name: None })
    ));
    assert!(matches!(seen.last(), Some(Command::Logout)));
}

#[tokio::test]
async fn wrong_password() {
    let (ours, theirs) = tokio::io::duplex(1 << 16);
    let _task = server(theirs, GREETING, vec![b"NO \"Authentication failed.\"\r\n"]);
    let client = Client::connect_test(ours, limits()).await.unwrap();
    let error = client
        .login("alice", &SecretString::from("wrong"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::AuthFailed(text) if text == "Authentication failed."));
}

#[tokio::test]
async fn challenges_are_cancelled() {
    // A server that wants more after PLAIN's initial response: we cancel
    // with "*" and it says NO.
    let (ours, mut theirs) = tokio::io::duplex(1 << 16);
    let task = tokio::spawn(async move {
        theirs.write_all(GREETING).await.unwrap();
        let mut decoder = CommandDecoder::new(FrameLimits::default());
        let mut buffer = [0_u8; 1024];
        let n = theirs.read(&mut buffer).await.unwrap();
        decoder.feed(&buffer[..n]);
        assert!(matches!(
            decoder.decode_command(),
            Ok(Some(Command::Authenticate { .. }))
        ));
        theirs.write_all(b"\"\"\r\n").await.unwrap();
        let n = theirs.read(&mut buffer).await.unwrap();
        decoder.feed(&buffer[..n]);
        assert_eq!(decoder.decode_sasl().unwrap(), Some(Command::SaslCancel));
        theirs.write_all(b"NO \"cancelled\"\r\n").await.unwrap();
    });
    let client = Client::connect_test(ours, limits()).await.unwrap();
    let error = client
        .login("alice", &SecretString::from("pw"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::AuthFailed(_)));
    task.await.unwrap();
}

/// RFC 7628 §3.2.3: OAUTHBEARER's error challenge is answered with a
/// single %x01 (base64 "AQ=="), not cancelled, so the server finishes.
#[tokio::test]
async fn oauthbearer_error_challenge_gets_x01() {
    const OAUTH_GREETING: &[u8] = b"\"IMPLEMENTATION\" \"Fake\"\r\n\"SASL\" \"OAUTHBEARER\"\r\n\
\"SIEVE\" \"fileinto\"\r\n\"VERSION\" \"1.0\"\r\nOK \"ready\"\r\n";
    let (ours, mut theirs) = tokio::io::duplex(1 << 16);
    let task = tokio::spawn(async move {
        theirs.write_all(OAUTH_GREETING).await.unwrap();
        let mut decoder = CommandDecoder::new(FrameLimits::default());
        let mut buffer = [0_u8; 1024];
        let n = theirs.read(&mut buffer).await.unwrap();
        decoder.feed(&buffer[..n]);
        assert!(matches!(
            decoder.decode_command(),
            Ok(Some(Command::Authenticate { .. }))
        ));
        theirs
            .write_all(b"\"eyJzdGF0dXMiOiJpbnZhbGlkX3Rva2VuIn0=\"\r\n")
            .await
            .unwrap();
        let n = theirs.read(&mut buffer).await.unwrap();
        assert_eq!(&buffer[..n], b"\"AQ==\"\r\n");
        theirs.write_all(b"NO \"invalid token\"\r\n").await.unwrap();
    });
    let client = Client::connect_test(ours, limits()).await.unwrap();
    let error = client
        .login_oauth(
            "alice",
            &SecretString::from("tok"),
            crate::OAuthMechanism::OAuthBearer,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::AuthFailed(_)), "{error:?}");
    task.await.unwrap();
}

#[tokio::test]
async fn no_plain_no_login() {
    let (ours, theirs) = tokio::io::duplex(1 << 16);
    let _task = server(
        theirs,
        b"\"SASL\" \"OAUTHBEARER\"\r\n\"VERSION\" \"1.0\"\r\nOK\r\n",
        vec![],
    );
    let client = Client::connect_test(ours, limits()).await.unwrap();
    let error = client
        .login("alice", &SecretString::from("pw"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NoAuthMechanism));
}

#[tokio::test]
async fn hostile_servers() {
    type Check = fn(&Error) -> bool;
    let cases: Vec<(&'static [u8], Check)> = vec![
        // BYE in the middle of a command.
        (
            b"\"a\"\r\nBYE \"shutting down\"\r\n",
            |e| matches!(e, Error::Bye(t) if t == "shutting down"),
        ),
        // A literal far bigger than the limit.
        (b"{999999999}\r\n", |e| matches!(e, Error::Decode(_))),
        // Garbage.
        (b"\x00\x01\x02\r\n", |e| matches!(e, Error::Decode(_))),
        // Two active scripts.
        (b"\"a\" ACTIVE\r\n\"b\" ACTIVE\r\nOK\r\n", |e| {
            matches!(e, Error::Protocol(_))
        }),
        // Closed mid-literal.
        (b"\"a\"\r\n{100}\r\nabc", |e| {
            matches!(e, Error::ConnectionClosed)
        }),
    ];
    for (reply, check) in cases {
        let (client, task) = logged_in(vec![reply]).await;
        let mut client = client.unwrap();
        let error = client.list_scripts().await.unwrap_err();
        assert!(check(&error), "{error:?}");
        assert!(error.is_fatal());
        // The connection is now unusable.
        assert!(client.is_broken());
        assert!(matches!(client.noop().await, Err(Error::Broken)));
        drop(client);
        let _ = task.await;
    }
}

#[tokio::test]
async fn silence_times_out() {
    let (ours, _theirs) = tokio::io::duplex(1 << 16);
    let limits = Limits {
        greeting_timeout: Duration::from_millis(200),
        ..limits()
    };
    let error = Client::connect_test(ours, limits).await.unwrap_err();
    assert!(matches!(error, Error::Timeout("greeting")));
}

#[tokio::test]
async fn too_many_lines() {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let mut all = vec![&b"OK\r\n"[..], GREETING];
    let many: &'static [u8] = Box::leak("\"x\"\r\n".repeat(20).into_bytes().into_boxed_slice());
    all.push(many);
    let _task = server(theirs, GREETING, all);
    let limits = Limits {
        max_data_lines: 10,
        ..limits()
    };
    let client = Client::connect_test(ours, limits).await.unwrap();
    let mut client = client.login("a", &SecretString::from("b")).await.unwrap();
    assert!(matches!(
        client.list_scripts().await,
        Err(Error::Protocol(_))
    ));
}

#[tokio::test]
async fn reply_too_large() {
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let mut all = vec![&b"OK\r\n"[..], GREETING];
    // 10 lines of 4000-byte literals: 40 KB, each line within the limits
    // and far fewer lines than `max_data_lines`.
    let line = format!("{{4000}}\r\n{}\r\n", "x".repeat(4000));
    let many: &'static [u8] = Box::leak(line.repeat(10).into_bytes().into_boxed_slice());
    all.push(many);
    let _task = server(theirs, GREETING, all);
    let limits = Limits {
        frame: FrameLimits {
            max_literal: 4096,
            max_line_literals: 4096,
            ..FrameLimits::default()
        },
        max_reply_overhead: 1024,
        ..limits()
    };
    assert_eq!(limits.max_reply_bytes(), 5120);
    let client = Client::connect_test(ours, limits).await.unwrap();
    let mut client = client.login("a", &SecretString::from("b")).await.unwrap();
    assert!(matches!(
        client.list_scripts().await,
        Err(Error::Protocol("response too large"))
    ));
    // The connection is broken after it.
    assert!(matches!(client.list_scripts().await, Err(Error::Broken)));
}

#[test]
fn before_tls_limits() {
    let before = Limits::default().before_tls();
    assert_eq!(before.frame.max_literal, 8 * 1024);
    assert_eq!(before.frame.max_line_literals, 8 * 1024);
    assert_eq!(before.frame.max_line, 8 * 1024);
    assert_eq!(before.max_reply_bytes(), 64 * 1024);
    assert_eq!(before.max_data_lines, 256);
    assert_eq!(before.read_timeout, Limits::default().read_timeout);
    // The usual limits leave room for a whole script.
    assert!(Limits::default().max_reply_bytes() > Limits::default().frame.max_line_literals);
}

#[tokio::test]
async fn bye_text_can_not_forge_log_lines() {
    let (ours, mut theirs) = tokio::io::duplex(1 << 16);
    let _task = tokio::spawn(async move {
        let _ = theirs
            .write_all(b"BYE {37}\r\nx\n2026-11-01T00:00:00Z  INFO login ok\r\n")
            .await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let error = Client::connect_test(ours, limits()).await.unwrap_err();
    let Error::Bye(text) = &error else {
        panic!("{error:?}");
    };
    assert!(!text.contains('\n'), "{text:?}");
    assert!(text.contains("INFO login ok"), "{text:?}");
    assert!(!error.to_string().contains('\n'));
}

#[test]
fn refusal_text_keeps_lines_but_not_in_its_display() {
    let refusal = crate::Refusal {
        command: "PUTSCRIPT",
        code: None,
        text: "line 1: error\nline 2: forged\u{2028}x".to_owned(),
    };
    assert!(refusal.text.contains('\n'));
    let shown = refusal.to_string();
    assert_eq!(shown, "PUTSCRIPT failed: line 1: error line 2: forged x");
    assert_eq!(Error::Refused(refusal).to_string(), shown);
}

#[tokio::test]
async fn old_servers_get_fallbacks() {
    // No VERSION: no CHECKSCRIPT or RENAMESCRIPT.
    const OLD: &[u8] = b"\"SASL\" \"PLAIN\"\r\n\"SIEVE\" \"fileinto\"\r\nOK\r\n";
    let (ours, theirs) = tokio::io::duplex(1 << 20);
    let task = server(
        theirs,
        OLD,
        vec![
            b"OK\r\n",
            OLD,
            // check_script: PUTSCRIPT scratch, then DELETESCRIPT.
            b"OK\r\n",
            b"OK\r\n",
            // rename_script: LISTSCRIPTS, GETSCRIPT, PUTSCRIPT, SETACTIVE, DELETESCRIPT.
            b"\"old\" ACTIVE\r\n\"other\"\r\nOK\r\n",
            b"{5}\r\nkeep;\r\nOK\r\n",
            b"OK\r\n",
            b"OK\r\n",
            b"OK\r\n",
            // A rename onto an existing name is refused before anything is written.
            b"\"old2\"\r\n\"other\"\r\nOK\r\n",
        ],
    );
    let client = Client::connect_test(ours, limits()).await.unwrap();
    let mut client = client.login("a", &SecretString::from("b")).await.unwrap();
    assert!(matches!(
        client.check_script(b"keep;").await.unwrap(),
        CheckOutcome::Valid { warnings: None }
    ));
    client
        .rename_script(&name("old"), &name("new"))
        .await
        .unwrap();
    let error = client
        .rename_script(&name("old2"), &name("other"))
        .await
        .unwrap_err();
    assert!(matches!(
        error.code(),
        Some(nebula_managesieve_proto::ResponseCode::AlreadyExists)
    ));
    drop(client);
    let seen = task.await.unwrap();
    let names: Vec<&str> = seen.iter().map(Command::name).collect();
    assert_eq!(
        names,
        [
            "AUTHENTICATE",
            "CAPABILITY",
            "PUTSCRIPT",
            "DELETESCRIPT",
            "LISTSCRIPTS",
            "GETSCRIPT",
            "PUTSCRIPT",
            "SETACTIVE",
            "DELETESCRIPT",
            "LISTSCRIPTS",
        ]
    );
    assert!(matches!(
        seen.get(2),
        Some(Command::PutScript { name, .. }) if name.as_str().starts_with("nebula-mail-check-")
    ));
}
