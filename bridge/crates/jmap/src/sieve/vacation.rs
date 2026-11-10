// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The vacation response (RFC 8621 §8) as the Sieve script `vacation`,
//! which the bridge writes, as Stalwart does:
//!
//! - `isEnabled` = the `vacation` script is the active one. ManageSieve
//!   has one active script, so turning the response on makes it the
//!   active script (the one active before stops running) and turning it
//!   off deactivates it; the one before is not switched back on. Saving it
//!   with `isEnabled: false` still writes the script, so another script
//!   (the web's filters) can `include :personal :optional "vacation"`.
//! - The script keeps the fields in a `# Data:` comment (JSON) and is read
//!   back only if writing those fields again gives exactly the same text;
//!   a `vacation` script written by hand is reported as an empty response
//!   (and replaced by the next `VacationResponse/set`).
//! - Dates: `currentdate :zone "+0000"` (extensions `date` and
//!   `relational`) against the UTC date-time, inclusive at both ends.
//! - The reply is `vacation :mime` with a `text/plain` and/or `text/html`
//!   part in base64, so no text of the user reaches Sieve syntax except
//!   the subject, which goes in a quoted string with `\`, `"` and `${`
//!   escaped (Stalwart expands variables even without `variables`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use nebula_jmap_proto::method::{GetArguments, GetResponse, SetArguments, SetResponse};
use nebula_jmap_proto::object::{
    VACATION_SINGLETON, VacationResponse, VacationResponsePatch, VacationResponseProperty,
};
use nebula_jmap_proto::{MethodResponse, SetError, SetErrorType, UtcDate, VacationResponseId};
use nebula_managesieve::proto::ScriptName;
use serde::{Deserialize, Serialize};

use super::script::{Fail, check_count, refused};
use super::{Session, Synced, VACATION_NAME, WriteTurn};
use crate::core::context::Ctx;
use crate::error::{Bug, CallError};

/// First line of the scripts we write.
const MARKER: &str = "# Nebula Mail vacation response (format 1).";
const DATA: &str = "# Data: ";
/// Can't appear in base64 (no `_`), so it never collides with a body.
const BOUNDARY: &str = "=_nebula_vacation_=";
/// Longest subject and body we accept, in bytes.
const MAX_SUBJECT: usize = 998;
const MAX_BODY: usize = 64 * 1024;
/// What Stalwart sends when neither body is set.
const DEFAULT_TEXT: &str = "I am away.";

/// The fields of a vacation response.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Fields {
    pub(crate) from_date: Option<UtcDate>,
    pub(crate) to_date: Option<UtcDate>,
    pub(crate) subject: Option<String>,
    pub(crate) text_body: Option<String>,
    pub(crate) html_body: Option<String>,
}

/// Sieve quoted string: `\` and `"` escaped, `${` written `$\{`.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    let mut previous = '\0';
    for c in text.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            '{' if previous == '$' => out.push_str("\\{"),
            _ => out.push(c),
        }
        previous = c;
    }
    out.push('"');
    out
}

fn base64_lines(text: &str) -> String {
    let encoded = BASE64.encode(text.as_bytes());
    let mut out = String::with_capacity(encoded.len().saturating_add(encoded.len() / 38));
    let mut rest = encoded.as_str();
    while !rest.is_empty() {
        let (line, tail) = rest.split_at(rest.len().min(76));
        out.push_str(line);
        out.push_str("\r\n");
        rest = tail;
    }
    out
}

fn part(media_type: &str, body: &str) -> String {
    format!(
        "Content-Type: {media_type}; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
        base64_lines(body)
    )
}

/// The MIME entity of `vacation :mime`.
fn mime(fields: &Fields) -> String {
    match (fields.text_body.as_deref(), fields.html_body.as_deref()) {
        (Some(text), Some(html)) => format!(
            "Content-Type: multipart/alternative; boundary=\"{BOUNDARY}\"\r\n\r\n\
             --{BOUNDARY}\r\n{}--{BOUNDARY}\r\n{}--{BOUNDARY}--\r\n",
            part("text/plain", text),
            part("text/html", html)
        ),
        (Some(text), None) => part("text/plain", text),
        (None, Some(html)) => part("text/html", html),
        (None, None) => part("text/plain", DEFAULT_TEXT),
    }
}

/// `text:` multi-line string body: lines starting with `.` doubled.
fn multiline(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(8));
    for line in text.split_inclusive("\r\n") {
        if line.starts_with('.') {
            out.push('.');
        }
        out.push_str(line);
    }
    out
}

/// `2026-10-30T00:00:00Z` → `2026-10-30T00:00:00`: compared as text with
/// the server's `iso8601`, whatever suffix it adds for UTC.
fn date_value(date: &UtcDate) -> String {
    let text = date.to_string();
    text.get(..19).unwrap_or(&text).to_owned()
}

/// The script for these fields.
pub(crate) fn script(fields: &Fields) -> Result<String, Bug> {
    let data = serde_json::to_string(fields).map_err(|_| Bug("vacation data"))?;
    let dated = fields.from_date.is_some() || fields.to_date.is_some();
    let mut out = String::new();
    out.push_str(MARKER);
    out.push_str("\r\n# Written by Nebula Mail's bridge (JMAP VacationResponse). Changes by\r\n");
    out.push_str("# hand are lost the next time the vacation response is saved.\r\n");
    out.push_str(DATA);
    out.push_str(&data);
    out.push_str("\r\n");
    out.push_str(if dated {
        "require [\"vacation\", \"date\", \"relational\"];\r\n\r\n"
    } else {
        "require [\"vacation\"];\r\n\r\n"
    });
    let mut tests = Vec::new();
    if let Some(from) = &fields.from_date {
        tests.push(format!(
            "currentdate :zone \"+0000\" :value \"ge\" \"iso8601\" \"{}\"",
            date_value(from)
        ));
    }
    if let Some(to) = &fields.to_date {
        tests.push(format!(
            "currentdate :zone \"+0000\" :value \"le\" \"iso8601\" \"{}\"",
            date_value(to)
        ));
    }
    let action = {
        let subject = fields
            .subject
            .as_deref()
            .map(|s| format!(":subject {} ", quoted(s)))
            .unwrap_or_default();
        format!(
            "vacation :mime {subject}text:\r\n{}.\r\n;\r\n",
            multiline(&mime(fields))
        )
    };
    let block = match tests.as_slice() {
        [] => action,
        [one] => format!("if {one} {{\r\n{action}}}\r\n"),
        _ => format!("if allof ({}) {{\r\n{action}}}\r\n", tests.join(", ")),
    };
    out.push_str(&block);
    Ok(out)
}

/// The fields of a script we wrote, `None` for anything else.
pub(crate) fn parse(content: &[u8]) -> Option<Fields> {
    let text = std::str::from_utf8(content).ok()?;
    let mut lines = text.split("\r\n");
    if lines.next()? != MARKER {
        return None;
    }
    let data = lines.find_map(|line| line.strip_prefix(DATA))?;
    let fields: Fields = serde_json::from_str(data).ok()?;
    (script(&fields).ok()?.as_bytes() == content).then_some(fields)
}

/// Checks the fields before writing them.
fn check(fields: &Fields, has_dates: bool) -> Result<(), SetError> {
    let invalid = |property: &str, why: &str| {
        SetError::invalid_properties(vec![property.to_owned()]).with_description(why.to_owned())
    };
    if let Some(subject) = &fields.subject {
        if subject.len() > MAX_SUBJECT {
            return Err(invalid("subject", "too long"));
        }
        if subject.chars().any(char::is_control) {
            return Err(invalid(
                "subject",
                "must be one line without control characters",
            ));
        }
    }
    for (property, body) in [
        ("textBody", &fields.text_body),
        ("htmlBody", &fields.html_body),
    ] {
        if let Some(body) = body {
            if body.len() > MAX_BODY {
                return Err(invalid(property, "too long"));
            }
            if body.contains('\0') {
                return Err(invalid(property, "must not contain NUL"));
            }
        }
    }
    if (fields.from_date.is_some() || fields.to_date.is_some()) && !has_dates {
        return Err(invalid(
            "fromDate",
            "the Sieve server has no 'date' and 'relational' extensions",
        ));
    }
    Ok(())
}

/// What reading the `vacation` script after a sync gave.
#[derive(Debug, PartialEq, Eq)]
enum VacationRead {
    /// The sync saw no such script.
    Missing,
    /// Its content, the one the sync saw.
    Content(Vec<u8>),
    /// Another client changed (or deleted) it after the sync.
    Changed,
}

/// What to do with a [`VacationRead`].
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// Go on with this content (`None`: there is no script).
    Use(Option<Vec<u8>>),
    /// Sync again and read again (once).
    Resync,
    /// It changed again: fail with an error the client may retry. Never
    /// "no script": a patch would start from the defaults and overwrite
    /// the other fields.
    Fail,
}

fn next(read: VacationRead, resynced: bool) -> Next {
    match read {
        VacationRead::Missing => Next::Use(None),
        VacationRead::Content(content) => Next::Use(Some(content)),
        VacationRead::Changed if resynced => Next::Fail,
        VacationRead::Changed => Next::Resync,
    }
}

/// How a call syncs: as a read, or inside its write turn.
#[derive(Clone, Copy)]
enum Turn<'a> {
    Read,
    Write(&'a WriteTurn),
}

/// The content of the `vacation` script of `synced` (`None`: there is
/// none), syncing again once if another client changed it meanwhile.
async fn current_content(
    inner: &crate::core::backend::Inner,
    session: &mut Session,
    synced: &mut Synced,
    turn: Turn<'_>,
) -> Result<Option<Vec<u8>>, CallError> {
    let mut resynced = false;
    loop {
        let read = match synced.by_name(VACATION_NAME) {
            None => VacationRead::Missing,
            Some(script) => match session.content(script).await? {
                Some(content) => VacationRead::Content(content),
                None => VacationRead::Changed,
            },
        };
        match next(read, resynced) {
            Next::Use(content) => return Ok(content),
            Next::Fail => {
                return Err(nebula_jmap_proto::MethodError::new(
                    nebula_jmap_proto::MethodErrorType::ServerUnavailable,
                )
                .with_description("the vacation response changed meanwhile; try again")
                .into());
            }
            Next::Resync => {
                resynced = true;
                *synced = match turn {
                    Turn::Read => session.sync(inner).await?,
                    Turn::Write(turn) => session.sync_turn(inner, turn).await?,
                };
            }
        }
    }
}

fn object(
    synced: &Synced,
    content: Option<&[u8]>,
    properties: Option<&[VacationResponseProperty]>,
) -> Result<VacationResponse, Bug> {
    let wants = |p| properties.is_none_or(|list| list.contains(&p));
    let script = synced.by_name(VACATION_NAME);
    let fields = content.and_then(parse).unwrap_or_default();
    Ok(VacationResponse {
        id: Some(VACATION_SINGLETON.parse().map_err(|_| Bug("singleton"))?),
        is_enabled: wants(VacationResponseProperty::IsEnabled)
            .then(|| script.is_some_and(|s| s.row.active)),
        from_date: wants(VacationResponseProperty::FromDate).then_some(fields.from_date),
        to_date: wants(VacationResponseProperty::ToDate).then_some(fields.to_date),
        subject: wants(VacationResponseProperty::Subject).then(|| fields.subject.clone()),
        text_body: wants(VacationResponseProperty::TextBody).then(|| fields.text_body.clone()),
        html_body: wants(VacationResponseProperty::HtmlBody).then(|| fields.html_body.clone()),
    })
}

pub(crate) async fn get(
    ctx: &Ctx<'_>,
    args: GetArguments<VacationResponseId, VacationResponseProperty>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let mut session = Session::open_ctx(ctx).await?;
    let read = async {
        let mut synced = session.sync(ctx.inner).await?;
        let content = current_content(ctx.inner, &mut session, &mut synced, Turn::Read).await?;
        Ok::<_, CallError>((synced, content))
    }
    .await;
    session.close().await;
    let (synced, content) = read?;
    let mut list = Vec::new();
    let mut not_found = Vec::new();
    match &args.ids {
        None => list.push(object(
            &synced,
            content.as_deref(),
            args.properties.as_deref(),
        )?),
        Some(ids) => {
            for id in ids.iter().filter_map(|id| ctx.resolve(id)) {
                if id.as_str() == VACATION_SINGLETON {
                    list.push(object(
                        &synced,
                        content.as_deref(),
                        args.properties.as_deref(),
                    )?);
                } else {
                    not_found.push(id);
                }
            }
        }
    }
    Ok(MethodResponse::VacationResponseGet(GetResponse {
        account_id: ctx.jmap_id(),
        state: synced.state,
        list,
        not_found,
    }))
}

fn merge(mut fields: Fields, patch: VacationResponsePatch) -> Fields {
    if let Some(value) = patch.from_date {
        fields.from_date = value;
    }
    if let Some(value) = patch.to_date {
        fields.to_date = value;
    }
    if let Some(value) = patch.subject {
        fields.subject = value;
    }
    if let Some(value) = patch.text_body {
        fields.text_body = value;
    }
    if let Some(value) = patch.html_body {
        fields.html_body = value;
    }
    fields
}

async fn update(
    inner: &crate::core::backend::Inner,
    session: &mut Session,
    turn: &WriteTurn,
    synced: &mut Synced,
    patch: VacationResponsePatch,
) -> Result<(), Fail> {
    let capabilities = session.client.capabilities();
    if !super::vacation_writable(capabilities) {
        return Err(SetError::new(SetErrorType::Forbidden)
            .with_description("the Sieve server has no vacation response the bridge can write")
            .into());
    }
    let has_dates = capabilities.has_extension("date") && capabilities.has_extension("relational");
    let stored = current_content(inner, session, synced, Turn::Write(turn)).await?;
    let was_active = synced.by_name(VACATION_NAME).is_some_and(|s| s.row.active);
    let enable = patch.is_enabled.unwrap_or(was_active);
    let old = stored.as_deref().and_then(parse).unwrap_or_default();
    let fields = merge(old, patch);
    check(&fields, has_dates)?;
    let content = script(&fields).map_err(CallError::from)?;
    let name = ScriptName::new(VACATION_NAME).map_err(|_| CallError::from(Bug("vacation name")))?;
    if synced.by_name(VACATION_NAME).is_none() {
        // The first vacation response is a new script: past
        // `maxNumberScripts`, every later sync would refuse the listing.
        let max = inner
            .sieve
            .as_ref()
            .and_then(|service| service.settings.max_scripts);
        check_count(synced.scripts.len(), max)?;
    }
    if stored.as_deref() != Some(content.as_bytes()) {
        session
            .client
            .put_script(&name, content.as_bytes())
            .await
            .map_err(refused)?;
    }
    if enable && !was_active {
        session
            .client
            .set_active(Some(&name))
            .await
            .map_err(refused)?;
    } else if !enable && was_active {
        session.client.set_active(None).await.map_err(refused)?;
    }
    Ok(())
}

pub(crate) async fn set(
    ctx: &Ctx<'_>,
    args: SetArguments<VacationResponseId>,
) -> Result<MethodResponse, CallError> {
    ctx.check_account(&args.account_id)?;
    let mut session = Session::open_ctx(ctx).await?;
    // One write at a time per account: two patches in parallel no longer
    // overwrite each other.
    let turn = session.write_turn().await;
    let result = set_inner(ctx, &mut session, &turn, args).await;
    drop(turn);
    session.close().await;
    result
}

async fn set_inner(
    ctx: &Ctx<'_>,
    session: &mut Session,
    turn: &WriteTurn,
    args: SetArguments<VacationResponseId>,
) -> Result<MethodResponse, CallError> {
    let mut synced = session.sync_turn(ctx.inner, turn).await?;
    if let Some(expected) = &args.if_in_state
        && *expected != synced.state
    {
        return Err(nebula_jmap_proto::MethodErrorType::StateMismatch.into());
    }
    let mut response: SetResponse<VacationResponse, VacationResponseId> = SetResponse::new(
        ctx.jmap_id(),
        Some(synced.state.clone()),
        synced.state.clone(),
    );
    for creation_id in args.create.into_keys() {
        response
            .not_created
            .insert(creation_id, SetError::new(SetErrorType::Singleton));
    }
    for id in args.destroy {
        response
            .not_destroyed
            .insert(id, SetError::new(SetErrorType::Singleton));
    }
    let mut changed = false;
    for (id, patch) in args.update {
        let singleton = ctx
            .resolve(&id)
            .is_some_and(|id| id.as_str() == VACATION_SINGLETON);
        if !singleton {
            response.not_updated.insert(id, SetError::not_found());
            continue;
        }
        let parsed = match VacationResponsePatch::parse(&patch) {
            Ok(parsed) => parsed,
            Err(error) => {
                response.not_updated.insert(id, error);
                continue;
            }
        };
        match update(ctx.inner, session, turn, &mut synced, parsed).await {
            Ok(()) => {
                changed = true;
                if let Some(id) = ctx.resolve(&id) {
                    response.updated.insert(id, None);
                }
            }
            Err(Fail::Set(error)) => {
                response.not_updated.insert(id, error);
            }
            Err(Fail::Call(error)) => return Err(error),
        }
    }
    if changed {
        response.new_state = session.sync_turn(ctx.inner, turn).await?.state;
    }
    Ok(MethodResponse::VacationResponseSet(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Fields {
        Fields {
            from_date: Some("2026-10-30T00:00:00Z".parse().unwrap()),
            to_date: Some("2026-11-02T23:59:59Z".parse().unwrap()),
            subject: Some("Out \"of\" office \\ ${name}".to_owned()),
            text_body: Some("Back on Monday.\n.\nBye ü".to_owned()),
            html_body: Some("<p>Back on <b>Monday</b>.</p>".to_owned()),
        }
    }

    /// A vacation script that changed after the sync is synced again once,
    /// then a retryable failure: never read as "no script".
    #[test]
    fn a_changed_script_is_not_missing() {
        assert_eq!(next(VacationRead::Missing, false), Next::Use(None));
        assert_eq!(
            next(VacationRead::Content(b"x".to_vec()), true),
            Next::Use(Some(b"x".to_vec()))
        );
        assert_eq!(next(VacationRead::Changed, false), Next::Resync);
        assert_eq!(next(VacationRead::Changed, true), Next::Fail);
    }

    #[test]
    fn round_trip() {
        for fields in [
            fields(),
            Fields::default(),
            Fields {
                text_body: Some("x".to_owned()),
                ..Fields::default()
            },
            Fields {
                html_body: Some("<i>x</i>".repeat(100)),
                to_date: Some("2026-11-02T23:59:59Z".parse().unwrap()),
                ..Fields::default()
            },
        ] {
            let text = script(&fields).unwrap();
            assert_eq!(parse(text.as_bytes()), Some(fields.clone()), "{text}");
        }
    }

    #[test]
    fn shape() {
        let text = script(&fields()).unwrap();
        assert!(text.starts_with(MARKER));
        assert!(text.contains("require [\"vacation\", \"date\", \"relational\"];"));
        assert!(text.contains(
            "if allof (currentdate :zone \"+0000\" :value \"ge\" \"iso8601\" \"2026-10-30T00:00:00\", \
             currentdate :zone \"+0000\" :value \"le\" \"iso8601\" \"2026-11-02T23:59:59\") {"
        ));
        assert!(text.contains(":subject \"Out \\\"of\\\" office \\\\ $\\{name}\" text:"));
        assert!(text.contains("boundary=\"=_nebula_vacation_=\""));
        // The bodies only appear in base64 (and in the data comment).
        let body = text.split_once("text:\r\n").unwrap().1;
        assert!(!body.contains("Monday"));
        let simple = script(&Fields::default()).unwrap();
        assert!(simple.contains("require [\"vacation\"];\r\n\r\nvacation :mime text:"));
    }

    #[test]
    fn foreign_scripts_are_not_read() {
        let text = script(&fields()).unwrap();
        assert_eq!(parse(text.replace("Back", "back").as_bytes()), None);
        assert_eq!(parse(format!("{text}# extra\r\n").as_bytes()), None);
        assert_eq!(parse(b"require \"vacation\"; vacation \"away\";"), None);
        assert_eq!(parse(b"\xff\xfe"), None);
    }

    #[test]
    fn checks() {
        assert!(check(&fields(), true).is_ok());
        assert!(check(&fields(), false).is_err());
        let bad_subject = Fields {
            subject: Some("a\r\nBcc: x".to_owned()),
            ..Fields::default()
        };
        assert!(check(&bad_subject, true).is_err());
        let big = Fields {
            text_body: Some("x".repeat(MAX_BODY + 1)),
            ..Fields::default()
        };
        assert!(check(&big, true).is_err());
    }

    #[test]
    fn dot_lines_are_stuffed() {
        assert_eq!(multiline(".a\r\nb\r\n..c\r\n"), "..a\r\nb\r\n...c\r\n");
    }
}
