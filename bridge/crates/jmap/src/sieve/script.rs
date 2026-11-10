// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `SieveScript/get`, `/changes`, `/query`, `/set` and `/validate`
//! (RFC 9661) on ManageSieve.
//!
//! `/set` (RFC 9661 §2.4, §4): the name `vacation` is
//! reserved (`forbidden`), the active script can't be destroyed
//! (`sieveIsActive`), the activation arguments only run if nothing failed
//! (deactivation first; an invalid or unknown id to activate is ignored)
//! and every script whose `isActive` flipped is reported. Creates and
//! content updates are `PUTSCRIPT` (the server checks the script:
//! `invalidSieve` with its text); renames are `RENAMESCRIPT` and keep the
//! id.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use nebula_jmap_proto::method::{
    ChangesArguments, Filter, GetArguments, GetResponse, Operator, QueryResponse, SetResponse,
};
use nebula_jmap_proto::object::{
    SieveScript, SieveScriptCreate, SieveScriptFilterCondition, SieveScriptPatch,
    SieveScriptProperty, SieveScriptQueryArguments, SieveScriptSetArguments, SieveScriptSort,
    SieveScriptValidateArguments, SieveScriptValidateResponse,
};
use nebula_jmap_proto::{
    BlobId, CreationId, IdOrRef, MethodErrorType, MethodResponse, SetError, SetErrorType,
    SieveScriptId as JmapScriptId, UnsignedInt,
};
use nebula_managesieve::proto::{ResponseCode, ScriptName};
use nebula_managesieve::{CheckOutcome, Error as SieveError};
use nebula_store::DataType;

use super::{
    Script, Session, Synced, VACATION_NAME, WriteTurn, blob_id, blob_id_of, content_hash,
    script_id, vacation_writable,
};
use crate::collation::CheckedSort;
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

fn to_jmap(
    script: &Script,
    properties: Option<&[SieveScriptProperty]>,
) -> Result<SieveScript, Bug> {
    let wants = |p| properties.is_none_or(|list| list.contains(&p));
    Ok(SieveScript {
        id: Some(script_id(script.row.id)?),
        name: wants(SieveScriptProperty::Name).then(|| script.row.name.clone()),
        blob_id: wants(SieveScriptProperty::BlobId)
            .then(|| blob_id(&script.row))
            .transpose()?,
        is_active: wants(SieveScriptProperty::IsActive).then_some(script.row.active),
    })
}

/// Opens a session and syncs (every method starts like this).
async fn synced(
    ctx: &Ctx<'_>,
    account: &nebula_jmap_proto::AccountId,
) -> Result<(Session, Synced), CallError> {
    ctx.check_account(account)?;
    let mut session = Session::open_ctx(ctx).await?;
    let synced = session.sync(ctx.inner).await?;
    Ok((session, synced))
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<JmapScriptId, SieveScriptProperty>,
) -> Result<MethodResponse, CallError> {
    let (session, synced) = synced(ctx, &args.account_id).await?;
    session.close().await;
    let properties = args.properties.as_deref();
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &args.ids {
        None => {
            for script in &synced.scripts {
                list.push(to_jmap(script, properties)?);
            }
        }
        Some(wanted) => {
            for id in wanted.iter().filter_map(|id| ctx.resolve(id)) {
                match super::store_script(&id).and_then(|row| synced.by_id(row)) {
                    Some(script) => list.push(to_jmap(script, properties)?),
                    None => not_found.push(id),
                }
            }
        }
    }
    Ok(MethodResponse::SieveScriptGet(GetResponse {
        account_id: ctx.jmap_id(),
        state: synced.state,
        list,
        not_found,
    }))
}

pub(crate) async fn changes(
    ctx: &Ctx<'_>,
    args: ChangesArguments,
) -> Result<MethodResponse, CallError> {
    // Bring the log up to date first: changes made by other clients.
    let (session, _) = synced(ctx, &args.account_id).await?;
    session.close().await;
    let changes = crate::changes::changes(ctx, DataType::SieveScript, args, |row| {
        nebula_store::SieveScriptId::from_raw(row)
            .ok_or(Bug("script id"))
            .and_then(script_id)
    })
    .await?;
    Ok(MethodResponse::SieveScriptChanges(changes))
}

fn condition_matches(script: &Script, condition: &SieveScriptFilterCondition) -> bool {
    condition.name.as_ref().is_none_or(|name| {
        script
            .row
            .name
            .to_lowercase()
            .contains(&name.to_lowercase())
    }) && condition
        .is_active
        .is_none_or(|active| active == script.row.active)
}

fn matches(script: &Script, filter: &Filter<SieveScriptFilterCondition>) -> bool {
    match filter {
        Filter::Condition(condition) => condition_matches(script, condition),
        Filter::Operator(operator) => {
            let mut results = operator.conditions.iter().map(|f| matches(script, f));
            match operator.operator {
                Operator::And => results.all(|r| r),
                Operator::Or => results.any(|r| r),
                Operator::Not => !results.any(|r| r),
                _ => false,
            }
        }
        _ => false,
    }
}

/// The order of `SieveScript/query`: `name` with the comparator's
/// collation (RFC 8620 §5.5; case-insensitive by default), `isActive`
/// false first; then by id.
fn compare(a: &Script, b: &Script, sort: &CheckedSort<'_, SieveScriptSort>) -> Ordering {
    sort.compare(a, b, |property, collate, a, b| match property {
        SieveScriptSort::Name => collate.compare(&a.row.name, &b.row.name),
        SieveScriptSort::IsActive => a.row.active.cmp(&b.row.active),
        _ => Ordering::Equal,
    })
    .then_with(|| a.row.id.cmp(&b.row.id))
}

pub(crate) async fn query(
    ctx: &Ctx<'_>,
    query: SieveScriptQueryArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&query.account_id)?;
    let sort = CheckedSort::new(&query.sort)?;
    let (session, synced) = synced(ctx, &query.account_id).await?;
    session.close().await;
    let mut found: Vec<&Script> = synced
        .scripts
        .iter()
        .filter(|s| query.filter.as_ref().is_none_or(|f| matches(s, f)))
        .collect();
    found.sort_by(|a, b| compare(a, b, &sort));
    let indices: Vec<usize> = (0..found.len()).collect();
    let anchor = match &query.anchor {
        Some(anchor) => Some(
            found
                .iter()
                .position(|s| script_id(s.row.id).is_ok_and(|id| id == *anchor))
                .ok_or(CallError::from(MethodErrorType::AnchorNotFound))?,
        ),
        None => None,
    };
    let page = crate::paging::page(
        &indices,
        query.position.get(),
        anchor.map(|a| (a, query.anchor_offset.get())),
        query.limit.map(UnsignedInt::get),
    )?;
    let ids = page
        .ids
        .into_iter()
        .filter_map(|index| found.get(index))
        .map(|s| script_id(s.row.id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MethodResponse::SieveScriptQuery(QueryResponse {
        account_id: ctx.jmap_id(),
        query_state: synced.state,
        can_calculate_changes: false,
        position: UnsignedInt::saturating(page.position),
        ids,
        total: query
            .calculate_total
            .then(|| UnsignedInt::try_from(found.len()).unwrap_or(UnsignedInt::MAX)),
        limit: None,
    }))
}

/// The bytes of a blob: a script of this sync (`bz…`, read from the
/// server; not found if it changed since) or an upload.
async fn blob(
    ctx: &Ctx<'_>,
    session: &mut Session,
    synced: &Synced,
    id: &BlobId,
) -> Result<Vec<u8>, Fail> {
    let found = if let Some((script, hash)) = super::parse_blob(id.as_str()) {
        match synced.by_id(script).filter(|s| s.row.content_hash == hash) {
            Some(script) => session.content(script).await?,
            None => None,
        }
    } else {
        let mut lease = crate::email::build::lease(ctx)?;
        crate::email::build::blob_bytes(ctx, id, &mut lease)
            .await?
            .map(|bytes| bytes.to_vec())
    };
    let content = found.ok_or_else(|| SetError::blob_not_found(vec![id.clone()]))?;
    let max = ctx
        .inner
        .sieve
        .as_ref()
        .map_or(usize::MAX, |service| service.settings.max_script_size);
    if content.len() > max {
        return Err(SetError::new(SetErrorType::TooLarge)
            .with_description(format!("scripts can be up to {max} bytes"))
            .into());
    }
    Ok(content)
}

/// Whether one more script fits next to `count` existing ones under `max`
/// (`maxNumberScripts`).
///
/// # Errors
/// `overQuota` when it doesn't.
pub(super) fn check_count(count: usize, max: Option<usize>) -> Result<(), SetError> {
    match max {
        Some(max) if count >= max => Err(SetError::new(SetErrorType::OverQuota)
            .with_description(format!("at most {max} Sieve scripts"))),
        _ => Ok(()),
    }
}

/// A name for a new or renamed script.
fn checked_name(name: &str) -> Result<ScriptName, SetError> {
    if name.eq_ignore_ascii_case(VACATION_NAME) {
        return Err(SetError::new(SetErrorType::Forbidden).with_description(
            "The 'vacation' name is reserved for the vacation response, please use a different name.",
        ));
    }
    ScriptName::new(name).map_err(|error| {
        SetError::invalid_properties(vec!["name".to_owned()]).with_description(error.to_string())
    })
}

fn random_name() -> Result<String, SetError> {
    let mut bytes = [0_u8; 6];
    getrandom::fill(&mut bytes).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    Ok(bytes.iter().fold(String::from("script-"), |mut out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
        out
    }))
}

/// Why one object of `/set` failed: its own `SetError`, or something
/// that fails the whole call (the connection broke).
pub(crate) enum Fail {
    Set(SetError),
    Call(CallError),
}

impl From<SetError> for Fail {
    fn from(error: SetError) -> Self {
        Self::Set(error)
    }
}

impl From<Bug> for Fail {
    fn from(error: Bug) -> Self {
        Self::Call(error.into())
    }
}

impl From<CallError> for Fail {
    fn from(error: CallError) -> Self {
        Self::Call(error)
    }
}

/// A refused command: the script (or the quota) is the problem, so a
/// `SetError`; anything else fails the call.
///
/// `ALREADYEXISTS` is not `alreadyExists` here: that `SetError` MUST carry
/// `existingId` (RFC 8620 §5.3, RFC 9661 §2.4), which only
/// [`Run::exists`] can look up; the commands that can answer it (renames)
/// go there first.
pub(crate) fn refused(error: SieveError) -> Fail {
    match error {
        SieveError::Refused(refusal) => Fail::Set(match &refusal.code {
            Some(code) if code.is_quota() => {
                SetError::new(SetErrorType::OverQuota).with_description(refusal.text)
            }
            Some(ResponseCode::TryLater) => SetError::new(SetErrorType::Forbidden)
                .with_description("the Sieve server is busy; try again later"),
            Some(ResponseCode::Nonexistent) => SetError::not_found(),
            Some(ResponseCode::AlreadyExists) => {
                SetError::new(SetErrorType::Forbidden).with_description(refusal.text)
            }
            Some(ResponseCode::Active) => {
                SetError::new(SetErrorType::SieveIsActive).with_description(refusal.text)
            }
            _ => SetError::new(SetErrorType::InvalidSieve).with_description(refusal.text),
        }),
        other => Fail::Call(CallError::Sieve(other)),
    }
}

/// `alreadyExists` with the id of the script called `name`, or, if no
/// script has that name any more (another client removed it after the
/// server refused), `forbidden` asking to try again: `alreadyExists` MUST
/// carry `existingId` (RFC 8620 §5.3, RFC 9661 §2.4), so it is never sent
/// without one.
fn exists_error(
    name: &str,
    existing: Option<nebula_store::SieveScriptId>,
) -> Result<SetError, Bug> {
    Ok(match existing {
        Some(id) => SetError::already_exists(
            script_id(id)?
                .as_str()
                .parse()
                .map_err(|_| Bug("script id"))?,
        )
        .with_description(format!("A sieve script with name '{name}' already exists.")),
        None => SetError::new(SetErrorType::Forbidden).with_description(format!(
            "A sieve script named '{name}' was being changed by another client; try again."
        )),
    })
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: SieveScriptSetArguments,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.set.account_id)?;
    let mut session = Session::open_ctx(ctx).await?;
    // One write at a time per account, and no sync stored in between.
    let turn = session.write_turn().await;
    let result = match session.sync_turn(ctx.inner, &turn).await {
        Ok(synced) => set_inner(ctx, &mut session, &turn, synced, args).await,
        Err(error) => Err(error),
    };
    drop(turn);
    session.close().await;
    result
}

/// State while a `/set` runs.
struct Run<'s> {
    session: &'s mut Session,
    turn: &'s WriteTurn,
    synced: Synced,
    /// Current name of each script (renames and creates applied).
    names: Vec<String>,
    /// Scripts renamed in this call → their new name.
    renamed: std::collections::HashMap<nebula_store::SieveScriptId, ScriptName>,
}

impl Run<'_> {
    /// The `alreadyExists` for `name`: a script of the first sync that has
    /// that name now, else (created or renamed in this call, or by another
    /// client) whatever a new listing says.
    async fn exists(&mut self, ctx: &Ctx<'_>, name: &str) -> Result<SetError, Fail> {
        let known = self
            .synced
            .scripts
            .iter()
            .map(|s| s.row.id)
            .find(|id| self.current_name(*id).is_some_and(|n| n.as_str() == name));
        let existing = match known {
            Some(id) => Some(id),
            None => self
                .session
                .sync_turn(ctx.inner, self.turn)
                .await?
                .by_name(name)
                .map(|s| s.row.id),
        };
        Ok(exists_error(name, existing)?)
    }

    async fn create(
        &mut self,
        ctx: &Ctx<'_>,
        object: serde_json::Map<String, serde_json::Value>,
    ) -> Result<ScriptName, Fail> {
        let create = SieveScriptCreate::parse(object)?;
        let name = match create.name {
            Some(name) => name,
            None => random_name()?,
        };
        let name = checked_name(&name)?;
        if self.names.iter().any(|n| n == name.as_str()) {
            return Err(self.exists(ctx, name.as_str()).await?.into());
        }
        let max = ctx
            .inner
            .sieve
            .as_ref()
            .and_then(|service| service.settings.max_scripts);
        check_count(self.names.len(), max)?;
        let content = blob(ctx, self.session, &self.synced, &create.blob_id).await?;
        self.session
            .client
            .put_script(&name, &content)
            .await
            .map_err(refused)?;
        self.names.push(name.as_str().to_owned());
        Ok(name)
    }

    /// Returns the new blob id if the content changed.
    async fn update(
        &mut self,
        ctx: &Ctx<'_>,
        id: nebula_store::SieveScriptId,
        patch: &nebula_jmap_proto::method::PatchObject,
    ) -> Result<Option<BlobId>, Fail> {
        let Some(script) = self.synced.by_id(id) else {
            return Err(SetError::not_found().into());
        };
        if script.row.name.eq_ignore_ascii_case(VACATION_NAME) {
            return Err(SetError::new(SetErrorType::Forbidden)
                .with_description(
                    "The vacation response's script can only be changed with VacationResponse/set.",
                )
                .into());
        }
        let current = self.current_name(id).ok_or_else(SetError::not_found)?;
        let patch = SieveScriptPatch::parse(patch)?;
        let new_name = match &patch.name {
            Some(name) if name != current.as_str() => {
                let name = checked_name(name)?;
                if self.names.iter().any(|n| n == name.as_str()) {
                    return Err(self.exists(ctx, name.as_str()).await?.into());
                }
                Some(name)
            }
            _ => None,
        };
        let mut new_blob = None;
        if let Some(blob_id) = &patch.blob_id {
            let content = blob(ctx, self.session, &self.synced, blob_id).await?;
            let hash = content_hash(&content);
            if hash != script.row.content_hash {
                self.session
                    .client
                    .put_script(&current, &content)
                    .await
                    .map_err(refused)?;
                new_blob = Some(blob_id_of(id, &hash)?);
            }
        }
        if let Some(name) = new_name {
            if let Err(error) = self.session.client.rename_script(&current, &name).await {
                // Another client took the name since the sync.
                if matches!(error.code(), Some(ResponseCode::AlreadyExists)) {
                    return Err(self.exists(ctx, name.as_str()).await?.into());
                }
                return Err(refused(error));
            }
            ctx.store()
                .rename_sieve_script(ctx.id(), id, name.as_str().to_owned())
                .await
                .map_err(CallError::from)?;
            // Already in the store: the sync at the end sees no change.
            if let Some(service) = &ctx.inner.sieve {
                service.notify(ctx.id());
            }
            self.names.retain(|n| n != current.as_str());
            self.names.push(name.as_str().to_owned());
            self.renamed.insert(id, name);
        }
        Ok(new_blob)
    }

    /// The name script `id` has now (after renames in this call), if it
    /// still exists.
    fn current_name(&self, id: nebula_store::SieveScriptId) -> Option<ScriptName> {
        if let Some(name) = self.renamed.get(&id) {
            return Some(name.clone());
        }
        let script = self.synced.by_id(id)?;
        self.names
            .iter()
            .any(|n| n == script.name.as_str())
            .then(|| script.name.clone())
    }

    async fn destroy(&mut self, id: nebula_store::SieveScriptId) -> Result<(), Fail> {
        let Some(script) = self.synced.by_id(id) else {
            return Err(SetError::not_found().into());
        };
        // RFC 9661 §4: with the vacation response kept as this script, it
        // "MUST NOT" be destroyed by SieveScript/set (`forbidden`). Only
        // the exact name: ManageSieve names are case-sensitive (RFC 5804
        // §1.6) and `vacation.rs` writes exactly this one.
        if script.row.name == VACATION_NAME && vacation_writable(self.session.client.capabilities())
        {
            return Err(SetError::new(SetErrorType::Forbidden)
                .with_description(
                    "The vacation response's script can only be changed with VacationResponse/set.",
                )
                .into());
        }
        if script.row.active {
            return Err(SetError::new(SetErrorType::SieveIsActive)
                .with_description("Deactivate Sieve script before deletion.")
                .into());
        }
        let name = self.current_name(id).ok_or_else(SetError::not_found)?;
        self.session
            .client
            .delete_script(&name)
            .await
            .map_err(refused)?;
        self.names.retain(|n| n != name.as_str());
        Ok(())
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "create, update, destroy and activate in order"
)]
async fn set_inner(
    ctx: &mut Ctx<'_>,
    session: &mut Session,
    turn: &WriteTurn,
    synced: Synced,
    args: SieveScriptSetArguments,
) -> Result<MethodResponse, CallError> {
    let old_state = synced.state.clone();
    if let Some(expected) = &args.set.if_in_state
        && *expected != old_state
    {
        return Err(MethodErrorType::StateMismatch.into());
    }
    let mut response: SetResponse<SieveScript, JmapScriptId> =
        SetResponse::new(ctx.jmap_id(), Some(old_state.clone()), old_state);
    let names = synced.scripts.iter().map(|s| s.row.name.clone()).collect();
    let was_active: Vec<nebula_store::SieveScriptId> = synced
        .scripts
        .iter()
        .filter(|s| s.row.active)
        .map(|s| s.row.id)
        .collect();
    let mut run = Run {
        session,
        turn,
        synced,
        names,
        renamed: std::collections::HashMap::new(),
    };
    let mut created_names: BTreeMap<CreationId, ScriptName> = BTreeMap::new();

    for (creation_id, object) in args.set.create {
        match run.create(ctx, object).await {
            Ok(name) => {
                created_names.insert(creation_id, name);
            }
            Err(Fail::Set(error)) => {
                response.not_created.insert(creation_id, error);
            }
            Err(Fail::Call(error)) => return Err(error),
        }
    }
    for (id, patch) in &args.set.update {
        let Some(row) = ctx.resolve(id).and_then(|id| super::store_script(&id)) else {
            response
                .not_updated
                .insert(id.clone(), SetError::not_found());
            continue;
        };
        match run.update(ctx, row, patch).await {
            Ok(new_blob) => {
                let changed = new_blob.map(|blob_id| SieveScript {
                    blob_id: Some(blob_id),
                    ..SieveScript::default()
                });
                response.updated.insert(script_id(row)?, changed);
            }
            Err(Fail::Set(error)) => {
                response.not_updated.insert(id.clone(), error);
            }
            Err(Fail::Call(error)) => return Err(error),
        }
    }
    for id in &args.set.destroy {
        let Some(row) = ctx.resolve(id).and_then(|id| super::store_script(&id)) else {
            response
                .not_destroyed
                .insert(id.clone(), SetError::not_found());
            continue;
        };
        match run.destroy(row).await {
            Ok(()) => response.destroyed.push(script_id(row)?),
            Err(Fail::Set(error)) => {
                response.not_destroyed.insert(id.clone(), error);
            }
            Err(Fail::Call(error)) => return Err(error),
        }
    }

    // Activation only after a fully successful call (RFC 9661 §2.4).
    let all_ok = response.not_created.is_empty()
        && response.not_updated.is_empty()
        && response.not_destroyed.is_empty();
    if all_ok {
        // An invalid or nonexistent id "MUST be ignored" (§2.4), including
        // a `#creationId` whose create isn't in this call.
        let target = match &args.on_success_activate_script {
            Some(IdOrRef::Ref(creation_id)) => created_names.get(creation_id).cloned(),
            Some(target) => ctx
                .resolve(target)
                .and_then(|id| super::store_script(&id))
                .and_then(|row| run.current_name(row)),
            None => None,
        };
        // Deactivation is "processed first" (§2.4): SETACTIVE of the
        // target replaces the active script in one step (RFC 5804 §2.8),
        // so it is only sent on its own without a target.
        match target {
            Some(name) => run
                .session
                .client
                .set_active(Some(&name))
                .await
                .map_err(CallError::Sieve)?,
            None if args.on_success_deactivate_script => run
                .session
                .client
                .set_active(None)
                .await
                .map_err(CallError::Sieve)?,
            None => {}
        }
    }

    let after = run.session.sync_turn(ctx.inner, turn).await?;
    let mut created_ids = Vec::new();
    for (creation_id, name) in created_names {
        let Some(script) = after.by_name(name.as_str()) else {
            continue;
        };
        let id = script_id(script.row.id)?;
        created_ids.push(script.row.id);
        ctx.created.insert(creation_id.clone(), id.clone());
        response.created.insert(
            creation_id,
            SieveScript {
                id: Some(id),
                name: Some(script.row.name.clone()),
                blob_id: Some(blob_id(&script.row)?),
                is_active: Some(script.row.active),
            },
        );
    }
    // Every activated or deactivated script is in `updated` with its
    // `isActive` (§2.4 "MUST be reported"); created ones are in `created`.
    for script in &after.scripts {
        if script.row.active == was_active.contains(&script.row.id)
            || created_ids.contains(&script.row.id)
        {
            continue;
        }
        let entry = response
            .updated
            .entry(script_id(script.row.id)?)
            .or_insert(None);
        entry.get_or_insert_with(SieveScript::default).is_active = Some(script.row.active);
    }
    response.new_state = after.state;
    Ok(MethodResponse::SieveScriptSet(response))
}

pub(crate) async fn validate(
    ctx: &Ctx<'_>,
    args: SieveScriptValidateArguments,
) -> Result<MethodResponse, CallError> {
    let (mut session, synced) = synced(ctx, &args.account_id).await?;
    let outcome = match blob(ctx, &mut session, &synced, &args.blob_id).await {
        Err(Fail::Set(error)) => Ok(Some(error)),
        Err(Fail::Call(error)) => Err(error),
        Ok(content) => match session.client.check_script(&content).await {
            Ok(CheckOutcome::Valid { .. }) => Ok(None),
            Ok(CheckOutcome::Invalid(text)) => Ok(Some(
                SetError::new(SetErrorType::InvalidSieve).with_description(text),
            )),
            Err(error) => match refused(error) {
                Fail::Set(error) => Ok(Some(error)),
                Fail::Call(error) => Err(error),
            },
        },
    };
    session.close().await;
    Ok(MethodResponse::SieveScriptValidate(
        SieveScriptValidateResponse {
            account_id: ctx.jmap_id(),
            error: outcome?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use nebula_jmap_proto::method::Comparator;
    use nebula_store::SieveScriptRow;
    use serde_json::json;

    use super::*;

    fn script(id: i64, name: &str, active: bool) -> Script {
        Script {
            row: SieveScriptRow {
                id: nebula_store::SieveScriptId::from_raw(id).unwrap(),
                name: name.to_owned(),
                active,
                content_hash: String::new(),
            },
            name: ScriptName::new(name).unwrap(),
        }
    }

    fn sort(value: serde_json::Value) -> Vec<Comparator<SieveScriptSort>> {
        Comparator::parse_list(value).unwrap()
    }

    /// RFC 8620 §5.5: `name` sorts with the comparator's collation,
    /// case-insensitive by default; one the session doesn't offer is
    /// `unsupportedSort`.
    #[test]
    fn sorts_honour_collations() {
        let list = [
            script(1, "alpha", false),
            script(2, "Beta", true),
            script(3, "10", false),
            script(4, "9", false),
        ];
        let order = |value| -> Vec<i64> {
            let comparators = sort(value);
            let checked = CheckedSort::new(&comparators).unwrap();
            let mut found: Vec<&Script> = list.iter().collect();
            found.sort_by(|a, b| compare(a, b, &checked));
            found.iter().map(|s| s.row.id.get()).collect()
        };
        // Bytes would put "Beta" before "alpha".
        assert_eq!(order(json!([{"property": "name"}])), [3, 4, 1, 2]);
        assert_eq!(
            order(json!([{"property": "name", "collation": "i;ascii-numeric"}])),
            [4, 3, 1, 2]
        );
        assert_eq!(
            order(json!([{"property": "isActive", "isAscending": false}])),
            [2, 1, 3, 4]
        );
        let octet = sort(json!([{"property": "name", "collation": "i;octet"}]));
        assert_eq!(
            CheckedSort::new(&octet).err().map(|e| e.kind()),
            Some(MethodErrorType::UnsupportedSort)
        );
    }

    /// `maxNumberScripts`: a new script past it is `overQuota`.
    #[test]
    fn script_count() {
        assert!(check_count(3, None).is_ok());
        assert!(check_count(3, Some(4)).is_ok());
        assert_eq!(
            check_count(3, Some(3)).unwrap_err().kind(),
            SetErrorType::OverQuota
        );
    }
}
