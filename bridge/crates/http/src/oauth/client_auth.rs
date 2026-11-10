// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `client_secret_basic` (RFC 6749 §2.3.1): `Authorization: Basic`
//! with the form-encoded client id and secret.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use data_encoding::BASE64;
use http::{HeaderMap, header};
use nebula_crypto::ClientSecretHash;
use nebula_store::UnixTime;
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

use crate::config::{ClientId, HttpConfig, OAuthClient};
use crate::form::decode_component;
use crate::limits::ClientAuthLimiter;

/// argon2id takes ~20 MiB and some milliseconds per check: a few at a time
/// is enough for real clients and caps what bad requests can cost.
const PARALLEL_HASHES: usize = 2;
/// How long a check may wait for a free hash before answering "busy", and
/// how many may wait at once. Short and bounded: no queue for an attacker
/// to fill in front of the BFF.
const HASH_WAIT: Duration = Duration::from_secs(1);
const MAX_WAITING: usize = 8;

/// Why client authentication failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientAuthError {
    /// Missing or wrong credentials (`invalid_client`).
    Invalid,
    /// Every hash is busy: try again soon. Not counted as a failure.
    Busy,
    /// Too many failed checks from this address. Answered like `Busy`
    /// (503), never with a 4xx: the BFF ends the session on a 4xx, and it
    /// may share the address with whoever filled the window.
    Limited,
}

/// The secret check (argon2); tests put a slow one in.
type Verify = dyn Fn(&ClientSecretHash, &SecretString) -> bool + Send + Sync;

/// Which secret a check is for: the client and the SHA-256 of the secret.
type FlightKey = (ClientId, [u8; 32]);

/// Checks client credentials. A secret that passed argon2 once is
/// remembered as its SHA-256 (in memory only), so the BFF's refreshes
/// don't pay argon2 every time. Before that (after a restart):
///
/// - at most [`PARALLEL_HASHES`] argon2 at once, each keeping its permit
///   until the hash ends, even if the request is cancelled;
/// - requests with the same secret share one hash (single flight);
/// - a short bounded wait for a free hash, then [`ClientAuthError::Busy`];
/// - failed checks are limited per address before argon2 runs.
pub(crate) struct ClientAuth {
    inner: Arc<Inner>,
}

struct Inner {
    verified: Mutex<HashMap<ClientId, [u8; 32]>>,
    /// Checks in progress; the result is sent when the hash ends.
    in_flight: Mutex<HashMap<FlightKey, watch::Receiver<Option<bool>>>>,
    hashing: Arc<Semaphore>,
    waiting: AtomicUsize,
    hash_wait: Duration,
    limiter: ClientAuthLimiter,
    verify: Box<Verify>,
    /// How many hashes ran (tests).
    hashes: AtomicUsize,
}

impl std::fmt::Debug for ClientAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientAuth").finish_non_exhaustive()
    }
}

impl Default for ClientAuth {
    fn default() -> Self {
        Self::with_verifier(
            HASH_WAIT,
            Box::new(|hash, secret| hash.verify(secret).is_ok()),
        )
    }
}

/// Takes a check's key out of [`Inner::in_flight`] when dropped: when the
/// hash ends, or if its leader gives up before starting it.
struct FlightGuard {
    inner: Arc<Inner>,
    key: FlightKey,
}

impl Drop for FlightGuard {
    fn drop(&mut self) {
        self.inner
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// One of the [`MAX_WAITING`] places to wait for a hash.
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The client id and secret of a `Basic` header.
pub(crate) fn parse_basic(headers: &HeaderMap) -> Option<(String, SecretString)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, encoded) = value.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = BASE64.decode(encoded.trim().as_bytes()).ok()?;
    let text = SecretString::from(String::from_utf8(decoded).ok()?);
    let (id, secret) = text.expose_secret().split_once(':')?;
    let id = decode_component(id)?;
    let secret = SecretString::from(decode_component(secret)?);
    Some((id, secret))
}

impl ClientAuth {
    fn with_verifier(hash_wait: Duration, verify: Box<Verify>) -> Self {
        Self {
            inner: Arc::new(Inner {
                verified: Mutex::new(HashMap::new()),
                in_flight: Mutex::new(HashMap::new()),
                hashing: Arc::new(Semaphore::new(PARALLEL_HASHES)),
                waiting: AtomicUsize::new(0),
                hash_wait,
                limiter: ClientAuthLimiter::new(),
                verify,
                hashes: AtomicUsize::new(0),
            }),
        }
    }

    /// The authenticated client. `ip` is the client's address, for the
    /// limit on failed checks.
    pub(crate) async fn authenticate<'c>(
        &self,
        config: &'c HttpConfig,
        headers: &HeaderMap,
        ip: IpAddr,
        now: UnixTime,
    ) -> Result<&'c OAuthClient, ClientAuthError> {
        let (id, secret) = parse_basic(headers).ok_or(ClientAuthError::Invalid)?;
        let client = config.client(&id).ok_or(ClientAuthError::Invalid)?;
        let digest: [u8; 32] = Sha256::digest(secret.expose_secret().as_bytes()).into();
        if self.is_remembered(client.id(), &digest) {
            return Ok(client);
        }
        // Counted when it starts, like logins, so parallel tries can't slip
        // through before the first failure is known.
        if self.inner.limiter.begin(ip, now).is_err() {
            tracing::warn!("OAuth client authentication refused: too many failures");
            return Err(ClientAuthError::Limited);
        }
        match self.check(client, secret, digest).await {
            Some(true) => {
                self.inner.limiter.succeeded(ip);
                Ok(client)
            }
            Some(false) => Err(ClientAuthError::Invalid),
            // No hash ran: nothing to count.
            None => {
                self.inner.limiter.not_counted(ip);
                Err(ClientAuthError::Busy)
            }
        }
    }

    /// Whether the secret matches, sharing a hash already running for the
    /// same secret; `None` if no hash was free.
    async fn check(
        &self,
        client: &OAuthClient,
        secret: SecretString,
        digest: [u8; 32],
    ) -> Option<bool> {
        let key = (client.id().clone(), digest);
        let (mut result, sender) = {
            let mut flights = self
                .inner
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(running) = flights.get(&key) {
                (running.clone(), None)
            } else {
                let (sender, receiver) = watch::channel(None);
                flights.insert(key.clone(), receiver.clone());
                (receiver, Some(sender))
            }
        };
        if let Some(sender) = sender {
            // This request leads: the guard clears the flight however it
            // ends, and the followers see the sender go.
            let guard = FlightGuard {
                inner: Arc::clone(&self.inner),
                key,
            };
            let permit = self.permit().await?;
            let hash = client.secret_hash().clone();
            let id = client.id().clone();
            let inner = Arc::clone(&self.inner);
            // Everything the hash needs moves into the blocking task, the
            // permit too: it is released when argon2 ends, not when the
            // request goes away.
            drop(tokio::task::spawn_blocking(move || {
                let _permit = permit;
                inner.hashes.fetch_add(1, Ordering::SeqCst);
                let verified = (inner.verify)(&hash, &secret);
                if verified {
                    inner
                        .verified
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(id, digest);
                }
                // Out of the map first: a request that starts after this
                // one ended runs its own check.
                drop(guard);
                let _ = sender.send(Some(verified));
            }));
        }
        result
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|verified| *verified)
    }

    /// A free hash, waiting a little for one; `None` if none came.
    async fn permit(&self) -> Option<OwnedSemaphorePermit> {
        if let Ok(permit) = Arc::clone(&self.inner.hashing).try_acquire_owned() {
            return Some(permit);
        }
        let waiting = Waiting(&self.inner.waiting);
        if waiting.0.fetch_add(1, Ordering::SeqCst) >= MAX_WAITING {
            return None;
        }
        tokio::time::timeout(
            self.inner.hash_wait,
            Arc::clone(&self.inner.hashing).acquire_owned(),
        )
        .await
        .ok()?
        .ok()
    }

    fn is_remembered(&self, id: &ClientId, digest: &[u8; 32]) -> bool {
        self.inner
            .verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .is_some_and(|known| bool::from(known.ct_eq(digest)))
    }

    /// Forgets old failures.
    pub(crate) fn prune(&self, now: UnixTime) {
        self.inner.limiter.prune(now);
    }

    /// How many argon2 checks ran.
    #[cfg(test)]
    pub(crate) fn hashes_run(&self) -> usize {
        self.inner.hashes.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn parses_form_encoded_parts() {
        // What the web BFF sends for "nebulamail" / "s3cret value".
        let value = format!("Basic {}", BASE64.encode(b"nebulamail:s3cret+value"));
        let (id, secret) = parse_basic(&headers(&value)).unwrap();
        assert_eq!(id, "nebulamail");
        assert_eq!(secret.expose_secret(), "s3cret value");

        let colon = format!("basic {}", BASE64.encode(b"web:a%3Ab:c"));
        let (_, secret) = parse_basic(&headers(&colon)).unwrap();
        assert_eq!(secret.expose_secret(), "a:b:c");
    }

    const IP: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));

    fn basic(secret: &str) -> HeaderMap {
        headers(&crate::tests::basic(crate::tests::CLIENT_ID, secret))
    }

    /// A verifier that blocks until the test lets it go, and tells when
    /// each check starts.
    struct Gate {
        started: tokio::sync::mpsc::UnboundedReceiver<()>,
        open: std::sync::mpsc::Sender<bool>,
        calls: Arc<AtomicUsize>,
    }

    fn gated(hash_wait: Duration) -> (Arc<ClientAuth>, Gate) {
        let (started_tx, started) = tokio::sync::mpsc::unbounded_channel();
        let (open, open_rx) = std::sync::mpsc::channel::<bool>();
        let open_rx = Mutex::new(open_rx);
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let auth = ClientAuth::with_verifier(
            hash_wait,
            Box::new(move |_, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = started_tx.send(());
                open_rx.lock().unwrap().recv().unwrap_or(false)
            }),
        );
        (
            Arc::new(auth),
            Gate {
                started,
                open,
                calls,
            },
        )
    }

    fn spawn_check(
        auth: &Arc<ClientAuth>,
        secret: &str,
    ) -> tokio::task::JoinHandle<Result<(), ClientAuthError>> {
        let auth = Arc::clone(auth);
        let headers = basic(secret);
        tokio::spawn(async move {
            let config = crate::tests::config();
            auth.authenticate(&config, &headers, IP, UnixTime::from_secs(1000))
                .await
                .map(|_| ())
        })
    }

    async fn wait_for_permits(auth: &ClientAuth, permits: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while auth.inner.hashing.available_permits() != permits {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_check_keeps_its_permit() {
        let (auth, mut gate) = gated(HASH_WAIT);
        let task = spawn_check(&auth, "guess");
        gate.started.recv().await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        // The request is gone, argon2 is not: its permit is still taken.
        assert_eq!(auth.inner.hashing.available_permits(), 1);
        gate.open.send(false).unwrap();
        wait_for_permits(&auth, 2).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn busy_when_both_hashes_run_and_busy_does_not_count() {
        let (auth, mut gate) = gated(Duration::from_millis(20));
        let first = spawn_check(&auth, "one");
        let second = spawn_check(&auth, "two");
        gate.started.recv().await.unwrap();
        gate.started.recv().await.unwrap();
        // Many more than the failure limit: all busy, none counted.
        for n in 0..15 {
            let busy = spawn_check(&auth, &format!("other {n}")).await.unwrap();
            assert_eq!(busy, Err(ClientAuthError::Busy));
        }
        assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
        gate.open.send(false).unwrap();
        gate.open.send(false).unwrap();
        assert_eq!(first.await.unwrap(), Err(ClientAuthError::Invalid));
        assert_eq!(second.await.unwrap(), Err(ClientAuthError::Invalid));
        // Two real failures counted: eight more are allowed, then limited.
        for _ in 0..8 {
            gate.open.send(false).unwrap();
        }
        for n in 0..8 {
            let failed = spawn_check(&auth, &format!("guess {n}")).await.unwrap();
            assert_eq!(failed, Err(ClientAuthError::Invalid));
        }
        let limited = spawn_check(&auth, "more").await.unwrap();
        assert_eq!(limited, Err(ClientAuthError::Limited));
        assert_eq!(gate.calls.load(Ordering::SeqCst), 10);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn one_hash_for_the_same_secret() {
        let (auth, mut gate) = gated(HASH_WAIT);
        let checks: Vec<_> = (0..5)
            .map(|_| spawn_check(&auth, crate::tests::CLIENT_SECRET))
            .collect();
        gate.started.recv().await.unwrap();
        // Let the others join the running hash.
        tokio::time::sleep(Duration::from_millis(50)).await;
        gate.open.send(true).unwrap();
        for check in checks {
            assert_eq!(check.await.unwrap(), Ok(()));
        }
        assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
        // Remembered now: no more hashes.
        assert_eq!(
            spawn_check(&auth, crate::tests::CLIENT_SECRET)
                .await
                .unwrap(),
            Ok(())
        );
        assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_other_schemes_and_garbage() {
        for value in [
            "Bearer abc",
            "Basic !!!",
            "Basic",
            &format!("Basic {}", BASE64.encode(b"nocolon")),
        ] {
            assert!(parse_basic(&headers(value)).is_none(), "{value}");
        }
        assert!(parse_basic(&HeaderMap::new()).is_none());
    }
}
