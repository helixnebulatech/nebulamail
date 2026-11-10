// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Web Push from the bridge (RFC 8620 §7.2.7 extension of
//! 2026-10-28): `PushSubscription/get|set` and the delivery of
//! `PushVerification`, `StateChange` and `CalendarAlert` objects to the
//! browsers' push services, as Stalwart does, so the web app's
//! notifications with every tab closed work through the bridge.
//!
//! - **Methods** ([`methods`]): a new subscription is checked (endpoint
//!   rules of `nebula-webpush`: by default only the browsers' push
//!   services, https, public addresses; the browser keys; at most
//!   [`WebPushSettings::max_subscriptions`] per login session; at most 7
//!   days, and never past the end of the session),
//!   stored sealed with the account's data key and sent a
//!   `PushVerification`; nothing else goes to it until the client sends the
//!   code back.
//! - **Delivery** ([`task`]): one task per running account listens to the
//!   mail and groupware events, keeps per subscription the newest states
//!   (merged: one `StateChange` carries them all) and the due alerts, and
//!   POSTs them encrypted and signed, at most one per
//!   [`WebPushSettings::min_interval`] per subscription and within the
//!   account's budget ([`WebPushSettings::burst`], one more every
//!   [`WebPushSettings::refill`]). Temporary failures are retried after
//!   [`WebPushSettings::retries`] (DNS trouble is one of them); a
//!   404/410, a refusal, an address `net-guard` blocks or a name that is
//!   never public deletes the subscription.
//!
//! Each subscription belongs to the login session (OAuth grant) that made
//! it (RFC 8620 §7.2): only that session sees and changes it, and it ends
//! with it (revoking the session deletes it; `expires` is never past the
//! session's end). Creating one replaces the subscriptions of the same
//! device (`deviceClientId`) made by the same client in another session
//! of the account (RFC 8620 §7.2), so a browser that logs in again does
//! not get every notice twice. A restart starts the accounts that have one
//! (so new mail is seen with every tab closed).

pub(crate) mod email_push;
pub(crate) mod methods;
mod task;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use nebula_crypto::{AccountKey, Binding, Field, SealedField};
use nebula_http::AccountContext;
use nebula_jmap_proto::PushSubscriptionId as JmapPushId;
use nebula_store::{AccountId, PushSubscriptionId, StoreError};
use nebula_webpush::{ClientKeys, EndpointPolicy, PushClient};
use tokio::sync::{mpsc, watch};
use zeroize::Zeroizing;

use crate::core::backend::Inner;
use crate::core::ids::parse_number;
use crate::error::Bug;

/// Settings of Web Push, built by the binary from `[push]`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WebPushSettings {
    /// Sends the messages (VAPID key, contact, resolver).
    pub client: PushClient,
    /// Which endpoints are accepted.
    pub policy: EndpointPolicy,
    /// Subscriptions per login session, at most: 10 (the web app has one per
    /// device).
    pub max_subscriptions: usize,
    /// Longest (and default) life of a subscription: 7 days, as Stalwart.
    pub lifetime: Duration,
    /// Least time between two messages to one subscription: 2 s (state
    /// changes in between are merged).
    pub min_interval: Duration,
    /// Messages an account may send at once: 30.
    pub burst: u32,
    /// One more message allowed every: 10 s (360 an hour, sustained).
    pub refill: Duration,
    /// Waits before each new try of a message that failed for a temporary
    /// reason; after the last one the message is dropped.
    pub retries: Vec<Duration>,
    /// Calendar alerts go by Web Push too (`[push] calendar_alerts`):
    /// `true`. `false`: only to an open event source.
    pub calendar_alerts: bool,
    /// How long a push service keeps an alert for an offline browser: 10
    /// min, the alert grace (a reminder later than that is noise).
    pub alert_ttl: Duration,
}

impl WebPushSettings {
    /// The defaults with this client and policy.
    #[must_use]
    pub fn new(client: PushClient, policy: EndpointPolicy) -> Self {
        Self {
            client,
            policy,
            max_subscriptions: 10,
            lifetime: Duration::from_hours(7 * 24),
            min_interval: Duration::from_secs(2),
            burst: 30,
            refill: Duration::from_secs(10),
            retries: vec![
                Duration::from_secs(10),
                Duration::from_mins(1),
                Duration::from_mins(5),
                Duration::from_mins(30),
            ],
            calendar_alerts: true,
            alert_ttl: Duration::from_mins(10),
        }
    }
}

/// What the methods tell an account's delivery task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    /// Subscriptions changed: read them again (a new one, not verified,
    /// gets its `PushVerification`).
    Reload,
}

struct Running {
    key: Arc<AccountKey>,
    commands: mpsc::Sender<Command>,
    stop: watch::Sender<bool>,
}

/// Web Push of the running accounts.
pub(crate) struct Service {
    settings: Arc<WebPushSettings>,
    accounts: Mutex<HashMap<AccountId, Running>>,
}

impl Service {
    pub(crate) fn new(settings: WebPushSettings) -> Self {
        Self {
            settings: Arc::new(settings),
            accounts: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn settings(&self) -> &WebPushSettings {
        &self.settings
    }

    /// The session's `applicationServerKey`.
    pub(crate) fn application_server_key(&self) -> String {
        self.settings.client.vapid().application_server_key()
    }

    fn accounts(&self) -> std::sync::MutexGuard<'_, HashMap<AccountId, Running>> {
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the delivery task of an account (idempotent).
    pub(crate) fn start(&self, inner: Weak<Inner>, account: &AccountContext, key: AccountKey) {
        let Some(strong) = inner.upgrade() else {
            return;
        };
        let mut accounts = self.accounts();
        if accounts
            .get(&account.id)
            .is_some_and(|running| !running.commands.is_closed())
        {
            return;
        }
        let key = Arc::new(key);
        let (commands, receiver) = mpsc::channel(64);
        let (stop, stopped) = watch::channel(false);
        let listen = task::Listen {
            mail: strong.mail.subscribe(),
            groupware: strong
                .groupware
                .as_ref()
                .map(nebula_groupware::Groupware::subscribe),
        };
        drop(strong);
        tokio::spawn(task::run(task::Task {
            inner,
            settings: Arc::clone(&self.settings),
            account: account.id,
            jmap_id: account.jmap_id.clone(),
            key: Arc::clone(&key),
            commands: receiver,
            stopped,
            listen,
        }));
        accounts.insert(
            account.id,
            Running {
                key,
                commands,
                stop,
            },
        );
    }

    /// Stops the account's task (its last session ended).
    pub(crate) fn stop(&self, account: AccountId) {
        if let Some(running) = self.accounts().remove(&account) {
            let _ = running.stop.send(true);
        }
    }

    /// The data key of a running account.
    pub(crate) fn key(&self, account: AccountId) -> Option<Arc<AccountKey>> {
        self.accounts().get(&account).map(|r| Arc::clone(&r.key))
    }

    /// Tells the account's task (if it runs; a full queue means a reload
    /// is pending anyway).
    pub(crate) fn tell(&self, account: AccountId, command: Command) {
        if let Some(running) = self.accounts().get(&account) {
            let _ = running.commands.try_send(command);
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let accounts = self
            .accounts
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for (_, running) in accounts.drain() {
            let _ = running.stop.send(true);
        }
    }
}

/// The JMAP id of a subscription: `w` and the row number.
pub(crate) fn jmap_id(id: PushSubscriptionId) -> Result<JmapPushId, Bug> {
    format!("w{}", id.get())
        .parse()
        .map_err(|_| Bug("push subscription id"))
}

/// The row of a JMAP subscription id, if it is one of ours.
pub(crate) fn store_id(id: &JmapPushId) -> Option<PushSubscriptionId> {
    parse_number(id.as_str(), 'w').and_then(PushSubscriptionId::from_raw)
}

/// Version of the first sealed layout: `1 || url length (u16 BE) || url ||
/// p256dh || auth`.
const SEALED_V1: u8 = 1;
/// The layout with `emailPush` (2026-11-01): version 1's and then the
/// `emailPush` JSON, as `PushSubscription/get` shows it (empty: none).
const SEALED_V2: u8 = 2;

/// What is sealed of a subscription: where to send, the browser keys and
/// what of the delivered mail goes in the push.
#[derive(Debug)]
pub(crate) struct Opened {
    pub(crate) url: String,
    pub(crate) keys: ClientKeys,
    /// `emailPush` as shown (`None`: none).
    pub(crate) email_push: Option<serde_json::Value>,
}

/// Seals the URL, keys and `emailPush` of subscription `id`. The
/// `emailPush` (the user's own filter: folder ids, keywords) is sealed
/// like the rest.
pub(crate) fn seal(
    key: &AccountKey,
    id: PushSubscriptionId,
    url: &str,
    keys: &ClientKeys,
    email_push: Option<&serde_json::Value>,
) -> Result<Vec<u8>, StoreError> {
    let length = u16::try_from(url.len()).map_err(|_| StoreError::Invalid("push url"))?;
    let config = email_push
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| StoreError::Invalid("emailPush"))?
        .unwrap_or_default();
    let mut plain = Zeroizing::new(Vec::with_capacity(
        url.len().saturating_add(84).saturating_add(config.len()),
    ));
    plain.push(SEALED_V2);
    plain.extend_from_slice(&length.to_be_bytes());
    plain.extend_from_slice(url.as_bytes());
    plain.extend_from_slice(&keys.to_bytes());
    plain.extend_from_slice(&config);
    let binding_bytes = id.get().to_be_bytes();
    let binding = Binding::new(&binding_bytes).map_err(|_| StoreError::Invalid("binding"))?;
    key.seal_bound(Field::PushSubscription, &binding, &plain)
        .map(|sealed| sealed.as_bytes().to_vec())
        .map_err(|_| StoreError::Invalid("push subscription could not be sealed"))
}

/// Opens what [`seal`] made (or the first layout); `None` if it does not
/// open or parse.
pub(crate) fn open(key: &AccountKey, id: PushSubscriptionId, sealed: &[u8]) -> Option<Opened> {
    use secrecy::ExposeSecret as _;
    let binding_bytes = id.get().to_be_bytes();
    let binding = Binding::new(&binding_bytes).ok()?;
    let sealed = SealedField::from_bytes(sealed).ok()?;
    let plain = key
        .open_bound(Field::PushSubscription, &binding, &sealed)
        .ok()?;
    let plain = plain.expose_secret();
    let (version, rest) = plain.split_first()?;
    if !matches!(*version, SEALED_V1 | SEALED_V2) {
        return None;
    }
    let (length, rest) = rest.split_at_checked(2)?;
    let length = usize::from(u16::from_be_bytes(length.try_into().ok()?));
    let (url, rest) = rest.split_at_checked(length)?;
    let url = std::str::from_utf8(url).ok()?.to_owned();
    let (keys, config) = if *version == SEALED_V1 {
        (rest, &[][..])
    } else {
        rest.split_at_checked(ClientKeys::STORED_LEN)?
    };
    let keys = ClientKeys::from_bytes(keys).ok()?;
    let email_push = if config.is_empty() {
        None
    } else {
        Some(serde_json::from_slice(config).ok()?)
    };
    Some(Opened {
        url,
        keys,
        email_push,
    })
}

#[cfg(test)]
mod tests {
    use nebula_crypto::{AccountId as CryptoAccountId, DataKey};

    use super::*;

    #[test]
    fn ids_and_sealing() {
        let id = PushSubscriptionId::from_raw(12).unwrap();
        let jmap = jmap_id(id).unwrap();
        assert_eq!(jmap.as_str(), "w12");
        assert_eq!(store_id(&jmap), Some(id));
        assert_eq!(store_id(&"w012".parse().unwrap()), None);
        assert_eq!(store_id(&"e12".parse().unwrap()), None);

        let key = AccountKey::new(
            CryptoAccountId::new(b"login:alice").unwrap(),
            DataKey::generate().unwrap(),
        );
        let keys = ClientKeys::parse(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
            "BTBZMqHH6r4Tts7J_aSIgg",
        )
        .unwrap();
        let url = "https://fcm.googleapis.com/fcm/send/abc";
        let sealed = seal(&key, id, url, &keys, None).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("fcm"));
        let opened = open(&key, id, &sealed).unwrap();
        assert_eq!(opened.url, url);
        assert_eq!(opened.keys, keys);
        assert!(opened.email_push.is_none());
        // Bound to the row: another id does not open it.
        assert!(open(&key, PushSubscriptionId::from_raw(13).unwrap(), &sealed).is_none());

        // With emailPush, sealed too.
        let config = serde_json::json!({"a1": {"filter": {"inMailbox": "m1"},
            "properties": ["id"], "urgency": "normal"}});
        let sealed = seal(&key, id, url, &keys, Some(&config)).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("inMailbox"));
        let opened = open(&key, id, &sealed).unwrap();
        assert_eq!((opened.url.as_str(), &opened.keys), (url, &keys));
        assert_eq!(opened.email_push, Some(config));

        // The first layout (before emailPush) still opens.
        let mut plain = vec![SEALED_V1];
        plain.extend_from_slice(&u16::try_from(url.len()).unwrap().to_be_bytes());
        plain.extend_from_slice(url.as_bytes());
        plain.extend_from_slice(&keys.to_bytes());
        let binding_bytes = id.get().to_be_bytes();
        let binding = Binding::new(&binding_bytes).unwrap();
        let old = key
            .seal_bound(Field::PushSubscription, &binding, &plain)
            .unwrap();
        let opened = open(&key, id, old.as_bytes()).unwrap();
        assert_eq!(opened.url, url);
        assert!(opened.email_push.is_none());
    }
}
