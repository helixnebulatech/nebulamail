// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The delivery task of one account (see [`super`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Weak};
use std::time::Duration;

use nebula_crypto::AccountKey;
use nebula_groupware::{AlertListener, GroupwareEvent};
use nebula_jmap_proto::object::PushVerification;
use nebula_jmap_proto::push::CalendarAlert;
use nebula_jmap_proto::types::DataType as JmapDataType;
use nebula_jmap_proto::{AccountId as JmapAccountId, State};
use nebula_mail::{MailEvent, StateChanged};
use nebula_store::{AccountId, DataType, EmailId, PushSubscriptionId, Store, UnixTime};
use nebula_webpush::{ClientKeys, Message, Outcome, PushEndpoint, Urgency, ece};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::email_push::{self, EmailPush};
use super::{Command, WebPushSettings, jmap_id, open};
use crate::core::backend::Inner;
use crate::core::events::{alert_for, jmap_data_type, state_change};
use crate::core::ids;

/// Alerts kept per subscription while it can't be reached.
const MAX_ALERTS: usize = 32;
/// How long the push service keeps each kind for an offline browser.
const STATE_TTL: Duration = Duration::from_hours(24);
const VERIFICATION_TTL: Duration = Duration::from_hours(1);
/// Delivered emails kept per subscription for its next `EmailPush`; more
/// (a big batch) and it gets a `StateChange` instead (the client asks
/// what's new). The web app shows at most 50 at once.
const MAX_PUSHED_EMAILS: usize = 50;
/// The types whose `StateChange` an `EmailPush` replaces (Stalwart:
/// an `EmailPush` goes to a subscription wanting any of them).
const DELIVERY_TYPES: &[JmapDataType] = &[
    JmapDataType::EmailDelivery,
    JmapDataType::Email,
    JmapDataType::Mailbox,
    JmapDataType::Thread,
];

/// Every mail type, for a listener that missed events.
const MAIL_TYPES: &[DataType] = &[
    DataType::Mailbox,
    DataType::Email,
    DataType::Thread,
    DataType::Identity,
    DataType::EmailSubmission,
    DataType::EmailDelivery,
];
const GROUPWARE_TYPES: &[DataType] = &[
    DataType::Calendar,
    DataType::CalendarEvent,
    DataType::AddressBook,
    DataType::ContactCard,
];

pub(super) struct Listen {
    pub(super) mail: Receiver<MailEvent>,
    pub(super) groupware: Option<Receiver<GroupwareEvent>>,
}

pub(super) struct Task {
    pub(super) inner: Weak<Inner>,
    pub(super) settings: Arc<WebPushSettings>,
    pub(super) account: AccountId,
    pub(super) jmap_id: JmapAccountId,
    pub(super) key: Arc<AccountKey>,
    pub(super) commands: mpsc::Receiver<Command>,
    pub(super) stopped: watch::Receiver<bool>,
    pub(super) listen: Listen,
}

/// One subscription and what waits for it.
struct Sub {
    id: PushSubscriptionId,
    endpoint: PushEndpoint,
    keys: ClientKeys,
    /// `None`: every type.
    types: Option<BTreeSet<JmapDataType>>,
    expires: i64,
    verified: bool,
    code: String,
    verify: bool,
    states: BTreeMap<JmapDataType, State>,
    alerts: VecDeque<CalendarAlert>,
    /// What of the delivered mail goes in the push (`emailPush`).
    email_push: Option<EmailPush>,
    /// Delivered emails waiting for the next `EmailPush`, oldest first.
    delivered: Vec<EmailId>,
    /// The `Email` state after the newest of them.
    delivered_state: Option<State>,
    /// Delivered mail didn't fit in one `EmailPush`: a `StateChange` of
    /// [`DELIVERY_TYPES`] is due instead.
    overflow: bool,
    attempts: usize,
    not_before: Instant,
}

impl Sub {
    fn wants(&self, data_type: &JmapDataType) -> bool {
        self.verified && self.types.as_ref().is_none_or(|t| t.contains(data_type))
    }

    /// Whether new mail goes to it as an `EmailPush`.
    fn takes_delivery(&self) -> bool {
        self.email_push.is_some() && DELIVERY_TYPES.iter().any(|t| self.wants(t))
    }

    fn has_pending(&self) -> bool {
        self.verify
            || (self.verified
                && (!self.states.is_empty()
                    || !self.alerts.is_empty()
                    || !self.delivered.is_empty()))
    }

    /// Adds delivered emails; `false` (and nothing kept) if there are too
    /// many for one push: then a `StateChange` goes instead.
    fn deliver(&mut self, emails: &[EmailId], state: Option<State>) -> bool {
        for email in emails {
            if !self.delivered.contains(email) {
                self.delivered.push(*email);
            }
        }
        if self.delivered.len() > MAX_PUSHED_EMAILS {
            self.delivered.clear();
            self.delivered_state = None;
            return false;
        }
        if state.is_some() {
            self.delivered_state = state;
        }
        true
    }
}

/// What one POST carries.
enum Payload {
    Verification,
    Alert(CalendarAlert),
    /// The JSON is built (it depends on the store); the emails and state
    /// are kept for a retry.
    Email {
        json: Vec<u8>,
        urgency: Urgency,
        emails: Vec<EmailId>,
        state: Option<State>,
    },
    State(BTreeMap<JmapDataType, State>),
}

/// Messages the account may still send: a bucket of `burst`, one more
/// every `refill`.
struct Bucket {
    tokens: u32,
    burst: u32,
    refill: Duration,
    last: Instant,
}

impl Bucket {
    fn new(burst: u32, refill: Duration) -> Self {
        Self {
            tokens: burst,
            burst,
            refill: refill.max(Duration::from_millis(1)),
            last: Instant::now(),
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last);
        let earned = elapsed.as_nanos() / self.refill.as_nanos().max(1);
        if earned > 0 {
            let earned = u32::try_from(earned).unwrap_or(u32::MAX);
            self.tokens = self.tokens.saturating_add(earned).min(self.burst);
            self.last = now;
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens = self.tokens.saturating_sub(1);
        true
    }

    /// Gives back a token taken for nothing (no email passed the filter).
    fn refund(&mut self) {
        self.tokens = self.tokens.saturating_add(1).min(self.burst);
    }

    fn next_token(&self, now: Instant) -> Instant {
        self.last.checked_add(self.refill).unwrap_or(now).max(now)
    }
}

/// The store type of a JMAP mail type of [`DELIVERY_TYPES`].
fn store_data_type(name: &JmapDataType) -> Option<DataType> {
    match name {
        JmapDataType::EmailDelivery => Some(DataType::EmailDelivery),
        JmapDataType::Email => Some(DataType::Email),
        JmapDataType::Mailbox => Some(DataType::Mailbox),
        JmapDataType::Thread => Some(DataType::Thread),
        _ => None,
    }
}

/// Whether a subscription read from the store still needs its
/// `PushVerification`: one not verified that the task did not have yet
/// (just created, or the task started again: after a restart, or once a
/// full command queue lost a message) gets it; one it had keeps what it
/// had (`previous`: whether it was still to send; not sent again on
/// every change).
fn verify_pending(verified: bool, previous: Option<bool>) -> bool {
    !verified && previous.unwrap_or(true)
}

/// What happens when the tries of a payload run out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GiveUp {
    /// What waits is dropped; the subscription stays.
    Drop,
    /// The subscription goes: it never got its verification, so it can't
    /// get anything else (the web makes another one).
    DeleteSub,
}

const fn give_up(payload: &Payload) -> GiveUp {
    match payload {
        Payload::Verification => GiveUp::DeleteSub,
        Payload::Alert(_) | Payload::Email { .. } | Payload::State(_) => GiveUp::Drop,
    }
}

/// A type whose states the shared accounts have too.
fn is_groupware(name: &JmapDataType) -> bool {
    GROUPWARE_TYPES
        .iter()
        .filter_map(|data_type| jmap_data_type(*data_type))
        .any(|groupware| groupware == *name)
}

fn later(now: Instant, wait: Duration) -> Instant {
    now.checked_add(wait).unwrap_or(now)
}

fn unix_now() -> i64 {
    UnixTime::now().map_or(0, UnixTime::secs)
}

impl Task {
    fn store(&self) -> Option<Store> {
        self.inner.upgrade().map(|inner| inner.store.clone())
    }

    /// Reads the subscriptions again, keeping what waits for the ones
    /// that stay. Rows that no longer open or pass the endpoint rules
    /// (the admin changed them) are deleted.
    async fn reload(&self, old: Vec<Sub>) -> Vec<Sub> {
        let Some(store) = self.store() else {
            return old;
        };
        let rows = match store
            .deliverable_push_subscriptions(self.account, UnixTime::from_secs(unix_now()))
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(account = self.account.get(), %error, "web push: subscriptions unavailable");
                return old;
            }
        };
        let mut old: BTreeMap<PushSubscriptionId, Sub> =
            old.into_iter().map(|sub| (sub.id, sub)).collect();
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let opened = open(&self.key, row.id, &row.sealed).and_then(|opened| {
                let email_push = match &opened.email_push {
                    None => None,
                    Some(value) => Some(EmailPush::from_json(&self.jmap_id, value)?),
                };
                PushEndpoint::parse(&opened.url, &self.settings.policy)
                    .ok()
                    .map(|endpoint| (endpoint, opened.keys, email_push))
            });
            let Some((endpoint, keys, email_push)) = opened else {
                tracing::warn!(
                    account = self.account.get(),
                    "web push: a subscription no longer opens or is not allowed; deleted"
                );
                let _ = store.drop_push_subscription(self.account, row.id).await;
                continue;
            };
            let types = row.types.map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.parse::<JmapDataType>().ok())
                    .collect()
            });
            let mut sub = Sub {
                id: row.id,
                endpoint,
                keys,
                types,
                expires: row.expires.secs(),
                verified: row.verified,
                code: row.verification_code,
                verify: false,
                states: BTreeMap::new(),
                alerts: VecDeque::new(),
                email_push,
                delivered: Vec::new(),
                delivered_state: None,
                overflow: false,
                attempts: 0,
                not_before: Instant::now(),
            };
            let previous = old.remove(&row.id);
            sub.verify = verify_pending(sub.verified, previous.as_ref().map(|p| p.verify));
            if let Some(previous) = previous {
                sub.states = previous.states;
                sub.alerts = previous.alerts;
                // Kept while it still has `emailPush` (removed meanwhile:
                // the next StateChange tells of that mail).
                if sub.email_push.is_some() {
                    sub.delivered = previous.delivered;
                    sub.delivered_state = previous.delivered_state;
                }
                sub.overflow = previous.overflow;
                sub.attempts = previous.attempts;
                sub.not_before = previous.not_before;
            }
            out.push(sub);
        }
        out
    }

    /// Holds an alert listener while some subscription wants alerts.
    fn update_listener(&self, subs: &[Sub], listener: &mut Option<AlertListener>) {
        let wanted = self.settings.calendar_alerts
            && subs.iter().any(|s| s.wants(&JmapDataType::CalendarAlert));
        if !wanted {
            *listener = None;
        } else if listener.is_none()
            && let Some(inner) = self.inner.upgrade()
            && let Some(groupware) = &inner.groupware
        {
            *listener = Some(groupware.listen_alerts(self.account));
        }
    }

    /// New states of these types, for the subscriptions that want them.
    async fn changed(&self, subs: &mut [Sub], types: &[DataType]) {
        self.changed_except(subs, types, &BTreeSet::new()).await;
    }

    /// [`Task::changed`], but the subscriptions in `took` (they got the
    /// delivery as an `EmailPush`) get none of [`DELIVERY_TYPES`].
    async fn changed_except(
        &self,
        subs: &mut [Sub],
        types: &[DataType],
        took: &BTreeSet<PushSubscriptionId>,
    ) {
        let Some(store) = self.store() else {
            return;
        };
        let gets = |sub: &Sub, name: &JmapDataType| {
            sub.wants(name) && !(took.contains(&sub.id) && DELIVERY_TYPES.contains(name))
        };
        for data_type in types {
            let Some(name) = jmap_data_type(*data_type) else {
                continue;
            };
            if !subs.iter().any(|s| gets(s, &name)) {
                continue;
            }
            match store.state(self.account, *data_type).await {
                Ok(seq) => {
                    let state = ids::state(seq);
                    for sub in subs.iter_mut().filter(|s| gets(s, &name)) {
                        sub.states.insert(name.clone(), state.clone());
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "web push: state unavailable");
                }
            }
        }
    }

    /// A mail event: new mail goes as an `EmailPush` to the subscriptions
    /// with `emailPush` (instead of the `StateChange` of the delivery's
    /// types), as a `StateChange` to the rest.
    async fn mail_changed(&self, subs: &mut [Sub], changed: &StateChanged) {
        let mut took = BTreeSet::new();
        if !changed.delivered.is_empty() && subs.iter().any(Sub::takes_delivery) {
            let state = match self.store() {
                Some(store) => store.state(self.account, DataType::Email).await.ok(),
                None => None,
            }
            .map(ids::state);
            for sub in subs.iter_mut().filter(|s| s.takes_delivery()) {
                // Too many: this one gets the StateChange.
                if sub.deliver(&changed.delivered, state.clone()) {
                    took.insert(sub.id);
                }
            }
        }
        self.changed_except(subs, &changed.data_types, &took).await;
    }

    /// Quota (RFC 9425 §6): after a mail change that may move the quotas,
    /// their state for the subscriptions that want it, when it moved
    /// (`last`: the last one this task saw).
    async fn quota_changed(&self, subs: &mut [Sub], types: &[DataType], last: &mut Option<State>) {
        let quota = JmapDataType::Quota;
        if !crate::quota::moves_quota(types) || !subs.iter().any(|s| s.wants(&quota)) {
            return;
        }
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let Some(state) = crate::quota::push_state(&inner, self.account).await else {
            return;
        };
        if last.as_ref() == Some(&state) {
            return;
        }
        *last = Some(state.clone());
        for sub in subs.iter_mut().filter(|s| s.wants(&quota)) {
            sub.states.insert(quota.clone(), state.clone());
        }
    }

    /// The `StateChange` due to subscriptions whose delivered mail didn't
    /// fit in one `EmailPush`.
    async fn overflowed(&self, subs: &mut [Sub]) {
        if !subs.iter().any(|s| s.overflow) {
            return;
        }
        let Some(store) = self.store() else {
            return;
        };
        for name in DELIVERY_TYPES {
            let Some(data_type) = store_data_type(name) else {
                continue;
            };
            let Ok(seq) = store.state(self.account, data_type).await else {
                continue;
            };
            let state = ids::state(seq);
            for sub in subs.iter_mut().filter(|s| s.overflow && s.wants(name)) {
                sub.states.insert(name.clone(), state.clone());
            }
        }
        for sub in subs.iter_mut() {
            sub.overflow = false;
        }
    }

    /// The `EmailPush` of these delivered emails for this subscription;
    /// `Ok(None)`: none passes its filter (nothing to send). `Err`: the
    /// store failed (tried again later).
    async fn email_push(
        &self,
        config: &EmailPush,
        emails: &[EmailId],
        state: Option<State>,
    ) -> Result<Option<Vec<u8>>, ()> {
        let Some(inner) = self.inner.upgrade() else {
            return Err(());
        };
        let mut stored = match inner.store.emails(self.account, emails.to_vec()).await {
            Ok(stored) => stored,
            Err(error) => {
                tracing::warn!(account = self.account.get(), %error, "web push: emails unavailable");
                return Err(());
            }
        };
        stored.retain(|email| config.matches(email));
        // A preview the server didn't give: the first bytes of the text,
        // only when asked for (as `Email/get`). Without it, left out.
        if config.wants_preview()
            && let Some(mail) = inner.mail.account(self.account)
        {
            let missing: Vec<EmailId> = stored
                .iter()
                .filter(|e| e.meta.preview.is_none())
                .map(|e| e.id)
                .collect();
            if !missing.is_empty()
                && let Ok(mut found) = mail.fill_previews(&missing).await
            {
                for email in &mut stored {
                    if let Some(preview) = found.remove(&email.id) {
                        email.meta.preview = Some(preview);
                    }
                }
            }
        }
        Ok(email_push::build(
            config,
            &self.jmap_id,
            state,
            &stored,
            ece::MAX_PLAINTEXT,
        ))
    }

    fn alert(subs: &mut [Sub], alert: &CalendarAlert) {
        for sub in subs
            .iter_mut()
            .filter(|s| s.wants(&JmapDataType::CalendarAlert))
        {
            if sub.alerts.len() >= MAX_ALERTS {
                sub.alerts.pop_front();
            }
            sub.alerts.push_back(alert.clone());
        }
    }

    fn message_of(
        &self,
        sub: &Sub,
        payload: &Payload,
        shared: &[JmapAccountId],
    ) -> Option<(Vec<u8>, Duration, Urgency, Option<&'static str>)> {
        let (json, ttl, urgency, topic) = match payload {
            Payload::Verification => (
                serde_json::to_vec(&PushVerification {
                    push_subscription_id: jmap_id(sub.id).ok()?,
                    verification_code: sub.code.clone(),
                }),
                VERIFICATION_TTL,
                Urgency::Normal,
                None,
            ),
            Payload::Alert(alert) => (
                serde_json::to_vec(alert),
                self.settings.alert_ttl,
                Urgency::High,
                None,
            ),
            // No topic: a newer one must not replace mail not yet shown.
            Payload::Email { json, urgency, .. } => (Ok(json.clone()), STATE_TTL, *urgency, None),
            // A newer state replaces one the browser has not fetched yet.
            // Groupware states go to the shared accounts too, as many as
            // fit in one message (the own account always does).
            Payload::State(states) => (
                serde_json::to_vec(&state_change(
                    &self.jmap_id,
                    states.clone(),
                    shared,
                    Some(ece::MAX_PLAINTEXT),
                )),
                STATE_TTL,
                Urgency::Normal,
                Some("StateChange"),
            ),
        };
        Some((json.ok()?, ttl, urgency, topic))
    }

    /// Sends what is due, within the account's budget. Subscriptions the
    /// push service says are gone (or refuses) are deleted.
    async fn deliver(&self, subs: &mut Vec<Sub>, bucket: &mut Bucket) {
        let now_unix = unix_now();
        subs.retain(|sub| sub.expires > now_unix);
        // The shared accounts, read once per pass if a state goes out.
        let mut shared: Option<Vec<JmapAccountId>> = None;
        let mut index = 0;
        while let Some(sub) = subs.get_mut(index) {
            let now = Instant::now();
            if !sub.has_pending() || sub.not_before > now {
                index = index.saturating_add(1);
                continue;
            }
            if !bucket.take(now) {
                // Over budget: everyone waits for the next token.
                let next = bucket.next_token(now);
                for sub in subs.iter_mut() {
                    sub.not_before = sub.not_before.max(next);
                }
                return;
            }
            let Some(payload) = self.next_payload(sub).await else {
                // Nothing to send after all (or a retry is scheduled).
                bucket.refund();
                continue;
            };
            if shared.is_none()
                && let Payload::State(states) = &payload
                && states.keys().any(is_groupware)
            {
                shared = Some(match self.store() {
                    Some(store) => {
                        crate::sharing::accounts::push_ids(&store, self.account, &self.jmap_id)
                            .await
                    }
                    None => Vec::new(),
                });
            }
            let Some((body, ttl, urgency, topic)) =
                self.message_of(sub, &payload, shared.as_deref().unwrap_or_default())
            else {
                continue;
            };
            let message = Message {
                payload: &body,
                ttl,
                urgency,
                topic,
            };
            let result = self
                .settings
                .client
                .send(&sub.endpoint, &sub.keys, message, now_unix)
                .await;
            let after = Instant::now();
            match result {
                Ok(Outcome::Delivered) => {
                    sub.attempts = 0;
                    sub.not_before = later(after, self.settings.min_interval);
                }
                Ok(
                    Outcome::Gone
                    | Outcome::Rejected(_)
                    | Outcome::Failed(nebula_webpush::Failure::Blocked(_)),
                ) => {
                    tracing::info!(
                        account = self.account.get(),
                        "web push: subscription dropped (gone or refused)"
                    );
                    let id = sub.id;
                    subs.remove(index);
                    if let Some(store) = self.store() {
                        let _ = store.drop_push_subscription(self.account, id).await;
                    }
                }
                Ok(other) => {
                    let retry_after = match other {
                        Outcome::Throttled(wait) => wait,
                        _ => None,
                    };
                    if self.retry(sub, payload, retry_after, after) == Some(GiveUp::DeleteSub) {
                        tracing::info!(
                            account = self.account.get(),
                            "web push: verification undeliverable; subscription deleted"
                        );
                        let id = sub.id;
                        subs.remove(index);
                        if let Some(store) = self.store() {
                            let _ = store.drop_push_subscription(self.account, id).await;
                        }
                    }
                }
                Err(error) => {
                    // The message can't be built (too large): dropped.
                    tracing::warn!(account = self.account.get(), %error, "web push: message dropped");
                }
            }
        }
    }

    /// What goes next to `sub`. `None`: nothing after all (no delivered
    /// email passed its filter, or the store failed and a retry is
    /// scheduled).
    async fn next_payload(&self, sub: &mut Sub) -> Option<Payload> {
        if sub.verify {
            sub.verify = false;
            return Some(Payload::Verification);
        }
        if let Some(alert) = sub.alerts.pop_front() {
            return Some(Payload::Alert(alert));
        }
        if let Some(config) = sub.email_push.clone()
            && !sub.delivered.is_empty()
        {
            let emails = std::mem::take(&mut sub.delivered);
            let state = sub.delivered_state.take();
            return match self.email_push(&config, &emails, state.clone()).await {
                Ok(Some(json)) => Some(Payload::Email {
                    json,
                    urgency: config.urgency(),
                    emails,
                    state,
                }),
                Ok(None) => None,
                Err(()) => {
                    let payload = Payload::Email {
                        json: Vec::new(),
                        urgency: Urgency::Normal,
                        emails,
                        state,
                    };
                    self.retry(sub, payload, None, Instant::now());
                    None
                }
            };
        }
        sub.delivered.clear();
        (!sub.states.is_empty()).then(|| Payload::State(std::mem::take(&mut sub.states)))
    }

    /// Puts a failed payload back and schedules the next try; after the
    /// last one, what waits is dropped and what to do is returned.
    fn retry(
        &self,
        sub: &mut Sub,
        payload: Payload,
        retry_after: Option<Duration>,
        now: Instant,
    ) -> Option<GiveUp> {
        let giving_up = give_up(&payload);
        match payload {
            Payload::Verification => sub.verify = true,
            Payload::Alert(alert) => sub.alerts.push_front(alert),
            Payload::Email { emails, state, .. } => {
                // Newer deliveries (arrived meanwhile) after these.
                let newer = std::mem::replace(&mut sub.delivered, emails);
                let newer_state = sub.delivered_state.take();
                sub.delivered_state = state;
                // Too many now: a StateChange instead (the client asks
                // what's new).
                sub.overflow |= !sub.deliver(&newer, newer_state);
            }
            Payload::State(states) => {
                // Newer states (arrived meanwhile) win.
                for (name, state) in states {
                    sub.states.entry(name).or_insert(state);
                }
            }
        }
        sub.attempts = sub.attempts.saturating_add(1);
        if let Some(wait) = self.settings.retries.get(sub.attempts.saturating_sub(1)) {
            let wait = retry_after.map_or(*wait, |r| r.max(*wait));
            sub.not_before = later(now, wait);
            None
        } else {
            tracing::info!(
                account = self.account.get(),
                "web push: giving up on pending messages"
            );
            sub.verify = false;
            sub.states.clear();
            sub.alerts.clear();
            sub.delivered.clear();
            sub.delivered_state = None;
            sub.attempts = 0;
            sub.not_before = later(now, self.settings.min_interval);
            Some(giving_up)
        }
    }
}

async fn recv_groupware(
    receiver: &mut Option<Receiver<GroupwareEvent>>,
) -> Option<Result<GroupwareEvent, RecvError>> {
    match receiver {
        Some(receiver) => Some(receiver.recv().await),
        None => std::future::pending().await,
    }
}

pub(super) async fn run(mut task: Task) {
    let mut subs = task.reload(Vec::new()).await;
    let mut bucket = Bucket::new(task.settings.burst, task.settings.refill);
    let mut listener = None;
    task.update_listener(&subs, &mut listener);
    let mut groupware = task.listen.groupware.take();
    let mut quota = None;
    loop {
        task.overflowed(&mut subs).await;
        task.deliver(&mut subs, &mut bucket).await;
        let wake = subs
            .iter()
            .filter(|s| s.has_pending())
            .map(|s| s.not_before)
            .min()
            .unwrap_or_else(|| later(Instant::now(), Duration::from_hours(1)));
        tokio::select! {
            changed = task.stopped.changed() => {
                if changed.is_err() || *task.stopped.borrow() {
                    return;
                }
            }
            command = task.commands.recv() => match command {
                None => return,
                Some(Command::Reload) => {
                    // A new subscription gets its verification here.
                    subs = task.reload(subs).await;
                    task.update_listener(&subs, &mut listener);
                }
            },
            event = task.listen.mail.recv() => match event {
                Ok(MailEvent::StateChanged(changed)) if changed.account == task.account => {
                    task.mail_changed(&mut subs, &changed).await;
                    task.quota_changed(&mut subs, &changed.data_types, &mut quota).await;
                }
                Err(RecvError::Lagged(_)) => {
                    task.changed(&mut subs, MAIL_TYPES).await;
                    task.quota_changed(&mut subs, MAIL_TYPES, &mut quota).await;
                }
                Err(RecvError::Closed) => return,
                Ok(_) => {}
            },
            event = recv_groupware(&mut groupware) => match event {
                Some(Ok(GroupwareEvent::StateChanged(changed))) if changed.account == task.account => {
                    task.changed(&mut subs, &changed.data_types).await;
                }
                Some(Ok(GroupwareEvent::CalendarAlert(due)))
                    if due.account == task.account && task.settings.calendar_alerts =>
                {
                    // In the account its event is in (a shared one too).
                    let alert = match task.store() {
                        Some(store) => alert_for(&store, task.account, &task.jmap_id, due).await,
                        None => None,
                    };
                    if let Some(alert) = alert {
                        Task::alert(&mut subs, &alert);
                    }
                }
                Some(Err(RecvError::Lagged(_))) => task.changed(&mut subs, GROUPWARE_TYPES).await,
                Some(Err(RecvError::Closed)) => groupware = None,
                Some(Ok(_)) | None => {}
            },
            () = tokio::time::sleep_until(wake) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_is_sent_again_when_the_task_starts() {
        // New to the task (created, or after a restart): sent.
        assert!(verify_pending(false, None));
        // Already sent, or still waiting: kept as it was.
        assert!(!verify_pending(false, Some(false)));
        assert!(verify_pending(false, Some(true)));
        // Verified: never.
        assert!(!verify_pending(true, None));
        assert!(!verify_pending(true, Some(true)));
        // An undeliverable verification takes the subscription with it.
        assert_eq!(give_up(&Payload::Verification), GiveUp::DeleteSub);
        assert_eq!(give_up(&Payload::State(BTreeMap::new())), GiveUp::Drop);
    }

    #[tokio::test]
    async fn bucket() {
        let start = Instant::now();
        let mut bucket = Bucket::new(2, Duration::from_secs(10));
        bucket.last = start;
        assert!(bucket.take(start));
        assert!(bucket.take(start));
        assert!(!bucket.take(start));
        assert_eq!(
            bucket.next_token(start),
            later(start, Duration::from_secs(10))
        );
        assert!(!bucket.take(later(start, Duration::from_secs(9))));
        let then = later(start, Duration::from_secs(10));
        assert!(bucket.take(then));
        assert!(!bucket.take(then));
        // Never more than the burst.
        let much_later = later(start, Duration::from_hours(1));
        assert!(bucket.take(much_later));
        assert!(bucket.take(much_later));
        assert!(!bucket.take(much_later));
    }
}
