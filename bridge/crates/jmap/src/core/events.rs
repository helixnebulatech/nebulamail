// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Push: the mail and groupware layers' `StateChanged` events as JMAP
//! `StateChange` objects, with the account's current states read from the
//! store. While a push stream is open, the account counts as watched by
//! the groupware layer (it polls the DAV server every minute instead of
//! every five), and the sieve layer reads its scripts.
//! Calendar alerts of the groupware layer come out as
//! `CalendarAlert` push objects.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use futures::StreamExt as _;
use futures::stream::{self, BoxStream};
use nebula_groupware::alerts::AlertDue;
use nebula_groupware::model::calendar::RecurrenceKey;
use nebula_groupware::{AlertListener, Groupware, GroupwareEvent, Watch};
use nebula_http::{AccountContext, PushEvent};
use nebula_jmap_proto::push::{CalendarAlert, StateChange};
use nebula_jmap_proto::types::{DataType as JmapDataType, LocalDateTime};
use nebula_jmap_proto::{AccountId as JmapAccountId, State};
use nebula_mail::MailEvent;
use nebula_store::{AccountId, DataType, Store};
use time::{OffsetDateTime, PrimitiveDateTime};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

use super::backend::Inner;
use super::ids;
use crate::sieve::Listening;

/// Every mail data type with a state, for a listener that missed events.
const ALL: &[DataType] = &[
    DataType::Mailbox,
    DataType::Email,
    DataType::Thread,
    DataType::Identity,
    DataType::EmailSubmission,
];

/// Every groupware data type with a state.
const ALL_GROUPWARE: &[DataType] = &[
    DataType::Calendar,
    DataType::CalendarEvent,
    DataType::AddressBook,
    DataType::ContactCard,
];

/// `events`' next value; a layer that isn't there never has one.
async fn next_from<T: Clone>(events: &mut Option<Receiver<T>>) -> Result<T, RecvError> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

/// A name `jmap-proto` may not have as a variant yet: parsed, so it is
/// the variant once there is one (and `Other` until then).
fn named(name: &str) -> Option<JmapDataType> {
    name.parse().ok()
}

/// The JMAP name of a store data type.
pub(crate) fn jmap_data_type(data_type: DataType) -> Option<JmapDataType> {
    Some(match data_type {
        DataType::Mailbox => JmapDataType::Mailbox,
        DataType::Email => JmapDataType::Email,
        DataType::Thread => JmapDataType::Thread,
        DataType::Identity => JmapDataType::Identity,
        DataType::EmailSubmission => JmapDataType::EmailSubmission,
        DataType::EmailDelivery => JmapDataType::EmailDelivery,
        DataType::Calendar => return named("Calendar"),
        DataType::CalendarEvent => return named("CalendarEvent"),
        DataType::AddressBook => return named("AddressBook"),
        DataType::ContactCard => return named("ContactCard"),
        DataType::ParticipantIdentity => return named("ParticipantIdentity"),
        DataType::SieveScript => return named("SieveScript"),
        _ => return None,
    })
}

/// The data types a shared account has (its push names them too).
const GROUPWARE_NAMES: [&str; 4] = ["Calendar", "CalendarEvent", "AddressBook", "ContactCard"];

/// The `recurrenceId` of an alert: the occurrence's wall-clock start in
/// the event's zone, as Stalwart sends it.
fn recurrence_id(key: RecurrenceKey) -> Option<LocalDateTime> {
    let at = OffsetDateTime::from_unix_timestamp(key.naive()).ok()?;
    LocalDateTime::new(PrimitiveDateTime::new(at.date(), at.time()))
}

/// The push object of a due alert.
pub(crate) fn calendar_alert(jmap_id: &JmapAccountId, due: AlertDue) -> Option<CalendarAlert> {
    let event = crate::calendars::ids::event_id(due.resource).ok()?;
    let recurrence = match due.recurrence {
        Some(key) => Some(recurrence_id(key)?),
        None => None,
    };
    CalendarAlert::new(jmap_id.clone(), event, due.uid, recurrence, due.alert_id).ok()
}

/// The push object of a due alert, in the account its event is in (a
/// shared one if its calendar is someone else's, `sharing::accounts`).
pub(crate) async fn alert_for(
    store: &Store,
    account: AccountId,
    own: &JmapAccountId,
    due: AlertDue,
) -> Option<CalendarAlert> {
    let jmap_id =
        crate::sharing::accounts::account_of_resource(store, account, own, due.resource).await;
    calendar_alert(&jmap_id, due)
}

/// The `StateChange` of new states of the account `own`: its own, and the
/// groupware ones also in each of the `shared` accounts (those who share
/// calendars or address books with the user have the same groupware
/// states: one change log). With a `limit` (bytes of JSON, a Web Push
/// message), the shared accounts that don't fit are left out; the own
/// account always goes (the web reloads groupware when it changes).
pub(crate) fn state_change(
    own: &JmapAccountId,
    states: BTreeMap<JmapDataType, State>,
    shared: &[JmapAccountId],
    limit: Option<usize>,
) -> StateChange {
    let groupware: BTreeMap<JmapDataType, State> = states
        .iter()
        .filter(|(name, _)| GROUPWARE_NAMES.contains(&name.as_str()))
        .map(|(name, state)| (name.clone(), state.clone()))
        .collect();
    let mut change = StateChange {
        changed: BTreeMap::from([(own.clone(), states)]),
    };
    if groupware.is_empty() {
        return change;
    }
    let mut used = json_len(&change);
    // `"id":{…},` for each one more.
    let entry = json_len(&groupware).saturating_add(2);
    for id in shared {
        let more = entry.saturating_add(json_len(id));
        if limit.is_some_and(|limit| used.saturating_add(more) > limit) {
            tracing::debug!("push: shared accounts left out of a StateChange that would not fit");
            break;
        }
        used = used.saturating_add(more);
        change.changed.insert(id.clone(), groupware.clone());
    }
    change
}

/// Bytes of a value as JSON.
fn json_len<T: serde::Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |json| json.len())
}

/// Which layer an event came from.
enum Heard {
    Mail(Result<MailEvent, RecvError>),
    Groupware(Result<GroupwareEvent, RecvError>),
    Sieve(Result<AccountId, RecvError>),
}

struct Listener {
    events: Receiver<MailEvent>,
    groupware: Option<Receiver<GroupwareEvent>>,
    /// Counts this stream as a push connection of the account.
    _watch: Option<Watch>,
    /// Counts it as hearing the account's calendar alerts, if it asked for
    /// them.
    _alerts: Option<AlertListener>,
    sieve: Option<Listening>,
    store: Store,
    account: AccountId,
    jmap_id: JmapAccountId,
    /// For the quotas (RFC 9425 §6: Quota push), asked again after each
    /// mail change that may move them.
    inner: Weak<Inner>,
    /// The last Quota state this stream sent.
    quota: Option<State>,
}

impl Listener {
    async fn heard(&mut self) -> Heard {
        let sieve = async {
            match &mut self.sieve {
                Some(sieve) => sieve.changes.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = self.events.recv() => Heard::Mail(event),
            event = next_from(&mut self.groupware) => Heard::Groupware(event),
            event = sieve => Heard::Sieve(event),
        }
    }

    /// The next change or alert for this account, or `None` when the mail
    /// layer is gone.
    async fn next(&mut self) -> Option<PushEvent> {
        loop {
            let types: Vec<DataType> = match self.heard().await {
                Heard::Mail(Ok(MailEvent::StateChanged(changed)))
                    if changed.account == self.account =>
                {
                    changed.data_types
                }
                Heard::Groupware(Ok(GroupwareEvent::StateChanged(changed)))
                    if changed.account == self.account =>
                {
                    changed.data_types
                }
                Heard::Groupware(Ok(GroupwareEvent::CalendarAlert(due)))
                    if due.account == self.account =>
                {
                    if let Some(alert) =
                        alert_for(&self.store, self.account, &self.jmap_id, due).await
                    {
                        return Some(PushEvent::CalendarAlert(alert));
                    }
                    continue;
                }
                Heard::Sieve(Ok(account)) if account == self.account => {
                    vec![DataType::SieveScript]
                }
                // A slow listener missed some events: say everything may
                // have changed, the client reloads what it shows.
                Heard::Mail(Err(RecvError::Lagged(_))) => ALL.to_vec(),
                Heard::Groupware(Err(RecvError::Lagged(_))) => ALL_GROUPWARE.to_vec(),
                Heard::Sieve(Err(RecvError::Lagged(_))) => vec![DataType::SieveScript],
                Heard::Mail(Err(RecvError::Closed)) => return None,
                Heard::Groupware(Err(RecvError::Closed)) => {
                    self.groupware = None;
                    continue;
                }
                Heard::Sieve(Err(RecvError::Closed)) => {
                    self.sieve = None;
                    continue;
                }
                Heard::Mail(Ok(_)) | Heard::Groupware(Ok(_)) | Heard::Sieve(Ok(_)) => continue,
            };
            if let Some(change) = self.change(&types).await {
                return Some(PushEvent::State(change));
            }
        }
    }

    async fn change(&mut self, types: &[DataType]) -> Option<StateChange> {
        let mut change = match change_of(&self.store, self.account, &self.jmap_id, types).await {
            Ok(change) => change,
            Err(error) => {
                tracing::warn!(error = %error, "push: state unavailable");
                None
            }
        };
        // Quota (RFC 9425 §6): when its state moved.
        if crate::quota::moves_quota(types)
            && let Some(inner) = self.inner.upgrade()
            && let Some(state) = crate::quota::push_state(&inner, self.account).await
            && self.quota.as_ref() != Some(&state)
        {
            self.quota = Some(state.clone());
            change
                .get_or_insert_with(StateChange::default)
                .changed
                .entry(self.jmap_id.clone())
                .or_default()
                .insert(JmapDataType::Quota, state);
        }
        change
    }
}

/// The current states of `types` as a `StateChange` of the account (and
/// of its shared accounts, for groupware types); `None` if none has one.
async fn change_of(
    store: &Store,
    account: AccountId,
    jmap_id: &JmapAccountId,
    types: &[DataType],
) -> Result<Option<StateChange>, nebula_store::StoreError> {
    let mut states: BTreeMap<JmapDataType, State> = BTreeMap::new();
    for data_type in types {
        let Some(name) = jmap_data_type(*data_type) else {
            continue;
        };
        states.insert(name, ids::state(store.state(account, *data_type).await?));
    }
    if states.is_empty() {
        return Ok(None);
    }
    let shared = if states
        .keys()
        .any(|name| GROUPWARE_NAMES.contains(&name.as_str()))
    {
        crate::sharing::accounts::push_ids(store, account, jmap_id).await
    } else {
        Vec::new()
    };
    // An event source has no size limit.
    Ok(Some(state_change(jmap_id, states, &shared, None)))
}

/// Every state the push stream of the account can change, now: the base
/// of the event ids of `EventSource` (RFC 8620 §7.3).
pub(crate) async fn current(
    inner: &Inner,
    account: &AccountContext,
) -> Result<StateChange, nebula_http::BackendError> {
    let mut types = ALL.to_vec();
    if inner.groupware.is_some() {
        types.extend_from_slice(ALL_GROUPWARE);
    }
    if inner.sieve.is_some() {
        types.push(DataType::SieveScript);
    }
    match change_of(&inner.store, account.id, &account.jmap_id, &types).await {
        Ok(change) => {
            let mut change = change.unwrap_or_default();
            if let Some(state) = crate::quota::push_state(inner, account.id).await {
                change
                    .changed
                    .entry(account.jmap_id.clone())
                    .or_default()
                    .insert(JmapDataType::Quota, state);
            }
            Ok(change)
        }
        Err(error) => {
            tracing::warn!(error = %error, "push: states unavailable");
            Err(nebula_http::BackendError::Unavailable)
        }
    }
}

/// The push stream of one account; `alerts`: it asked for
/// `CalendarAlert` (only then is an alert sent for it).
pub(crate) fn stream(
    inner: &Arc<Inner>,
    account: &AccountContext,
    alerts: bool,
) -> BoxStream<'static, PushEvent> {
    let listener = Listener {
        events: inner.mail.subscribe(),
        groupware: inner.groupware.as_ref().map(Groupware::subscribe),
        _watch: inner.groupware.as_ref().map(|g| g.watch(account.id)),
        _alerts: inner
            .groupware
            .as_ref()
            .filter(|_| alerts)
            .map(|g| g.listen_alerts(account.id)),
        sieve: inner.sieve.as_ref().map(|s| s.listen(account.id)),
        store: inner.store.clone(),
        account: account.id,
        jmap_id: account.jmap_id.clone(),
        inner: Arc::downgrade(inner),
        quota: None,
    };
    stream::unfold(listener, |mut listener| async move {
        listener.next().await.map(|change| (change, listener))
    })
    .boxed()
}

#[cfg(test)]
mod tests {
    use nebula_store::{
        CalendarExtras, CollectionKind, CollectionProps, CollectionSharing, Components, DavHref,
        NewAccount, Parsed, ResourceIndex, ResourceWrite, Rights, UnixTime,
    };

    use super::*;

    fn href(path: &str) -> DavHref {
        DavHref::new(&format!("https://dav.example.com{path}")).unwrap()
    }

    fn calendar(path: &str, owner: Option<&str>) -> CollectionProps {
        CollectionProps {
            kind: CollectionKind::Calendar,
            href: href(path),
            name: Some("Calendar".to_owned()),
            color: None,
            description: None,
            sort_order: None,
            time_zone: None,
            default_alarms_datetime: None,
            default_alarms_date: None,
            rights: Some(Rights::READ),
            components: Some(Components::EVENT),
            vcard_versions: None,
            supports_sync: true,
            sharing: CollectionSharing {
                owner: owner.map(href),
                ..CollectionSharing::default()
            },
            extras: CalendarExtras::default(),
        }
    }

    fn states(names: &[&str]) -> BTreeMap<JmapDataType, State> {
        names
            .iter()
            .map(|name| (named(name).unwrap(), State::new("s9")))
            .collect()
    }

    /// The push of a change: groupware states in the shared accounts too,
    /// mail ones only in the own account; in a Web Push message only as
    /// many shared accounts as fit, never without the own one.
    #[test]
    fn state_changes_and_shared_accounts() {
        let own: JmapAccountId = "a1".parse().unwrap();
        let shared: Vec<JmapAccountId> = (0..60)
            .map(|n| format!("a1-r{}", 1_000_000_000_u64 + n).parse().unwrap())
            .collect();
        let mail = state_change(&own, states(&["Email"]), &shared, None);
        assert_eq!(mail.changed.keys().collect::<Vec<_>>(), [&own]);
        let all_groupware = [
            "Calendar",
            "CalendarEvent",
            "AddressBook",
            "ContactCard",
            "Email",
            "Mailbox",
        ];
        let everyone = state_change(&own, states(&all_groupware), &shared, None);
        assert_eq!(everyone.changed.len(), 61);
        assert_eq!(everyone.changed[&shared[0]].len(), 4);
        let limit = nebula_webpush::ece::MAX_PLAINTEXT;
        let fitted = state_change(&own, states(&all_groupware), &shared, Some(limit));
        assert!(fitted.changed.contains_key(&own));
        assert!(fitted.changed.len() > 1 && fitted.changed.len() < 61);
        assert!(serde_json::to_vec(&fitted).unwrap().len() <= limit);
        // Nothing fits: still the own account.
        let tiny = state_change(&own, states(&all_groupware), &shared, Some(10));
        assert_eq!(tiny.changed.keys().collect::<Vec<_>>(), [&own]);
    }

    /// The current states for event ids (RFC 8620 §7.3): every type
    /// asked for, in the own account; none asked, none.
    #[tokio::test]
    async fn current_states_of_every_type() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).await.unwrap();
        let account = store
            .get_or_create_account(
                NewAccount {
                    login: "alice",
                    email: "[EMAIL]",
                    wrapped_dek: b"k",
                },
                UnixTime::from_secs(1_000),
            )
            .await
            .unwrap()
            .id;
        let own: JmapAccountId = "a1".parse().unwrap();
        let mut types = ALL.to_vec();
        types.extend_from_slice(ALL_GROUPWARE);
        types.push(DataType::SieveScript);
        let change = change_of(&store, account, &own, &types)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(change.changed.keys().collect::<Vec<_>>(), [&own]);
        let states = &change.changed[&own];
        assert_eq!(states.len(), types.len());
        for data_type in &types {
            let name = jmap_data_type(*data_type).unwrap();
            let seq = store.state(account, *data_type).await.unwrap();
            assert_eq!(states[&name], ids::state(seq), "{name}");
        }
        assert_eq!(change_of(&store, account, &own, &[]).await.unwrap(), None);
    }

    /// An alert of an event in a calendar someone shares is in their
    /// account, in the event source and in Web Push alike.
    #[tokio::test]
    async fn alerts_of_shared_calendars_are_in_the_shared_account() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("test.db")).await.unwrap();
        let now = UnixTime::from_secs(1_000);
        let account = store
            .get_or_create_account(
                NewAccount {
                    login: "alice",
                    email: "alice@example.com",
                    wrapped_dek: b"k",
                },
                now,
            )
            .await
            .unwrap()
            .id;
        let (_, live) = store
            .apply_collections(
                account,
                vec![
                    calendar("/cal/alice/work/", None),
                    calendar("/cal/bob/team/", Some("/principals/bob/")),
                ],
                now,
            )
            .await
            .unwrap();
        let event = |path: &str| ResourceWrite {
            href: href(path),
            etag: Some("1".to_owned()),
            sealed: b"sealed".to_vec(),
            size: 6,
            parsed: Parsed::Valid(ResourceIndex {
                uid: Some(path.to_owned()),
                utc_start: Some(UnixTime::from_secs(2_000)),
                utc_end: None,
                recurring: false,
                sort_name: None,
            }),
        };
        let mut resources = Vec::new();
        for (collection, path) in [
            (&live[0], "/cal/alice/work/1.ics"),
            (&live[1], "/cal/bob/team/1.ics"),
        ] {
            store
                .apply_resources(account, collection.id, vec![event(path)], Vec::new(), now)
                .await
                .unwrap();
            resources.push(store.dav_resources(account, collection.id).await.unwrap()[0].id);
        }
        let own: JmapAccountId = "a1".parse().unwrap();
        let shared = crate::sharing::accounts::push_ids(&store, account, &own).await;
        assert_eq!(shared.len(), 1);
        let due = |resource| AlertDue {
            account,
            resource,
            uid: "u".to_owned(),
            alert_id: "k".to_owned(),
            recurrence: None,
        };
        let mine = alert_for(&store, account, &own, due(resources[0]))
            .await
            .unwrap();
        assert_eq!(serde_json::to_value(&mine).unwrap()["accountId"], "a1");
        let theirs = alert_for(&store, account, &own, due(resources[1]))
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&theirs).unwrap()["accountId"],
            shared[0].as_str()
        );
    }

    #[test]
    fn groupware_types_have_jmap_names() {
        for (data_type, name) in [
            (DataType::Calendar, "Calendar"),
            (DataType::CalendarEvent, "CalendarEvent"),
            (DataType::AddressBook, "AddressBook"),
            (DataType::ContactCard, "ContactCard"),
            (DataType::ParticipantIdentity, "ParticipantIdentity"),
            (DataType::SieveScript, "SieveScript"),
            (DataType::Email, "Email"),
        ] {
            assert_eq!(jmap_data_type(data_type).unwrap().as_str(), name);
        }
    }

    /// Stalwart's form (checked live): the series id, and the occurrence
    /// as a local date-time in the event's zone.
    #[test]
    fn alerts_as_push_objects() {
        let due = AlertDue {
            account: AccountId::from_raw(1).unwrap(),
            resource: nebula_store::ResourceId::from_raw(7).unwrap(),
            uid: "u1".to_owned(),
            alert_id: "k1".to_owned(),
            // 2026-10-27T17:11:44 as if UTC.
            recurrence: Some(RecurrenceKey::from_naive(1_793_121_104).unwrap()),
        };
        let jmap_id: JmapAccountId = "a1".parse().unwrap();
        let alert = calendar_alert(&jmap_id, due.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&alert).unwrap(),
            serde_json::json!({
                "@type": "CalendarAlert", "accountId": "a1", "calendarEventId": "v7",
                "uid": "u1", "recurrenceId": "2026-10-27T17:11:44", "alertId": "k1"
            })
        );
        let single = calendar_alert(
            &jmap_id,
            AlertDue {
                recurrence: None,
                ..due
            },
        )
        .unwrap();
        assert_eq!(single.recurrence_id(), None);
    }
}
