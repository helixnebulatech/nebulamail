// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `PushSubscription/get` and `PushSubscription/set` (RFC 8620 §7.2), as
//! Stalwart answers them: `url` and `keys` are write-only, `verificationCode`
//! must be the one sent in the `PushVerification`, `expires` is at most
//! 7 days away (later is shortened, missing is 7 days), `types` is a list
//! of type names or `null` (all), `emailPush` (Stalwart 1.0) is checked by
//! [`super::email_push`] and kept sealed with the URL and keys.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nebula_jmap_proto::object::{
    PushSubscription, PushSubscriptionCreate, PushSubscriptionGetArguments,
    PushSubscriptionGetResponse, PushSubscriptionPatch, PushSubscriptionProperty,
    PushSubscriptionSetArguments, PushSubscriptionSetResponse,
};
use nebula_jmap_proto::types::DataType;
use nebula_jmap_proto::{MethodResponse, SetError, SetErrorType, UtcDate};
use nebula_store::{NewPushSubscription, PushSubscriptionChange, PushSubscriptionRow, UnixTime};
use nebula_webpush::{ClientKeys, EndpointError, PushEndpoint};
use serde_json::{Value, json};

use super::email_push::EmailPush;
use super::{Command, Service, jmap_id, open, seal, store_id};
use crate::core::context::Ctx;
use crate::error::{CallError, store_set_error};

fn service<'a>(ctx: &'a Ctx<'_>) -> Option<&'a Service> {
    ctx.inner.push.as_ref()
}

fn type_names(row: &PushSubscriptionRow) -> Option<Vec<DataType>> {
    row.types.as_ref().map(|names| {
        names
            .iter()
            .filter_map(|name| name.parse::<DataType>().ok())
            .collect()
    })
}

fn date(at: UnixTime) -> Option<UtcDate> {
    UtcDate::from_unix_timestamp(at.secs()).ok()
}

/// `PushSubscription/get`.
pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: PushSubscriptionGetArguments,
) -> Result<MethodResponse, CallError> {
    let now = Ctx::now()?;
    let rows = if service(ctx).is_some() {
        ctx.store()
            .push_subscriptions(ctx.id(), ctx.session, now)
            .await?
    } else {
        Vec::new()
    };
    let wanted = |property: PushSubscriptionProperty| {
        args.properties
            .as_ref()
            .is_none_or(|list| list.contains(&property))
    };
    let mut response = PushSubscriptionGetResponse {
        list: Vec::new(),
        not_found: Vec::new(),
    };
    let selected: Vec<&PushSubscriptionRow> = match &args.ids {
        None => rows.iter().collect(),
        Some(ids) => {
            let mut found = Vec::new();
            for id in ids {
                match store_id(id).and_then(|id| rows.iter().find(|row| row.id == id)) {
                    Some(row) => found.push(row),
                    None => response.not_found.push(id.clone()),
                }
            }
            found
        }
    };
    let key = service(ctx).and_then(|service| service.key(ctx.id()));
    for row in selected {
        let email_push = wanted(PushSubscriptionProperty::EmailPush).then(|| {
            key.as_ref()
                .and_then(|key| open(key, row.id, &row.sealed))
                .and_then(|opened| opened.email_push)
        });
        response.list.push(PushSubscription {
            id: Some(jmap_id(row.id)?),
            device_client_id: wanted(PushSubscriptionProperty::DeviceClientId)
                .then(|| row.device_client_id.clone()),
            verification_code: wanted(PushSubscriptionProperty::VerificationCode)
                .then(|| row.verified.then(|| row.verification_code.clone())),
            expires: wanted(PushSubscriptionProperty::Expires).then(|| date(row.expires)),
            types: wanted(PushSubscriptionProperty::Types).then(|| type_names(row)),
            email_push,
        });
    }
    Ok(MethodResponse::PushSubscriptionGet(response))
}

/// A random verification code: 24 bytes as base64url (32 characters).
fn verification_code() -> Result<String, SetError> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// `expires` within the allowed life (`None`: the longest).
fn expiry(
    requested: Option<UtcDate>,
    now: UnixTime,
    service: &Service,
) -> Result<UnixTime, SetError> {
    let max = now.saturating_add(service.settings().lifetime);
    match requested {
        None => Ok(max),
        Some(date) if date.unix_timestamp() <= now.secs() => {
            Err(SetError::invalid_properties(["expires"])
                .with_description("expires is in the past"))
        }
        Some(date) => Ok(UnixTime::from_secs(date.unix_timestamp().min(max.secs()))),
    }
}

/// Type names to store; only well-known types (an unknown name would
/// never be pushed).
fn checked_types(types: Option<Vec<DataType>>) -> Result<Option<Vec<String>>, SetError> {
    let Some(types) = types else {
        return Ok(None);
    };
    let mut names = BTreeSet::new();
    for data_type in types {
        if matches!(data_type, DataType::Other(_)) {
            return Err(
                SetError::invalid_properties(["types"]).with_description("unknown data type")
            );
        }
        names.insert(data_type.as_str().to_owned());
    }
    Ok(Some(names.into_iter().collect()))
}

fn endpoint_error(error: EndpointError) -> SetError {
    let description = match error {
        EndpointError::HostNotAllowed => {
            "the push URL is not one of the push services this server allows"
        }
        EndpointError::TooLong => "the push URL is too long",
        EndpointError::Blocked(_) => "the push URL must be https on port 443 to a public address",
        _ => "the push URL is not allowed",
    };
    SetError::invalid_properties(["url"]).with_description(description)
}

async fn create(
    ctx: &Ctx<'_>,
    service: &Service,
    object: &serde_json::Map<String, Value>,
    now: UnixTime,
) -> Result<Value, SetError> {
    let create = PushSubscriptionCreate::parse(object)?;
    let endpoint =
        PushEndpoint::parse(&create.url, &service.settings().policy).map_err(endpoint_error)?;
    // Unencrypted pushes would show the push service what changed.
    let keys = create.keys.as_ref().ok_or_else(|| {
        SetError::invalid_properties(["keys"]).with_description("keys are required")
    })?;
    let keys = ClientKeys::parse(&keys.p256dh, &keys.auth).map_err(|_| {
        SetError::invalid_properties(["keys"])
            .with_description("p256dh must be a P-256 public key and auth 16 bytes")
    })?;
    let expires = expiry(create.expires, now, service)?;
    let types = checked_types(create.types)?;
    let jmap_account = ctx.jmap_id();
    let email_push = EmailPush::from_request(&jmap_account, create.email_push)?
        .map(|config| config.to_json(&jmap_account));
    let key = service.key(ctx.id()).ok_or_else(|| {
        SetError::new(SetErrorType::Forbidden).with_description("the account is not running")
    })?;
    let url = endpoint.as_str().to_owned();
    let target = url.clone();
    let opener = Arc::clone(&key);
    let new = NewPushSubscription {
        device_client_id: create.device_client_id,
        types,
        expires,
        verification_code: verification_code()?,
    };
    let created = ctx
        .store()
        .create_push_subscription(
            ctx.id(),
            ctx.session,
            new,
            service.settings().max_subscriptions,
            now,
            move |id| seal(&key, id, &url, &keys, email_push.as_ref()),
            // Another session's subscription of this device is replaced only
            // if it pushes to the same URL (see `replace_device`).
            move |id, sealed| open(&opener, id, sealed).is_some_and(|old| old.url == target),
        )
        .await
        .map_err(|error| store_set_error(&error))?
        .ok_or_else(|| {
            SetError::new(SetErrorType::OverQuota)
                .with_description("too many push subscriptions; delete some first")
        })?;
    if created.replaced > 0 {
        tracing::debug!(
            account = ctx.id().get(),
            replaced = created.replaced,
            "web push: replaced this device's subscriptions of other sessions"
        );
    }
    // Also drops, from the delivery task, the ones it replaced.
    service.tell(ctx.id(), Command::Reload);
    let id = jmap_id(created.id).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    // Never past the end of the session: say when it really ends.
    Ok(json!({
        "id": id,
        "expires": date(created.expires),
    }))
}

async fn update(
    ctx: &Ctx<'_>,
    service: &Service,
    id: nebula_store::PushSubscriptionId,
    patch: &nebula_jmap_proto::method::PatchObject,
    now: UnixTime,
) -> Result<Option<Value>, SetError> {
    let patch = PushSubscriptionPatch::parse(patch)?;
    let rows = ctx
        .store()
        .push_subscriptions(ctx.id(), ctx.session, now)
        .await
        .map_err(|error| store_set_error(&error))?;
    let row = rows
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(SetError::not_found)?;
    let mut change = PushSubscriptionChange::default();
    if let Some(code) = &patch.verification_code {
        // Constant time: the code is a (short-lived) secret.
        let matches = code.len() == row.verification_code.len()
            && code
                .bytes()
                .zip(row.verification_code.bytes())
                .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        if !matches {
            return Err(SetError::invalid_properties(["verificationCode"])
                .with_description("the verification code does not match"));
        }
        change.verified = true;
    }
    let changed_expiry = patch.expires.is_some();
    if let Some(expires) = patch.expires {
        change.expires = Some(expiry(expires, now, service)?);
    }
    if let Some(types) = patch.types {
        change.types = Some(checked_types(types)?);
    }
    if let Some(requested) = patch.email_push {
        let jmap_account = ctx.jmap_id();
        let email_push = EmailPush::from_request(&jmap_account, requested)?
            .map(|config| config.to_json(&jmap_account));
        let key = service.key(ctx.id()).ok_or_else(|| {
            SetError::new(SetErrorType::Forbidden).with_description("the account is not running")
        })?;
        let opened = open(&key, id, &row.sealed).ok_or_else(|| {
            SetError::new(SetErrorType::Forbidden).with_description("the subscription is damaged")
        })?;
        change.sealed = Some(
            seal(&key, id, &opened.url, &opened.keys, email_push.as_ref())
                .map_err(|error| store_set_error(&error))?,
        );
    }
    let expires = ctx
        .store()
        .update_push_subscription(ctx.id(), ctx.session, id, change, now)
        .await
        .map_err(|error| store_set_error(&error))?
        .ok_or_else(SetError::not_found)?;
    service.tell(ctx.id(), Command::Reload);
    // The server may shorten `expires` (to the end of the session, too):
    // say what it is now.
    Ok(changed_expiry.then(|| json!({"expires": date(expires)})))
}

/// `PushSubscription/set`.
pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: PushSubscriptionSetArguments,
) -> Result<MethodResponse, CallError> {
    let now = Ctx::now()?;
    let mut response = PushSubscriptionSetResponse::new();
    let Some(service) = ctx.inner.push.as_ref() else {
        // No `[push]` in the configuration: nothing can be subscribed.
        for creation_id in args.create.keys() {
            response.not_created.insert(
                creation_id.clone(),
                SetError::new(SetErrorType::Forbidden)
                    .with_description("Web Push is not enabled on this server"),
            );
        }
        for id in args.update.keys() {
            response
                .not_updated
                .insert(id.clone(), SetError::not_found());
        }
        for id in args.destroy {
            response.not_destroyed.insert(id, SetError::not_found());
        }
        return Ok(MethodResponse::PushSubscriptionSet(response));
    };
    let destroying: BTreeSet<_> = args
        .destroy
        .iter()
        .filter_map(|id| ctx.resolve(id))
        .collect();
    for (creation_id, object) in &args.create {
        match create(ctx, service, object, now).await {
            Ok(created) => {
                if let Some(id) = created.get("id").and_then(Value::as_str)
                    && let Ok(id) = id.parse::<nebula_jmap_proto::Id>()
                {
                    ctx.created.insert(creation_id.clone(), id);
                }
                response.created.insert(creation_id.clone(), created);
            }
            Err(error) => {
                response.not_created.insert(creation_id.clone(), error);
            }
        }
    }
    for (reference, patch) in &args.update {
        let Some(jmap) = ctx.resolve(reference) else {
            response
                .not_updated
                .insert(reference.clone(), SetError::not_found());
            continue;
        };
        if destroying.contains(&jmap) {
            response
                .not_updated
                .insert(reference.clone(), SetError::new(SetErrorType::WillDestroy));
            continue;
        }
        let Some(id) = store_id(&jmap) else {
            response
                .not_updated
                .insert(reference.clone(), SetError::not_found());
            continue;
        };
        match update(ctx, service, id, patch, now).await {
            Ok(changed) => {
                response.updated.insert(jmap, changed);
            }
            Err(error) => {
                response.not_updated.insert(reference.clone(), error);
            }
        }
    }
    for reference in args.destroy {
        let id = ctx.resolve(&reference);
        let Some((jmap, id)) = id.and_then(|jmap| store_id(&jmap).map(|id| (jmap, id))) else {
            response
                .not_destroyed
                .insert(reference, SetError::not_found());
            continue;
        };
        if ctx
            .store()
            .delete_push_subscription(ctx.id(), ctx.session, id)
            .await?
        {
            response.destroyed.push(jmap);
        } else {
            response
                .not_destroyed
                .insert(reference, SetError::not_found());
        }
    }
    if !response.destroyed.is_empty() {
        service.tell(ctx.id(), Command::Reload);
    }
    Ok(MethodResponse::PushSubscriptionSet(response))
}
