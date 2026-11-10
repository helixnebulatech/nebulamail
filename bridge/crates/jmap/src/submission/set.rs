// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! `EmailSubmission/set` (RFC 8621 §7.5).
//!
//! Create: the email must be in the account; the bytes that go out must
//! have exactly one `From` field, holding only the identity's address, and
//! at most one `Sender`, also the identity's (`check_authors`); the
//! envelope's `MAIL FROM` must be that address. The envelope is the
//! client's or, if `null`, `MAIL FROM` = the identity and `RCPT TO` = To +
//! Cc + Bcc. The message goes out without `Bcc`.
//!
//! Sending is the one step that can't be undone, so it is the last one
//! that can fail: everything is checked and the submission's row is
//! stored first (with an empty SMTP reply), the response object is built,
//! and only then is the message sent. After a successful send nothing
//! (storing the reply, states, the implicit `Email/set`) turns it into a
//! failure: the client would send it again. A row whose reply stays empty
//! (the bridge stopped in between) means "outcome unknown" and is shown
//! so, never as delivered. If the send fails, the row is removed.
//!
//! After a successful send, `onSuccessUpdateEmail`/`onSuccessDestroyEmail`
//! run as an implicit `Email/set` (a second response with the same call
//! id), which the web app uses to move the draft to Sent and clear
//! `$draft`.
//!
//! Sent copy (`sent_copy`): with `append`, the bridge appends the message
//! (without Bcc) to the Sent mailbox **unless** the client's
//! `onSuccessUpdateEmail` puts the email there and does put it (then that
//! email is the copy; appending would leave two). Whether it did is known
//! after the implicit `Email/set`: if that update fails, the copy is
//! appended then. An email that `onSuccessDestroyEmail` destroys is no
//! copy. With `server`, nothing is appended.
//!
//! Held messages (RFC 4865 FUTURERELEASE, `submission::hold`): with
//! `HOLDFOR` (undo send) or `HOLDUNTIL` (scheduled send) the same checks
//! run, but nothing is sent: the message (as it would go out, without
//! `Bcc`) is stored sealed with the account's data key, the submission is
//! `pending` with `sendAt` = the release time, and `submission::release`
//! sends it then. The implicit `Email/set` runs at once (the web app files
//! the email in Sent, and puts it back in Drafts if it cancels). The Sent
//! copy follows the same rule, decided now and appended when it goes out.
//! A pending submission can be canceled (`undoStatus: canceled`) until the
//! scheduler takes it; it can't be destroyed (that would not stop it).
//!
//! Sent after sign-out (vendor property `sendWhenSignedOut`, only with
//! `[submission] send_while_signed_out`): a held message so marked, on
//! create or by an update while pending, goes out even if the user signs
//! out first (the HTTP layer keeps the account running until then).
//! Ignored for a message sent at once. Asking for it where the bridge
//! doesn't allow it is `invalidProperties`.
//!
//! ENVID (RFC 3461): if the client gives none, the bridge sends a random
//! one of its own, but only when the server announces DSN in the `EHLO` of
//! that same connection (otherwise none, and the message still goes out).
//! It is stored like a client's, so delivery reports must quote it to
//! change `deliveryStatus`; a recipient can't forge that, unlike the
//! `Message-ID`. It is **not** added to the `envelope` the client sees:
//! RFC 8621 §7 makes `envelope` immutable and §7.5 only lets the server
//! fill it in when the client sends `null`, and RFC 3461 §4.4 leaves the
//! ENVID to the sender's own use; ours only serves to match the reports
//! here, which the client gets already matched in `deliveryStatus`.

use std::collections::{BTreeMap, BTreeSet};

use mail_parser::MessageParser;
use nebula_jmap_proto::method::{PatchObject, SetResponse};
use nebula_jmap_proto::object::Email;
use nebula_jmap_proto::object::email::{
    EmailPatch, HeaderFormValue, HeaderProperty, HeaderValue, SetPatch,
};
use nebula_jmap_proto::object::{
    EmailSubmission, EmailSubmissionCreate, EmailSubmissionPatch, EmailSubmissionSetArguments,
    Envelope as JmapEnvelope, EnvelopeAddress, SEND_WHEN_SIGNED_OUT_PROPERTY, SmtpParameterValue,
    SmtpParameters, SubmissionAddress, UndoStatus,
};
use nebula_jmap_proto::{
    EmailId as JmapEmailId, EmailSubmissionId, IdOrRef, Keyword, MethodResponse, SetError,
    SetErrorType, State, UnsignedInt,
};
use std::sync::Arc;

use nebula_crypto::{Binding, Field};
use nebula_mail::{Copies, Lease, MailAccount};
use nebula_smtp::{
    Envelope, EnvelopeId, HeaderField, MailDsn, MailFrom, OutgoingMessage, RcptDsn, RcptTo,
};
use nebula_store::{
    AccountId, Cancel, DataType, Email as StoredEmail, EmailId, HeldLimits, Identity, MailboxId,
    NewHeld, NewSubmission, Role, SetSignedOut, StoreError, SubmissionId, UndoState, UnixTime,
};

use super::hold::{HoldError, HoldRequest, Now};
use super::send::{self, set_error};
use crate::core::backend::{Inner, SmtpLogin};
use crate::core::config::SentCopy;
use crate::core::context::Ctx;
use crate::core::ids;
use crate::email::set::update_and_destroy;
use crate::error::{CallError, mail_set_error, store_set_error};
use crate::identity::identities;

fn invalid(property: &str, why: &str) -> SetError {
    SetError::invalid_properties([property]).with_description(why.to_owned())
}

/// Checks a request for `sendWhenSignedOut: on` against what the bridge
/// allows: turning it on where it isn't allowed is `invalidProperties`
/// (off is always fine).
fn check_signed_out(on: bool, allowed: bool) -> Result<bool, SetError> {
    if on && !allowed {
        return Err(invalid(
            SEND_WHEN_SIGNED_OUT_PROPERTY,
            "this server does not send held messages after sign-out",
        ));
    }
    Ok(on)
}

/// The envelope: the client's, or built from the email's headers.
fn envelope_of(
    given: Option<JmapEnvelope>,
    email: &StoredEmail,
    identity: &Identity,
) -> Result<JmapEnvelope, SetError> {
    if let Some(envelope) = given {
        return Ok(envelope);
    }
    let mail_from = SubmissionAddress {
        email: identity
            .email
            .parse()
            .map_err(|_| SetError::new(SetErrorType::ForbiddenMailFrom))?,
        parameters: None,
    };
    let meta = &email.meta;
    let mut rcpt_to: Vec<SubmissionAddress> = Vec::new();
    let mut bad = Vec::new();
    for address in [&meta.to, &meta.cc, &meta.bcc]
        .into_iter()
        .flatten()
        .flatten()
    {
        match address.email.trim().parse::<EnvelopeAddress>() {
            Ok(email) if !email.as_str().is_empty() => {
                if !rcpt_to.iter().any(|r| r.email == email) {
                    rcpt_to.push(SubmissionAddress {
                        email,
                        parameters: None,
                    });
                }
            }
            _ => bad.push(address.email.clone()),
        }
    }
    if !bad.is_empty() {
        return Err(SetError::invalid_recipients(bad));
    }
    Ok(JmapEnvelope { mail_from, rcpt_to })
}

/// A parameter value that must be there and parse.
fn value<T: std::str::FromStr>(
    name: &str,
    value: Option<&SmtpParameterValue>,
) -> Result<T, SetError> {
    value.and_then(|v| v.as_str().parse().ok()).ok_or_else(|| {
        invalid(
            "envelope",
            &format!("invalid value for SMTP parameter {name}"),
        )
    })
}

/// Parameters the bridge sets itself from the message and envelope
/// (`BODY`, `SMTPUTF8`, `SIZE`): accepted and ignored.
const COMPUTED: [&str; 3] = ["BODY", "SMTPUTF8", "SIZE"];

/// A FUTURERELEASE parameter's value, which must be there.
fn hold_value<'a>(
    name: &'static str,
    given: Option<&'a SmtpParameterValue>,
) -> Result<&'a str, HoldError> {
    given
        .map(SmtpParameterValue::as_str)
        .ok_or(HoldError::Malformed(name))
}

/// The `MAIL FROM` parameters: DSN (`RET`, `ENVID`), FUTURERELEASE
/// (`HOLDFOR`, `HOLDUNTIL`: the bridge holds the message itself, they are
/// never sent) and the computed ones. Anything else is refused: no free
/// text reaches the SMTP command.
fn mail_dsn(
    parameters: Option<&SmtpParameters>,
) -> Result<(MailDsn, Option<HoldRequest>), SetError> {
    let mut dsn = MailDsn::default();
    let mut hold = None;
    for (name, given) in parameters.into_iter().flatten() {
        match name.as_str() {
            "RET" => dsn.ret = Some(value(name.as_str(), given.as_ref())?),
            "ENVID" => dsn.envid = Some(value(name.as_str(), given.as_ref())?),
            "HOLDFOR" | "HOLDUNTIL" if hold.is_some() => {
                return Err(HoldError::Both.set_error());
            }
            "HOLDFOR" => {
                hold = Some(
                    hold_value("HOLDFOR", given.as_ref())
                        .and_then(HoldRequest::parse_for)
                        .map_err(HoldError::set_error)?,
                );
            }
            "HOLDUNTIL" => {
                hold = Some(
                    hold_value("HOLDUNTIL", given.as_ref())
                        .and_then(HoldRequest::parse_until)
                        .map_err(HoldError::set_error)?,
                );
            }
            other if COMPUTED.contains(&other) => {}
            other => {
                return Err(invalid(
                    "envelope",
                    &format!("unsupported SMTP parameter {other}"),
                ));
            }
        }
    }
    Ok((dsn, hold))
}

/// A random ENVID of our own when the client gave none (see the module
/// docs). `nebula-smtp` sends it only if this send's `EHLO` has DSN.
pub(crate) fn own_envid(envelope: &Envelope) -> Result<Option<EnvelopeId>, SetError> {
    if envelope.mail_dsn().envid.is_some() {
        return Ok(None);
    }
    EnvelopeId::random().map(Some).map_err(|error| {
        tracing::error!(error = %error, "no ENVID");
        SetError::new(SetErrorType::Forbidden).with_description("internal error")
    })
}

/// The `RCPT TO` parameters: DSN (`NOTIFY`, `ORCPT`).
fn rcpt_dsn(parameters: Option<&SmtpParameters>) -> Result<RcptDsn, SetError> {
    let mut dsn = RcptDsn::default();
    for (name, given) in parameters.into_iter().flatten() {
        match name.as_str() {
            "NOTIFY" => dsn.notify = Some(value(name.as_str(), given.as_ref())?),
            "ORCPT" => dsn.orcpt = Some(value(name.as_str(), given.as_ref())?),
            other => {
                return Err(invalid(
                    "envelope",
                    &format!("unsupported SMTP parameter {other}"),
                ));
            }
        }
    }
    Ok(dsn)
}

/// The SMTP envelope, checked against the identity, and the hold asked for.
fn smtp_envelope(
    envelope: &JmapEnvelope,
    identity: &Identity,
) -> Result<(Envelope, Option<HoldRequest>), SetError> {
    if !envelope
        .mail_from
        .email
        .as_str()
        .eq_ignore_ascii_case(identity.email.trim())
    {
        return Err(SetError::new(SetErrorType::ForbiddenMailFrom)
            .with_description("MAIL FROM must be the identity's address"));
    }
    envelope_to_smtp(envelope)
}

/// The SMTP envelope of a JMAP one (already checked against its identity
/// when it was submitted: a held message is sent with this), and the hold
/// it asks for.
pub(crate) fn envelope_to_smtp(
    envelope: &JmapEnvelope,
) -> Result<(Envelope, Option<HoldRequest>), SetError> {
    let from: MailFrom = envelope
        .mail_from
        .email
        .as_str()
        .parse()
        .map_err(|_| SetError::new(SetErrorType::ForbiddenMailFrom))?;
    if envelope.rcpt_to.is_empty() {
        return Err(SetError::new(SetErrorType::NoRecipients));
    }
    if envelope.rcpt_to.len() > Envelope::MAX_RECIPIENTS {
        return Err(SetError::too_many_recipients(
            UnsignedInt::try_from(Envelope::MAX_RECIPIENTS).unwrap_or(UnsignedInt::MAX),
        ));
    }
    let mut to = Vec::with_capacity(envelope.rcpt_to.len());
    let mut bad = Vec::new();
    for rcpt in &envelope.rcpt_to {
        match rcpt.email.as_str().parse::<RcptTo>() {
            Ok(address) => to.push(address),
            Err(_) => bad.push(rcpt.email.as_str().to_owned()),
        }
    }
    if !bad.is_empty() {
        return Err(SetError::invalid_recipients(bad));
    }
    let (dsn, hold) = mail_dsn(envelope.mail_from.parameters.as_ref())?;
    let mut smtp = Envelope::new(from, to.clone())
        .map_err(|_| SetError::new(SetErrorType::NoRecipients))?
        .with_mail_dsn(dsn);
    for (address, rcpt) in to.iter().zip(&envelope.rcpt_to) {
        let dsn = rcpt_dsn(rcpt.parameters.as_ref())?;
        if !dsn.is_empty() {
            smtp = smtp
                .with_recipient_dsn(address, dsn)
                .map_err(|_| SetError::new(SetErrorType::NoRecipients))?;
        }
    }
    Ok((smtp, hold))
}

/// Whether the client's `onSuccessUpdateEmail` for this submission puts
/// the email in `sent`.
fn patch_adds(ctx: &Ctx<'_>, patch: Option<&PatchObject>, sent: MailboxId) -> bool {
    let Some(parsed) = patch.and_then(|p| EmailPatch::parse(p).ok()) else {
        return false;
    };
    let has = |list: &BTreeSet<IdOrRef<nebula_jmap_proto::MailboxId>>| {
        list.iter()
            .any(|id| ctx.resolve(id).as_ref().and_then(ids::store_mailbox) == Some(sent))
    };
    match &parsed.mailbox_ids {
        Some(SetPatch::Replace(list)) => has(list),
        Some(SetPatch::Change { add, .. }) => has(add),
        _ => false,
    }
}

/// What to do about the Sent copy of one submission ([`sent_copy_plan`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyPlan {
    /// Append the copy now.
    Now,
    /// The client's `onSuccessUpdateEmail` puts the email in Sent: that
    /// email is the copy, unless the implicit `Email/set` fails for it.
    IfUpdateFails,
}

/// The Sent copy rule (see the module docs), decided with what the
/// client asked for; whether its patch worked is only known after the
/// implicit `Email/set`. An email the client also destroys is no copy.
fn sent_copy_plan(patch_adds: bool, destroyed: bool) -> CopyPlan {
    if patch_adds && !destroyed {
        CopyPlan::IfUpdateFails
    } else {
        CopyPlan::Now
    }
}

/// A copy to append after the implicit `Email/set`, if the patch that
/// would have made one failed.
struct LaterCopy {
    sent: MailboxId,
    email: EmailId,
    jmap_email: JmapEmailId,
}

/// Everything checked: what is left is holding or sending.
struct Checked {
    identity: Identity,
    email: StoredEmail,
    jmap_email: JmapEmailId,
    /// The envelope as stored (the client's, or built from the headers).
    envelope: JmapEnvelope,
    smtp: Envelope,
    /// When to release it, if held.
    release: Option<UnixTime>,
    message: Arc<OutgoingMessage>,
    own_envid: Option<EnvelopeId>,
    /// The memory of `message` (and of its 7-bit copy, if one is made).
    lease: Lease,
    now: UnixTime,
    /// If held: it goes out even after the user signs out (ignored for a
    /// message sent at once).
    send_when_signed_out: bool,
}

/// Everything checked and the submission's row reserved: what is left is
/// sending. The response is built here, before sending, so nothing after
/// the send can fail.
struct Prepared {
    row: SubmissionId,
    id: EmailSubmissionId,
    email: StoredEmail,
    jmap_email: JmapEmailId,
    object: EmailSubmission,
    smtp: Envelope,
    message: Arc<OutgoingMessage>,
    own_envid: Option<EnvelopeId>,
    /// The memory of `message` (and of its 7-bit copy, if one is made).
    lease: Lease,
}

/// What a successful create gives back.
struct Sent {
    id: EmailSubmissionId,
    email: JmapEmailId,
    object: EmailSubmission,
    later_copy: Option<LaterCopy>,
    /// A held submission whose Sent copy waits for the implicit
    /// `Email/set`: none if the client's patch filed the email in Sent.
    held_copy: Option<HeldCopy>,
}

/// A held submission that appends its Sent copy when it goes out, unless
/// the client's `onSuccessUpdateEmail` files the email in Sent now.
struct HeldCopy {
    row: SubmissionId,
    jmap_email: JmapEmailId,
}

/// Whether `outgoing` (the bytes that are sent) has exactly one `From`
/// field with one mailbox, the identity's, and at most one `Sender`, also
/// the identity's. The fields are split as `remove_bcc` splits them (the
/// same bytes, the same lines), not as `mail-parser` does: what is
/// checked is what the server gets. Several authors are not supported.
fn check_authors(outgoing: &OutgoingMessage, identity: &str) -> Result<(), SetError> {
    let forbidden =
        |why: &str| SetError::new(SetErrorType::ForbiddenFrom).with_description(why.to_owned());
    let mut from = Vec::new();
    let mut sender = Vec::new();
    for field in outgoing.header_fields() {
        match field {
            HeaderField::Field { name, value } if name.eq_ignore_ascii_case(b"from") => {
                from.push(value);
            }
            HeaderField::Field { name, value } if name.eq_ignore_ascii_case(b"sender") => {
                sender.push(value);
            }
            HeaderField::Field { .. } => {}
            HeaderField::Malformed | _ => {
                return Err(SetError::new(SetErrorType::InvalidEmail)
                    .with_description("the message has a header line that is not a field"));
            }
        }
    }
    let is_identity = |value: &[u8]| {
        only_mailbox(value).is_some_and(|address| address.eq_ignore_ascii_case(identity.trim()))
    };
    match from.as_slice() {
        [one] if is_identity(one) => {}
        [_] => {
            return Err(forbidden(
                "the From address must be the identity's, and only it",
            ));
        }
        _ => return Err(forbidden("the message must have exactly one From field")),
    }
    match sender.as_slice() {
        [] => Ok(()),
        [one] if is_identity(one) => Ok(()),
        _ => Err(forbidden("a Sender field must be the identity's address")),
    }
}

/// The address of a field value that holds exactly one mailbox (no
/// group, no list).
fn only_mailbox(value: &[u8]) -> Option<String> {
    let mut field = b"From:".to_vec();
    field.extend_from_slice(value);
    if !field.ends_with(b"\n") {
        field.extend_from_slice(b"\r\n");
    }
    field.extend_from_slice(b"\r\n");
    let parsed = MessageParser::default().parse_headers(field.as_slice())?;
    let property: HeaderProperty = "header:From:asGroupedAddresses".parse().ok()?;
    let HeaderValue::Last(HeaderFormValue::GroupedAddresses(groups)) =
        crate::email::headers::property(parsed.headers(), &property)
    else {
        return None;
    };
    match groups.as_slice() {
        [group] if group.name.is_none() => match group.addresses.as_slice() {
            [one] => Some(one.email.trim().to_owned()),
            _ => None,
        },
        _ => None,
    }
}

fn bug<E>(_: E) -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description("internal error")
}

/// Checks a create (see [`Checked`]).
async fn check(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    identities: &[Identity],
    object: serde_json::Map<String, serde_json::Value>,
) -> Result<Checked, SetError> {
    let create = EmailSubmissionCreate::parse(object)?;
    let send_when_signed_out = check_signed_out(
        create.send_when_signed_out,
        ctx.config().hold.sends_while_signed_out(),
    )?;
    let identity = ctx
        .resolve(&create.identity_id)
        .as_ref()
        .and_then(ids::store_identity)
        .and_then(|row| identities.iter().find(|i| i.id == row))
        .ok_or_else(|| invalid("identityId", "unknown identity"))?;
    let jmap_email = ctx
        .resolve(&create.email_id)
        .ok_or_else(|| invalid("emailId", "unknown email"))?;
    let email = match ids::store_email(&jmap_email) {
        Some(row) => ctx
            .store()
            .emails(ctx.id(), vec![row])
            .await
            .map_err(|e| store_set_error(&e))?
            .pop(),
        None => None,
    }
    .ok_or_else(|| invalid("emailId", "unknown email"))?;
    let envelope = envelope_of(create.envelope, &email, identity)?;
    let (smtp, hold) = smtp_envelope(&envelope, identity)?;
    let clock = Now::system().ok_or_else(|| SetError::new(SetErrorType::Forbidden))?;
    let release = match hold {
        Some(hold) => hold
            .release_at(clock, ctx.config().hold.max_delay)
            .map_err(HoldError::set_error)?,
        None => None,
    };
    // Without the client's ENVID, ours (used only if the server has DSN):
    // see `own_envid`.
    let own_envid = own_envid(&smtp)?;
    // The message and the `OutgoingMessage` made from it.
    let mut lease = mail.lease();
    let raw = mail
        .load_message(email.id, &mut lease, Copies::Two)
        .await
        .map_err(|e| mail_set_error(&e))?;
    let message =
        OutgoingMessage::new(&raw).map_err(|_| SetError::new(SetErrorType::InvalidEmail))?;
    let raw_len = u64::try_from(raw.len()).unwrap_or(u64::MAX);
    drop(raw);
    // Only the `OutgoingMessage` is left; and the SMTP dialogue to come
    // must not keep the account's turn.
    lease.keep_only(raw_len);
    let message = Arc::new(message);
    check_authors(&message, &identity.email)?;
    Ok(Checked {
        identity: identity.clone(),
        email,
        jmap_email,
        envelope,
        smtp,
        release,
        message,
        own_envid,
        lease,
        now: clock.floor(),
        send_when_signed_out,
    })
}

/// The row of a checked submission, sent at `send_at`.
fn new_row(checked: &Checked, ctx: &Ctx<'_>, send_at: UnixTime) -> NewSubmission {
    NewSubmission {
        identity: checked.identity.id,
        email: checked.email.id,
        thread: checked.email.thread,
        envelope: checked.envelope.clone(),
        send_at,
        smtp_reply: String::new(),
        expires_at: send_at.saturating_add(ctx.config().submission_lifetime),
        message_id: checked.email.meta.message_id.clone(),
        envid: None,
    }
}

/// Reserves the row of a checked submission sent now (see [`Prepared`]).
async fn prepare(ctx: &Ctx<'_>, checked: Checked) -> Result<Prepared, SetError> {
    let now = checked.now;
    let thread_id = ids::thread_id(checked.email.thread).map_err(bug)?;
    let send_at = crate::email::get::utc_date(now.secs()).map_err(bug)?;
    // The row goes in before sending: a message never goes out without
    // its record. Its empty reply means "outcome unknown" until
    // `finish_submission` fills it in.
    let (row, _) = ctx
        .store()
        .add_submission(ctx.id(), new_row(&checked, ctx, now))
        .await
        .map_err(|e| store_set_error(&e))?;
    let Checked {
        email,
        jmap_email,
        smtp,
        message,
        own_envid,
        lease,
        ..
    } = checked;
    let id = match ids::submission_id(row) {
        Ok(id) => id,
        Err(error) => {
            forget(ctx, row).await;
            return Err(bug(error));
        }
    };
    let object = EmailSubmission {
        id: Some(id.clone()),
        thread_id: Some(thread_id),
        send_at: Some(send_at),
        undo_status: Some(UndoStatus::Final),
        ..EmailSubmission::default()
    };
    Ok(Prepared {
        row,
        id,
        email,
        jmap_email,
        object,
        smtp,
        message,
        own_envid,
        lease,
    })
}

/// What a held message is bound to: its submission (`nebula-crypto`).
pub(crate) fn held_binding(row: SubmissionId) -> [u8; 19] {
    let mut out = *b"submission\0\0\0\0\0\0\0\0\0";
    if let Some(tail) = out.get_mut(11..) {
        tail.copy_from_slice(&row.get().to_be_bytes());
    }
    out
}

/// Stores a checked submission as held until `release`: its message
/// sealed with the account's data key (bound to the row), its Sent copy
/// decided now ([`sent_copy_plan`]). Nothing is sent.
async fn hold(
    ctx: &Ctx<'_>,
    checked: Checked,
    release: UnixTime,
    login: Arc<SmtpLogin>,
    copy: Option<CopyPlan>,
) -> Result<Sent, SetError> {
    let config = ctx.config().hold;
    let size = u64::try_from(checked.message.as_bytes().len()).unwrap_or(u64::MAX);
    if usize::try_from(size).is_ok_and(|len| len > nebula_crypto::SealedField::MAX_MESSAGE_LEN) {
        return Err(SetError::new(SetErrorType::TooLarge)
            .with_description("the message is too large to hold"));
    }
    let thread_id = ids::thread_id(checked.email.thread).map_err(bug)?;
    let send_at = crate::email::get::utc_date(release.secs()).map_err(bug)?;
    let message = Arc::clone(&checked.message);
    let seal = move |row: SubmissionId| {
        let binding = held_binding(row);
        let binding = Binding::new(&binding).map_err(|_| StoreError::Invalid("binding"))?;
        login
            .key
            .seal_bound(Field::HeldMessage, &binding, message.as_bytes())
            .map(|sealed| sealed.as_bytes().to_vec())
            .map_err(|error| {
                tracing::error!(error = %error, "a held message could not be sealed");
                StoreError::Invalid("held message")
            })
    };
    let (row, _) = ctx
        .store()
        .add_held_submission(
            ctx.id(),
            new_row(&checked, ctx, release),
            NewHeld {
                size,
                sent_copy: copy.is_some(),
                send_when_signed_out: checked.send_when_signed_out,
            },
            HeldLimits {
                max_count: config.max_count,
                max_bytes: config.max_bytes,
            },
            seal,
        )
        .await
        .map_err(|e| match e {
            StoreError::OverQuota(what) => SetError::new(SetErrorType::OverQuota)
                .with_description(format!("too many {what} waiting to be sent")),
            other => store_set_error(&other),
        })?;
    ctx.inner.held_wake.notify_one();
    tracing::info!(
        account = ctx.id().get(),
        recipients = checked.smtp.to().len(),
        send_when_signed_out = checked.send_when_signed_out,
        "message held"
    );
    let id = ids::submission_id(row).map_err(bug)?;
    Ok(Sent {
        id: id.clone(),
        email: checked.jmap_email.clone(),
        object: EmailSubmission {
            id: Some(id),
            thread_id: Some(thread_id),
            send_at: Some(send_at),
            undo_status: Some(UndoStatus::Pending),
            send_when_signed_out: config
                .sends_while_signed_out()
                .then_some(checked.send_when_signed_out),
            ..EmailSubmission::default()
        },
        later_copy: None,
        held_copy: (copy == Some(CopyPlan::IfUpdateFails)).then_some(HeldCopy {
            row,
            jmap_email: checked.jmap_email,
        }),
    })
}

/// Drops a reserved row whose message was not sent. A failure is only
/// logged: the row expires with `submission_lifetime`.
async fn forget(ctx: &Ctx<'_>, row: SubmissionId) {
    if let Err(error) = ctx.store().destroy_submission(ctx.id(), row).await {
        tracing::warn!(error = %error, "a reserved submission could not be removed");
    }
}

/// Sends a prepared message and records the outcome in its row: the
/// server's reply, or the row dropped (nothing went out), or kept with no
/// reply (the outcome is unknown). Owns everything, so it can run on its
/// own task; the lease comes back for the Sent copy.
#[allow(
    clippy::too_many_arguments,
    reason = "the parts of a `Prepared` the task needs, moved in"
)]
async fn send_and_record(
    inner: Arc<Inner>,
    account: AccountId,
    login: Arc<SmtpLogin>,
    row: SubmissionId,
    smtp: Envelope,
    message: Arc<OutgoingMessage>,
    mut lease: Lease,
    own_envid: Option<EnvelopeId>,
) -> (Result<nebula_smtp::Sent, send::SendError>, Lease) {
    let result = send::send(&inner, &login, &smtp, message, &mut lease, own_envid).await;
    drop(login);
    match &result {
        Ok(sent) => {
            tracing::info!(
                account = account.get(),
                recipients = smtp.to().len(),
                "message sent"
            );
            let reply = format!("{} {}", sent.reply.code, sent.reply.text);
            // Reports quote it back; then they must (see the store's
            // `delivery`).
            let envid = sent.envid.as_ref().map(|id| id.as_str().to_owned());
            if let Err(error) = inner
                .store
                .finish_submission(account, row, reply, envid)
                .await
            {
                tracing::error!(error = %error, "the server's reply to a sent message could not be stored");
            }
        }
        Err(error) if send::outcome_unknown(error) => {
            // It may have gone out: the row stays, its empty reply says
            // "outcome unknown" (never shown as a success).
            tracing::warn!(
                account = account.get(),
                "submission outcome unknown; its record is kept"
            );
        }
        Err(_) => {
            if let Err(error) = inner.store.destroy_submission(account, row).await {
                tracing::warn!(error = %error, "a reserved submission could not be removed");
            }
        }
    }
    (result, lease)
}

async fn create_one(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    identities: &[Identity],
    object: serde_json::Map<String, serde_json::Value>,
    on_success: Option<&PatchObject>,
    destroyed: bool,
) -> Result<Sent, SetError> {
    let checked = check(ctx, mail, identities, object).await?;
    let Some(login) = ctx.inner.smtp_login(ctx.id()) else {
        return Err(SetError::new(SetErrorType::ForbiddenToSend)
            .with_description("temporary failure, try again later: not connected"));
    };
    if let Some(release) = checked.release {
        let copy = sent_mailbox(ctx)
            .await
            .map(|sent| sent_copy_plan(patch_adds(ctx, on_success, sent), destroyed));
        return hold(ctx, checked, release, login, copy).await;
    }
    let prepared = prepare(ctx, checked).await?;
    let Prepared {
        row,
        id,
        email,
        jmap_email,
        object,
        smtp,
        message,
        own_envid,
        lease,
    } = prepared;
    // The send and its record run on their own task: if the request goes
    // away meanwhile (the tab closed, the HTTP timeout of a slow transfer:
    // the SMTP limits are RFC 5321's, minutes long), the message still goes
    // out or fails cleanly, and its row says so, instead of being cut in
    // the middle of DATA and left "unknown".
    let task = tokio::spawn(send_and_record(
        Arc::clone(ctx.inner),
        ctx.id(),
        login,
        row,
        smtp,
        Arc::clone(&message),
        lease,
        own_envid,
    ));
    let (result, lease) = match task.await {
        Ok(done) => done,
        Err(error) => {
            // A panic in the send: it may have gone out; its row stays.
            tracing::error!(error = %error, "the send task failed");
            return Err(
                SetError::new(SetErrorType::ForbiddenToSend).with_description(
                    "the message may have been sent; check Sent before sending it again",
                ),
            );
        }
    };
    // The reply is in the row (`send_and_record`).
    if let Err(error) = result {
        return Err(set_error(&error));
    }
    let later_copy = match sent_mailbox(ctx).await {
        None => None,
        Some(sent) => {
            let patch_adds = patch_adds(ctx, on_success, sent);
            match sent_copy_plan(patch_adds, destroyed) {
                CopyPlan::Now => {
                    append_sent(mail, sent, message.as_bytes()).await;
                    None
                }
                CopyPlan::IfUpdateFails => Some(LaterCopy {
                    sent,
                    email: email.id,
                    jmap_email: jmap_email.clone(),
                }),
            }
        }
    };
    // Held until here: the Sent copy above still used the message.
    drop(lease);
    Ok(Sent {
        id,
        email: jmap_email,
        object,
        later_copy,
        held_copy: None,
    })
}

/// The Sent copies whose patch didn't put the email in Sent: the email is
/// read again (it is still where it was, and likely cached) instead of
/// holding every message for the whole call. Two submissions of the same
/// email whose update fails give two copies (two messages went out).
async fn later_copies(mail: &MailAccount, copies: Vec<LaterCopy>, updated: &BTreeSet<JmapEmailId>) {
    for copy in copies {
        if updated.contains(&copy.jmap_email) {
            continue;
        }
        // The message and the `OutgoingMessage` made from it.
        let mut lease = mail.lease();
        let raw = match mail.load_message(copy.email, &mut lease, Copies::Two).await {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(error = %error, "the sent message could not be saved in Sent");
                continue;
            }
        };
        lease.drop_turn();
        match OutgoingMessage::new(&raw) {
            Ok(message) => append_sent(mail, copy.sent, message.as_bytes()).await,
            Err(error) => {
                tracing::warn!(error = %error, "the sent message could not be saved in Sent");
            }
        }
    }
}

/// Where the bridge keeps a copy of what it sends: the Sent mailbox, or
/// `None` when the server keeps its own (`sent_copy = "server"`) or there
/// is no Sent mailbox (logged).
pub(crate) async fn sent_mailbox(ctx: &Ctx<'_>) -> Option<MailboxId> {
    sent_mailbox_of(ctx.inner, ctx.id()).await
}

/// [`sent_mailbox`] of any account.
pub(crate) async fn sent_mailbox_of(inner: &Inner, account: AccountId) -> Option<MailboxId> {
    if inner.config.sent_copy == SentCopy::Server {
        return None;
    }
    match inner.store.mailbox_by_role(account, Role::Sent).await {
        Ok(Some(sent)) => Some(sent.id),
        Ok(None) => {
            tracing::warn!("no Sent mailbox: the sent message is not saved");
            None
        }
        Err(error) => {
            tracing::warn!(error = %error, "Sent mailbox lookup failed");
            None
        }
    }
}

/// Appends a sent message to `sent`, read (`$seen`). A failure is only
/// logged: the message already went out.
pub(crate) async fn append_sent(mail: &MailAccount, sent: MailboxId, outgoing: &[u8]) {
    let keywords = BTreeSet::from([Keyword::seen()]);
    if let Err(error) = mail.append(sent, outgoing, &keywords, None).await {
        tracing::warn!(error = %error, "the sent message could not be saved in Sent");
    }
}

/// The state after the call, or `old` if it can't be read: the next
/// `/changes` then gives the changes again, which is safe. Used once
/// messages went out, when no error may hide that.
async fn state_or(ctx: &Ctx<'_>, data_type: DataType, old: &State) -> State {
    match ctx.state(data_type).await {
        Ok(state) => state,
        Err(error) => {
            tracing::error!(error = %error, "the new state could not be read");
            old.clone()
        }
    }
}

pub(crate) async fn set(
    ctx: &mut Ctx<'_>,
    args: EmailSubmissionSetArguments,
) -> Result<Vec<MethodResponse>, CallError> {
    let EmailSubmissionSetArguments {
        set: request,
        on_success_update_email,
        on_success_destroy_email,
    } = args;
    ctx.check_account(&request.account_id)?;
    let old_state = ctx
        .check_state(DataType::EmailSubmission, request.if_in_state.as_ref())
        .await?;
    let mail = ctx.mail()?;
    let identities = identities(ctx).await?;
    // Read before anything is sent, so later failures can't need it.
    let email_old_state =
        if on_success_update_email.is_empty() && on_success_destroy_email.is_empty() {
            None
        } else {
            Some(ctx.state(DataType::Email).await?)
        };

    // From the first send on, nothing below returns an error: what was
    // sent is always reported as created.
    // Submission (as the client names it) → its email, for onSuccess*.
    let mut succeeded: BTreeMap<IdOrRef<EmailSubmissionId>, JmapEmailId> = BTreeMap::new();
    let mut created = BTreeMap::new();
    let mut not_created = BTreeMap::new();
    let mut copies = Vec::new();
    let mut held_copies = Vec::new();
    for (creation_id, object) in request.create {
        let key = IdOrRef::Ref(creation_id.clone());
        let on_success = on_success_update_email.get(&key);
        let destroyed = on_success_destroy_email.contains(&key);
        match create_one(ctx, &mail, &identities, object, on_success, destroyed).await {
            Ok(sent) => {
                ctx.created.insert(creation_id.clone(), sent.id.clone());
                succeeded.insert(key, sent.email.clone());
                succeeded.insert(IdOrRef::Id(sent.id), sent.email);
                copies.extend(sent.later_copy);
                held_copies.extend(sent.held_copy);
                created.insert(creation_id, sent.object);
            }
            Err(error) => {
                not_created.insert(creation_id, error);
            }
        }
    }

    let mut response =
        update_and_destroy_submissions(ctx, &request.update, &request.destroy, &old_state).await;
    response.old_state = Some(old_state);
    response.created = created;
    response.not_created = not_created;
    let mut responses = vec![MethodResponse::EmailSubmissionSet(response)];
    let implicit = match email_old_state {
        Some(email_old_state) => {
            implicit_email_set(
                ctx,
                &mail,
                (&on_success_update_email, &on_success_destroy_email),
                &succeeded,
                email_old_state,
            )
            .await
        }
        None => None,
    };
    let updated: BTreeSet<JmapEmailId> = implicit
        .as_ref()
        .map(|implicit| implicit.updated.keys().cloned().collect())
        .unwrap_or_default();
    later_copies(&mail, copies, &updated).await;
    // A held message whose email the client filed in Sent needs no copy.
    for copy in held_copies {
        if updated.contains(&copy.jmap_email)
            && let Err(error) = ctx
                .store()
                .set_held_sent_copy(ctx.id(), copy.row, false)
                .await
        {
            tracing::warn!(error = %error, "a held message will be saved in Sent twice");
        }
    }
    if let Some(implicit) = implicit {
        responses.push(MethodResponse::EmailSet(implicit));
    }
    Ok(responses)
}

type SubmissionUpdates = BTreeMap<IdOrRef<EmailSubmissionId>, PatchObject>;

/// Every update and destroy failed with `error`, and the state stays
/// `old_state`: for when the store can't even list the submissions.
fn degraded_response(
    ctx_account: nebula_jmap_proto::AccountId,
    old_state: &State,
    update: &SubmissionUpdates,
    destroy: &[IdOrRef<EmailSubmissionId>],
    error: &SetError,
) -> SetResponse<EmailSubmission, EmailSubmissionId> {
    let mut response = SetResponse::new(ctx_account, None, old_state.clone());
    response.not_updated = update
        .keys()
        .map(|key| (key.clone(), error.clone()))
        .collect();
    response.not_destroyed = destroy
        .iter()
        .map(|key| (key.clone(), error.clone()))
        .collect();
    response
}

fn database_error() -> SetError {
    SetError::new(SetErrorType::Forbidden).with_description("database error")
}

/// Updates (an empty patch, or a cancel: only a pending submission can be
/// canceled) and destroys (forgetting a submission; not a pending one,
/// which would still go out). Never an error for the whole call: this runs
/// after messages went out.
async fn update_and_destroy_submissions(
    ctx: &Ctx<'_>,
    update: &SubmissionUpdates,
    destroy: &[IdOrRef<EmailSubmissionId>],
    old_state: &State,
) -> SetResponse<EmailSubmission, EmailSubmissionId> {
    if update.is_empty() && destroy.is_empty() {
        let new_state = state_or(ctx, DataType::EmailSubmission, old_state).await;
        return SetResponse::new(ctx.jmap_id(), None, new_state);
    }
    let existing = match Ctx::now() {
        Ok(now) => ctx.store().submissions(ctx.id(), None, now).await.ok(),
        Err(_) => None,
    };
    let Some(existing) = existing else {
        tracing::error!("the submissions could not be listed");
        let new_state = state_or(ctx, DataType::EmailSubmission, old_state).await;
        return degraded_response(
            ctx.jmap_id(),
            &new_state,
            update,
            destroy,
            &database_error(),
        );
    };
    let exists = |key: &IdOrRef<EmailSubmissionId>| {
        ctx.resolve(key)
            .as_ref()
            .and_then(ids::store_submission)
            .and_then(|row| existing.iter().find(|s| s.id == row))
    };
    let mut updated = BTreeMap::new();
    let mut not_updated = BTreeMap::new();
    for (key, patch) in update {
        let result = match (exists(key), EmailSubmissionPatch::parse(patch)) {
            (_, Err(error)) => Err(error),
            (None, Ok(_)) => Err(SetError::not_found()),
            (Some(found), Ok(EmailSubmissionPatch::Cancel)) => cancel(ctx, found.id).await,
            (Some(found), Ok(EmailSubmissionPatch::SendWhenSignedOut(on))) => {
                set_signed_out(ctx, found.id, on).await
            }
            (Some(found), Ok(_)) => ids::submission_id(found.id).map_err(bug),
        };
        match result {
            Ok(id) => {
                updated.insert(id, None);
            }
            Err(error) => {
                not_updated.insert(key.clone(), error);
            }
        }
    }
    let mut destroyed = Vec::new();
    let mut not_destroyed = BTreeMap::new();
    for key in destroy {
        let result = match exists(key) {
            Some(found) if matches!(found.undo, UndoState::Pending | UndoState::Sending) => {
                Err(SetError::new(SetErrorType::Forbidden)
                    .with_description("the message is still waiting to be sent: cancel it first"))
            }
            Some(found) => match ctx.store().destroy_submission(ctx.id(), found.id).await {
                Ok(_) => ids::submission_id(found.id).map_err(bug),
                Err(error) => Err(store_set_error(&error)),
            },
            None => Err(SetError::not_found()),
        };
        match result {
            Ok(id) => destroyed.push(id),
            Err(error) => {
                not_destroyed.insert(key.clone(), error);
            }
        }
    }

    let new_state = state_or(ctx, DataType::EmailSubmission, old_state).await;
    let mut response = SetResponse::new(ctx.jmap_id(), None, new_state);
    response.updated = updated;
    response.not_updated = not_updated;
    response.destroyed = destroyed;
    response.not_destroyed = not_destroyed;
    response
}

/// Cancels a held submission (`undoStatus: canceled`), if it is still
/// pending.
async fn cancel(ctx: &Ctx<'_>, row: SubmissionId) -> Result<EmailSubmissionId, SetError> {
    let now = Ctx::now().map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    let lifetime = ctx.config().submission_lifetime;
    match ctx.store().cancel_held(ctx.id(), row, now, lifetime).await {
        Ok(Cancel::Canceled(_)) => {
            tracing::info!(account = ctx.id().get(), "held message canceled");
            ids::submission_id(row).map_err(bug)
        }
        Ok(Cancel::AlreadyCanceled) => Err(SetError::new(SetErrorType::CannotUnsend)
            .with_description("the message was already canceled")),
        Ok(_) => Err(SetError::new(SetErrorType::CannotUnsend)
            .with_description("the message was already sent")),
        Err(error) => Err(store_set_error(&error)),
    }
}

/// Sets `sendWhenSignedOut` of a pending submission. Turning it off lets
/// the HTTP layer delete the password at its next cleanup if nothing else
/// needs it.
async fn set_signed_out(
    ctx: &Ctx<'_>,
    row: SubmissionId,
    on: bool,
) -> Result<EmailSubmissionId, SetError> {
    let on = check_signed_out(on, ctx.config().hold.sends_while_signed_out())?;
    match ctx
        .store()
        .set_held_send_when_signed_out(ctx.id(), row, on)
        .await
    {
        Ok(SetSignedOut::Set(_)) => {
            tracing::info!(
                account = ctx.id().get(),
                send_when_signed_out = on,
                "held message changed"
            );
            ids::submission_id(row).map_err(bug)
        }
        Ok(_) => Err(invalid(
            SEND_WHEN_SIGNED_OUT_PROPERTY,
            "the message is no longer waiting to be sent",
        )),
        Err(error) => Err(store_set_error(&error)),
    }
}

/// The implicit `Email/set` of `onSuccessUpdateEmail` and
/// `onSuccessDestroyEmail`, for the submissions sent in this call. Never
/// an error: `old_state` was read before sending.
async fn implicit_email_set(
    ctx: &Ctx<'_>,
    mail: &MailAccount,
    (on_update, on_destroy): (&SubmissionUpdates, &[IdOrRef<EmailSubmissionId>]),
    succeeded: &BTreeMap<IdOrRef<EmailSubmissionId>, JmapEmailId>,
    old_state: State,
) -> Option<SetResponse<Email, JmapEmailId>> {
    let email_updates: BTreeMap<IdOrRef<JmapEmailId>, PatchObject> = on_update
        .iter()
        .filter_map(|(key, patch)| {
            succeeded
                .get(key)
                .map(|email| (IdOrRef::Id(email.clone()), patch.clone()))
        })
        .collect();
    let email_destroys: Vec<IdOrRef<JmapEmailId>> = on_destroy
        .iter()
        .filter_map(|key| succeeded.get(key).map(|email| IdOrRef::Id(email.clone())))
        .collect();
    if email_updates.is_empty() && email_destroys.is_empty() {
        return None;
    }
    let changed = update_and_destroy(ctx, mail, &email_updates, &email_destroys).await;
    let new_state = state_or(ctx, DataType::Email, &old_state).await;
    let mut implicit = SetResponse::new(ctx.jmap_id(), Some(old_state), new_state);
    implicit.updated = changed.updated;
    implicit.not_updated = changed.not_updated;
    implicit.destroyed = changed.destroyed;
    implicit.not_destroyed = changed.not_destroyed;
    Some(implicit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(list: &[(&str, Option<&str>)]) -> SmtpParameters {
        list.iter()
            .map(|(name, value)| (name.parse().unwrap(), value.map(|v| v.parse().unwrap())))
            .collect()
    }

    #[test]
    fn mail_parameters() {
        let (dsn, hold) = mail_dsn(Some(&params(&[
            ("RET", Some("HDRS")),
            ("ENVID", Some("abc")),
            ("BODY", Some("8BITMIME")),
            ("SMTPUTF8", None),
        ])))
        .unwrap();
        assert!(dsn.ret.is_some() && dsn.envid.is_some());
        assert_eq!(hold, None);
        assert!(mail_dsn(None).unwrap().0.is_empty());
        // FUTURERELEASE: the bridge holds it; not a DSN parameter.
        let (dsn, hold) = mail_dsn(Some(&params(&[("HOLDFOR", Some("60"))]))).unwrap();
        assert!(dsn.is_empty());
        assert_eq!(hold, Some(HoldRequest::For(60)));
        let (_, hold) = mail_dsn(Some(&params(&[(
            "HOLDUNTIL",
            Some("2026-11-03T12:00:00Z"),
        )])))
        .unwrap();
        assert!(matches!(hold, Some(HoldRequest::Until(_))));
        for bad in [
            params(&[
                ("HOLDFOR", Some("60")),
                ("HOLDUNTIL", Some("2026-11-03T12:00:00Z")),
            ]),
            params(&[("HOLDFOR", Some("-1"))]),
            params(&[("HOLDFOR", None)]),
            params(&[("HOLDUNTIL", Some("mañana"))]),
            params(&[("AUTH", Some("bob@example.org"))]),
            params(&[("RET", Some("ALL"))]),
            params(&[("RET", None)]),
        ] {
            let error = mail_dsn(Some(&bad)).unwrap_err();
            assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        }
    }

    #[test]
    fn rcpt_parameters() {
        let dsn = rcpt_dsn(Some(&params(&[
            ("NOTIFY", Some("SUCCESS,FAILURE")),
            ("ORCPT", Some("rfc822;bob@example.org")),
        ])))
        .unwrap();
        assert!(dsn.notify.is_some() && dsn.orcpt.is_some());
        assert!(rcpt_dsn(Some(&params(&[("NOTIFY", Some("SOMETIMES"))]))).is_err());
        assert!(rcpt_dsn(Some(&params(&[("BODY", Some("7BIT"))]))).is_err());
    }

    fn authors(raw: &[u8]) -> Result<(), SetErrorType> {
        let message = OutgoingMessage::new(raw).unwrap();
        check_authors(&message, "yo@org").map_err(|e| e.kind())
    }

    #[test]
    fn only_the_identity_is_the_author() {
        let ok = |raw: &[u8]| assert_eq!(authors(raw), Ok(()), "{raw:?}");
        let from = |raw: &[u8]| {
            assert_eq!(authors(raw), Err(SetErrorType::ForbiddenFrom), "{raw:?}");
        };
        ok(b"From: yo@org\r\n\r\nhola");
        ok(b"From: \"Yo\" <YO@org>\r\nTo: a@b\r\n\r\n");
        ok(b"From: yo@org\r\nSender: yo@org\r\n\r\n");
        from(b"From: \"CEO\" <ceo@org>, yo@org\r\n\r\n");
        from(b"From: ceo@org\r\nFrom: yo@org\r\n\r\n");
        from(b"From: yo@org\rFrom: ceo@org\r\n\r\n");
        from(b"From: yo@org\r\nSender: ceo@org\r\n\r\n");
        from(b"From: yo@org\r\nSender: yo@org\r\nSender: yo@org\r\n\r\n");
        from(b"From: grupo: yo@org;\r\n\r\n");
        from(b"To: yo@org\r\n\r\nFrom: yo@org");
        from(b"From : ceo@org\r\nFrom: yo@org\r\n\r\n");
        from(b"fRoM: ceo@org\r\nFrom: yo@org\r\n\r\n");
        from(b"From: ceo@org,\r\n yo@org\r\n\r\n");
        // A From in the body is not a field.
        ok(b"From: yo@org\r\n\r\nFrom: ceo@org\r\n");
        assert_eq!(
            authors(b"From: yo@org\r\nbasura\r\nFrom: ceo@org\r\n\r\n"),
            Err(SetErrorType::InvalidEmail)
        );
    }

    #[test]
    fn send_when_signed_out_only_where_allowed() {
        assert_eq!(check_signed_out(true, true), Ok(true));
        assert_eq!(check_signed_out(false, true), Ok(false));
        assert_eq!(check_signed_out(false, false), Ok(false));
        let error = check_signed_out(true, false).unwrap_err();
        assert_eq!(error.kind(), SetErrorType::InvalidProperties);
        assert_eq!(error.properties(), [SEND_WHEN_SIGNED_OUT_PROPERTY]);
    }

    #[test]
    fn held_messages_are_bound_to_their_row() {
        let one = held_binding(SubmissionId::from_raw(1).unwrap());
        let other = held_binding(SubmissionId::from_raw(256).unwrap());
        assert_ne!(one, other);
        assert!(one.starts_with(b"submission\0"));
        assert_eq!(one.get(11..), Some(&1_i64.to_be_bytes()[..]));
    }

    #[test]
    fn sent_copy_after_the_update() {
        assert_eq!(sent_copy_plan(true, false), CopyPlan::IfUpdateFails);
        assert_eq!(sent_copy_plan(true, true), CopyPlan::Now);
        assert_eq!(sent_copy_plan(false, false), CopyPlan::Now);
        assert_eq!(sent_copy_plan(false, true), CopyPlan::Now);
    }

    #[test]
    fn degraded_keeps_the_old_state() {
        let old: State = serde_json::from_value(serde_json::json!("s7")).unwrap();
        let account: nebula_jmap_proto::AccountId =
            serde_json::from_value(serde_json::json!("a1")).unwrap();
        let key: IdOrRef<EmailSubmissionId> =
            serde_json::from_value(serde_json::json!("s1")).unwrap();
        let update = SubmissionUpdates::from([(key.clone(), PatchObject::default())]);
        let response = degraded_response(account, &old, &update, &[key], &database_error());
        assert_eq!(response.new_state, old);
        assert_eq!(response.not_updated.len(), 1);
        assert_eq!(response.not_destroyed.len(), 1);
        assert!(response.updated.is_empty() && response.destroyed.is_empty());
        let error = response.not_updated.values().next().unwrap();
        assert_eq!(error.kind(), SetErrorType::Forbidden);
    }
}
