// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The mail servers of `docker/compose.yaml` that the integration tests of
//! every crate run against.
//!
//! The tests are skipped unless asked for: `NEBULA_TEST_IMAP=1` for IMAP
//! (and everything above it), `NEBULA_TEST_SMTP=1` for submission.
//! `NEBULA_TEST_SERVER` picks the server, `stalwart` (default),
//! `dovecot`, `postfix` (Dovecot's IMAP with Postfix's submission, which
//! has DSN) or `dovecot-prefix` (folders under `INBOX.`, its own Postfix):
//!
//! ```sh
//! docker compose -f docker/compose.yaml --profile bridge up -d stalwart dovecot
//! NEBULA_TEST_IMAP=1 NEBULA_TEST_SERVER=dovecot cargo test --test server
//! ```
//!
//! `NEBULA_TEST_IMAP_PORT` replaces the IMAP port (implicit TLS) of the
//! selected server, for the same server behind a proxy: with
//! `apps/web/e2e-bridge/rev2-proxy.ts` on 2997 in front of Dovecot,
//! `NEBULA_TEST_SERVER=dovecot NEBULA_TEST_IMAP_PORT=2997` tests against
//! an IMAP4rev2-only server (RFC 9051).
//!
//! Passwords come from the environment (`TEST_ALICE_PASSWORD`,
//! `TEST_BOB_PASSWORD`, `TEST_DOVECOT_PASSWORD`) or else from the env file
//! at `NEBULA_TEST_ENV_FILE` (default: `docker/.env` of the repository).
//! Dovecot accepts any user with one shared password, `password` unless
//! `TEST_DOVECOT_PASSWORD` says otherwise (as in `docker/compose.yaml`).
//!
//! ManageSieve tests need `NEBULA_TEST_SIEVE=1`, with the
//! same `NEBULA_TEST_SERVER`.
//!
//! CalDAV/CardDAV tests need `NEBULA_TEST_DAV=1`, with
//! `NEBULA_TEST_SERVER` = `stalwart` (default; its DAV on the HTTP port),
//! `radicale` or `nextcloud` (same users and password as Dovecot; Nextcloud
//! is big and never started by CI): [`DavTestServer`].

#![forbid(unsafe_code)]
// Only tests use this crate: a missing password or a bad setting is a
// failed test, so panicking with a clear message is what we want.
#![allow(
    clippy::panic,
    clippy::expect_used,
    reason = "test configuration; a panic is a failed test"
)]

use std::fmt;

use secrecy::SecretString;

/// Host of every test server: the published ports listen on loopback only.
pub const HOST: &str = "127.0.0.1";

/// Domain of the test addresses (`alice@nebulamail.test`).
pub const DOMAIN: &str = "nebulamail.test";

/// Which server of `docker/compose.yaml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKind {
    /// Stalwart, the main target (JMAP too).
    Stalwart,
    /// Dovecot 2.4: a plain IMAP server without `OBJECTID`, whose
    /// submission relays to Stalwart.
    Dovecot,
    /// Dovecot for IMAP and Postfix 3.10 for submission, which announces
    /// DSN and delivers to Dovecot's `alice` and `bob` itself, so its
    /// success reports say `delivered` (`docker/postfix`). Mail to
    /// [`BOUNCE_DOMAIN`] bounces with a real DSN.
    Postfix,
    /// A Dovecot of its own with folders under `INBOX.`, `.` between
    /// levels, and its own Postfix.
    DovecotPrefix,
}

/// Mail to this domain is accepted by the Postfix test server and then
/// bounced back to the sender with a DSN (RFC 3464).
pub const BOUNCE_DOMAIN: &str = "bounce.nebulamail.test";

impl fmt::Display for ServerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stalwart => "stalwart",
            Self::Dovecot => "dovecot",
            Self::Postfix => "postfix",
            Self::DovecotPrefix => "dovecot-prefix",
        })
    }
}

/// How a connection gets TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsKind {
    /// TLS from the first byte (993, 465).
    Implicit,
    /// Plain greeting, then STARTTLS (143, 587).
    StartTls,
}

/// A port of a test server and how it does TLS. Every test server uses a
/// self-signed certificate, so tests check it with the "insecure, loopback
/// only" mode of each crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    /// Port on [`HOST`].
    pub port: u16,
    /// TLS mode of the port.
    pub tls: TlsKind,
}

impl Endpoint {
    /// `127.0.0.1:port`, for `TcpStream::connect`.
    #[must_use]
    pub fn address(self) -> String {
        format!("{HOST}:{}", self.port)
    }
}

/// A test user. Both servers know `alice` and `bob`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum User {
    /// The user most tests log in as.
    Alice,
    /// The other one, who receives.
    Bob,
}

impl User {
    /// The IMAP and SMTP login.
    #[must_use]
    pub fn login(self) -> &'static str {
        match self {
            Self::Alice => "alice",
            Self::Bob => "bob",
        }
    }

    /// The mail address.
    #[must_use]
    pub fn address(self) -> String {
        format!("{}@{DOMAIN}", self.login())
    }
}

/// The selected test server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestServer {
    /// Which one.
    pub kind: ServerKind,
    /// IMAP port.
    pub imap: Endpoint,
    /// IMAP with STARTTLS, if the server has it (Stalwart 0.16 listens on
    /// 993 only), to test that path too.
    pub imap_starttls: Option<Endpoint>,
    /// Submission port.
    pub smtp: Endpoint,
    /// Submission with STARTTLS, if the server has it (Stalwart 0.16 has
    /// no 587 listener).
    pub smtp_starttls: Option<Endpoint>,
    /// ManageSieve (STARTTLS on both): Stalwart's 4190, Dovecot's 24190.
    pub sieve: Endpoint,
}

impl TestServer {
    /// The server for IMAP tests, or `None` when they are not enabled
    /// (`NEBULA_TEST_IMAP` is not `1`).
    ///
    /// # Panics
    /// If `NEBULA_TEST_SERVER` names an unknown server.
    #[must_use]
    pub fn imap() -> Option<Self> {
        enabled("NEBULA_TEST_IMAP").then(Self::selected)
    }

    /// The server for ManageSieve tests, or `None` when they are not
    /// enabled (`NEBULA_TEST_SIEVE` is not `1`).
    ///
    /// # Panics
    /// If `NEBULA_TEST_SERVER` names an unknown server.
    #[must_use]
    pub fn sieve() -> Option<Self> {
        enabled("NEBULA_TEST_SIEVE").then(Self::selected)
    }

    /// The server for SMTP tests, or `None` when they are not enabled
    /// (`NEBULA_TEST_SMTP` is not `1`).
    ///
    /// # Panics
    /// If `NEBULA_TEST_SERVER` names an unknown server.
    #[must_use]
    pub fn smtp() -> Option<Self> {
        enabled("NEBULA_TEST_SMTP").then(Self::selected)
    }

    fn selected() -> Self {
        let kind = match std::env::var("NEBULA_TEST_SERVER").as_deref() {
            Err(_) | Ok("" | "stalwart") => ServerKind::Stalwart,
            Ok("dovecot") => ServerKind::Dovecot,
            Ok("postfix") => ServerKind::Postfix,
            Ok("dovecot-prefix") => ServerKind::DovecotPrefix,
            Ok(other) => panic!(
                "NEBULA_TEST_SERVER={other}: use stalwart, dovecot, postfix or dovecot-prefix"
            ),
        };
        let server = Self::of(kind);
        // The same server behind a proxy (apps/web/e2e-bridge/rev2-proxy.ts
        // makes Dovecot an IMAP4rev2-only server): implicit TLS on that
        // port, and no STARTTLS port, which would skip the proxy.
        match setting("NEBULA_TEST_IMAP_PORT").map(|port| port.parse::<u16>()) {
            None => server,
            Some(Ok(port)) => Self {
                imap: Endpoint {
                    port,
                    tls: TlsKind::Implicit,
                },
                imap_starttls: None,
                ..server
            },
            Some(Err(_)) => panic!("NEBULA_TEST_IMAP_PORT is not a port"),
        }
    }

    /// The ports `docker/compose.yaml` publishes for `kind`.
    #[must_use]
    pub fn of(kind: ServerKind) -> Self {
        let endpoint = |port, tls| Endpoint { port, tls };
        match kind {
            ServerKind::Stalwart => Self {
                kind,
                imap: endpoint(1993, TlsKind::Implicit),
                imap_starttls: None,
                smtp: endpoint(1465, TlsKind::Implicit),
                smtp_starttls: None,
                sieve: endpoint(4190, TlsKind::StartTls),
            },
            ServerKind::Dovecot => Self {
                kind,
                imap: endpoint(2993, TlsKind::Implicit),
                imap_starttls: Some(endpoint(2143, TlsKind::StartTls)),
                smtp: endpoint(2465, TlsKind::Implicit),
                smtp_starttls: Some(endpoint(2587, TlsKind::StartTls)),
                sieve: endpoint(24190, TlsKind::StartTls),
            },
            ServerKind::Postfix => Self {
                kind,
                smtp: endpoint(2466, TlsKind::Implicit),
                smtp_starttls: Some(endpoint(2588, TlsKind::StartTls)),
                ..Self::of(ServerKind::Dovecot)
            },
            ServerKind::DovecotPrefix => Self {
                kind,
                imap: endpoint(2994, TlsKind::Implicit),
                imap_starttls: None,
                smtp: endpoint(2467, TlsKind::Implicit),
                smtp_starttls: None,
                sieve: endpoint(24191, TlsKind::StartTls),
            },
        }
    }

    /// A submission port of this server that doesn't announce 8BITMIME
    /// (Postfix's `2469`, same login), for the bridge's 7-bit downgrade
    /// with a real server; `None` elsewhere.
    #[must_use]
    pub fn smtp_without_8bitmime(self) -> Option<Endpoint> {
        (self.kind == ServerKind::Postfix).then_some(Endpoint {
            port: 2469,
            tls: TlsKind::Implicit,
        })
    }

    /// The password of `user` on this server.
    ///
    /// # Panics
    /// If Stalwart's password is neither in the environment nor in the env
    /// file.
    #[must_use]
    pub fn password(self, user: User) -> SecretString {
        match self.kind {
            ServerKind::Stalwart => {
                let name = format!("TEST_{}_PASSWORD", user.login().to_uppercase());
                setting(&name).unwrap_or_else(|| panic!("{name} is not set (see nebula-tests)"))
            }
            // Postfix has the same users and password as Dovecot.
            ServerKind::Dovecot | ServerKind::Postfix | ServerKind::DovecotPrefix => {
                setting("TEST_DOVECOT_PASSWORD").unwrap_or_else(|| "password".to_owned())
            }
        }
        .into()
    }

    /// Where mail submitted here for a test user lands. Dovecot relays to
    /// Stalwart, so bob gets it there, not in Dovecot; Postfix delivers to
    /// Dovecot, this same IMAP server.
    #[must_use]
    pub fn delivery(self) -> Self {
        match self.kind {
            ServerKind::Postfix | ServerKind::DovecotPrefix => self,
            ServerKind::Stalwart | ServerKind::Dovecot => Self::of(ServerKind::Stalwart),
        }
    }
}

/// A CalDAV/CardDAV server of `docker/compose.yaml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavKind {
    /// Stalwart's DAV, on its HTTP port (plain HTTP on loopback).
    Stalwart,
    /// Radicale 3 (`docker/radicale`): no scheduling, plain HTTP.
    Radicale,
    /// Nextcloud (`docker/nextcloud`): sabre/dav with Nextcloud's own
    /// sharing (`oc:share`) and scheduling, plain HTTP.
    Nextcloud,
}

impl fmt::Display for DavKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stalwart => "stalwart",
            Self::Radicale => "radicale",
            Self::Nextcloud => "nextcloud",
        })
    }
}

/// The DAV server of the running tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavTestServer {
    /// Which one.
    pub kind: DavKind,
}

impl DavTestServer {
    /// The server for DAV tests, or `None` when they are not enabled
    /// (`NEBULA_TEST_DAV` is not `1`).
    ///
    /// # Panics
    /// If `NEBULA_TEST_SERVER` names a server without DAV.
    #[must_use]
    pub fn selected() -> Option<Self> {
        if !enabled("NEBULA_TEST_DAV") {
            return None;
        }
        let kind = match std::env::var("NEBULA_TEST_SERVER").as_deref() {
            Err(_) | Ok("" | "stalwart") => DavKind::Stalwart,
            Ok("radicale") => DavKind::Radicale,
            Ok("nextcloud") => DavKind::Nextcloud,
            Ok(other) => {
                panic!("NEBULA_TEST_SERVER={other}: DAV tests use stalwart, radicale or nextcloud")
            }
        };
        Some(Self { kind })
    }

    /// The URL discovery starts from (what an admin would configure).
    #[must_use]
    pub fn base_url(self) -> String {
        // The same server behind something else (a proxy that turns its
        // etags weak, say), with the same users.
        if let Some(url) = setting("NEBULA_TEST_DAV_URL") {
            return url;
        }
        match self.kind {
            DavKind::Stalwart => format!("http://{HOST}:8080/"),
            DavKind::Radicale => format!("http://{HOST}:5232/"),
            DavKind::Nextcloud => format!("http://{HOST}:5234/"),
        }
    }

    /// The login of `user` (the same as for IMAP).
    #[must_use]
    pub fn login(self, user: User) -> &'static str {
        user.login()
    }

    /// The password of `user`.
    ///
    /// # Panics
    /// If Stalwart's password is neither in the environment nor in the env
    /// file.
    #[must_use]
    pub fn password(self, user: User) -> SecretString {
        self.mail().password(user)
    }

    /// The mail server with the same users and passwords, for tests of
    /// the whole bridge (IMAP + DAV): Stalwart for its own DAV, Dovecot
    /// for Radicale and Nextcloud (docker/compose.yaml).
    #[must_use]
    pub fn mail(self) -> TestServer {
        TestServer::of(match self.kind {
            DavKind::Stalwart => ServerKind::Stalwart,
            DavKind::Radicale | DavKind::Nextcloud => ServerKind::Dovecot,
        })
    }
}

fn enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1")
}

/// `name` from the environment, or else from the env file.
fn setting(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        return Some(value);
    }
    let path = std::env::var("NEBULA_TEST_ENV_FILE")
        .unwrap_or_else(|_| format!("{}/../../docker/.env", env!("CARGO_MANIFEST_DIR")));
    let contents = std::fs::read_to_string(path).ok()?;
    contents.lines().find_map(|line| {
        let value = line.strip_prefix(name)?.strip_prefix('=')?;
        Some(value.trim().trim_matches('"').to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_use_the_test_domain() {
        assert_eq!(User::Alice.address(), "alice@nebulamail.test");
        assert_eq!(User::Bob.login(), "bob");
    }

    #[test]
    fn each_server_has_its_own_ports() {
        let stalwart = TestServer::of(ServerKind::Stalwart);
        let dovecot = TestServer::of(ServerKind::Dovecot);
        assert_eq!(stalwart.imap.address(), "127.0.0.1:1993");
        assert_eq!(dovecot.imap.address(), "127.0.0.1:2993");
        assert_ne!(stalwart.smtp, dovecot.smtp);
        assert_eq!(dovecot.delivery(), stalwart);
        let postfix = TestServer::of(ServerKind::Postfix);
        assert_eq!(postfix.imap, dovecot.imap);
        assert_eq!(postfix.smtp.address(), "127.0.0.1:2466");
        assert_eq!(postfix.delivery(), postfix);
    }
}
