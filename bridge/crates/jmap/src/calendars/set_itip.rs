// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Invitations sent by the bridge (servers
//! without `calendar-auto-schedule`, or `scheduling = "bridge"`): each
//! iTIP message of `nebula_groupware::model::itip` as iMIP (RFC 6047)
//! through the account's SMTP, the same path as `EmailSubmission/set`.
//!
//! As Stalwart does: one email per recipient (a refused address does not
//! stop the others), from the identity with the organizer's address, a
//! `text/plain` part and the `text/calendar; method=…` part. Unlike
//! Stalwart, no `Auto-Submitted`: every message here comes from the user's
//! own `/set` (the client asked with `sendSchedulingMessages`), as if
//! their mail client sent it, and RFC 3834 §5 keeps `auto-generated` for
//! messages of automatic processes ("often periodic", cron jobs) that are
//! no direct user action; absent is the same as `no` (§5). So the guests'
//! out-of-office replies (RFC 3834 §2, RFC 5230 §5) reach the organizer,
//! as for an invitation sent from a mail client. Sending happens after the server took
//! the write; a failure is logged (without addresses), the write stands.
//!
//! **Copy in Sent** (Stalwart keeps none of its
//! own invitations, but the web kept its proposals and the bridge keeps
//! what `EmailSubmission/set` sends): one per message, with `$seen`: the
//! email itself if it went to one person, else the same email with all
//! who got it in `To` (not one copy per guest); none if nothing went out.
//! The same rule as other mail: with `sent_copy = "server"` (Gmail…, the
//! server keeps what goes through its SMTP) nothing is appended, so there
//! is never a second copy; without a Sent mailbox, nothing (logged). A
//! failed copy is only logged.
//!
//! Headers never take a CR or LF from the event: the title and names go
//! through [`one_line`], addresses through `RcptTo`/`MailFrom` (no
//! control characters, spaces or angle brackets).

use std::fmt::Write as _;

use mail_builder::MessageBuilder;
use mail_builder::headers::HeaderType;
use mail_builder::headers::address::Address as BuilderAddress;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::date::Date;
use mail_builder::headers::message_id::MessageId;
use mail_builder::headers::text::Text;
use mail_builder::mime::{BodyPart, MimePart};
use nebula_groupware::model::itip::{ItipMessage, Method, Party, Subject};
use nebula_jmap_proto::{SetError, SetErrorType};
use nebula_smtp::{Envelope, MailFrom, OutgoingMessage, RcptTo};
use nebula_store::Identity;

use super::itip_text::Language;
use crate::core::context::Ctx;
use crate::identity::identities;
use crate::submission::send;
use crate::submission::set::{append_sent, sent_mailbox};

/// Most recipients of the messages of one write (Stalwart's
/// `maxParticipantsPerEvent`, also announced in the session).
pub(crate) const MAX_RECIPIENTS: usize = 20;
/// Most emails one write sends (each message to each of its recipients;
/// a series with overrides sends one message per override): the same
/// bound as the recipients of one `EmailSubmission` envelope.
pub(crate) const MAX_EMAILS: usize = 100;

/// Whether `messages` stay within [`MAX_RECIPIENTS`] different recipients
/// and [`MAX_EMAILS`] emails in all.
///
/// # Errors
/// `invalidProperties` on `participants` when they don't.
pub(crate) fn check_outgoing(messages: &[ItipMessage]) -> Result<(), SetError> {
    let mut unique: Vec<&str> = Vec::new();
    let mut emails = 0_usize;
    for party in messages.iter().flat_map(|m| m.to.iter()) {
        emails = emails.saturating_add(1);
        if !unique
            .iter()
            .any(|a| a.eq_ignore_ascii_case(&party.address))
        {
            unique.push(&party.address);
        }
        if unique.len() > MAX_RECIPIENTS || emails > MAX_EMAILS {
            return Err(
                SetError::invalid_properties(["participants"]).with_description(
                    "The number of scheduling message recipients exceeds the maximum allowed.",
                ),
            );
        }
    }
    Ok(())
}
/// Longest title in a subject, in characters.
const MAX_TITLE: usize = 200;

/// Whether the event is new or changed (only for the subject).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occasion {
    Created,
    Changed,
}

/// `text` on one line: every control character (CR, LF, tab…) becomes a
/// space, runs of spaces one, at most `max` characters.
pub(crate) fn one_line(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    let mut space = false;
    for c in text.chars() {
        let c = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        if c == ' ' {
            if space || out.is_empty() {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        out.push(c);
    }
    out.trim_end().chars().take(max).collect()
}

fn subject(message: &ItipMessage, occasion: Occasion, language: Language) -> String {
    let title = one_line(&message.title, MAX_TITLE);
    let texts = language.texts();
    let task = message.subject == Subject::Task;
    let prefix = match (message.method, occasion) {
        (Method::Cancel, _) if task => texts.task_cancelled,
        (Method::Request, Occasion::Created) if task => texts.task_assigned,
        (Method::Request, Occasion::Changed) if task => texts.task_updated,
        (Method::Cancel, _) => texts.cancelled,
        (Method::Request, Occasion::Created) => texts.invitation,
        (Method::Request, Occasion::Changed) => texts.updated,
        (Method::Counter, _) => texts.proposal,
        (Method::DeclineCounter, _) => texts.proposal_declined,
        _ => texts.calendar,
    };
    if title.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}: {title}")
    }
}

fn text_part(
    message: &ItipMessage,
    occasion: Occasion,
    from: &Identity,
    language: Language,
) -> String {
    let texts = language.texts();
    let proposal = matches!(message.method, Method::Counter | Method::DeclineCounter);
    let task = message.subject == Subject::Task;
    let (when_label, organizer_label, attached) = if task {
        (texts.due, texts.assigned_by, texts.task_attached)
    } else if message.method == Method::Counter {
        (texts.proposed_time, texts.organizer, texts.attached)
    } else {
        (texts.when, texts.organizer, texts.attached)
    };
    let mut text = subject(message, occasion, language);
    text.push_str("\r\n\r\n");
    if let Some(when) = &message.when {
        let _ = write!(text, "{when_label}: {}\r\n", one_line(when, 100));
    }
    // The sender: the attendee who proposes, else the organizer.
    let role = if message.method == Method::Counter {
        texts.from
    } else {
        organizer_label
    };
    let name = one_line(&from.name, 100);
    if name.is_empty() {
        let _ = write!(text, "{role}: {}\r\n", from.email.trim());
    } else {
        let _ = write!(text, "{role}: {name} <{}>\r\n", from.email.trim());
    }
    // The note, line by line (the body, never a header).
    if let Some(comment) = &message.comment {
        text.push_str("\r\n");
        for line in comment.lines() {
            let line: String = line.chars().filter(|c| !c.is_control()).collect();
            let _ = write!(text, "{line}\r\n");
        }
    }
    let last = if proposal {
        texts.attached_message
    } else {
        attached
    };
    let _ = write!(text, "\r\n{last}\r\n");
    text
}

/// A random `Message-ID` at the identity's domain.
fn message_id(identity: &Identity) -> Result<String, SetError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SetError::new(SetErrorType::Forbidden))?;
    let mut local = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(local, "{byte:02x}");
    }
    let domain = identity
        .email
        .rsplit_once('@')
        .map(|(_, d)| d.trim())
        .filter(|d| {
            !d.is_empty()
                && d.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
        .unwrap_or("nebula.invalid");
    Ok(format!("{local}.itip@{domain}"))
}

/// The email of one message to one recipient (RFC 6047 §2).
pub(crate) fn build(
    message: &ItipMessage,
    occasion: Occasion,
    from: &Identity,
    to: &Party,
    now: i64,
    language: Language,
) -> Result<Vec<u8>, SetError> {
    build_to(
        message,
        occasion,
        from,
        std::slice::from_ref(to),
        now,
        language,
    )
}

/// The copy for Sent of a message that went to several recipients: the
/// same email, with all of them in `To` (one copy, not one per guest).
pub(crate) fn sent_copy(
    message: &ItipMessage,
    occasion: Occasion,
    from: &Identity,
    to: &[Party],
    now: i64,
    language: Language,
) -> Result<Vec<u8>, SetError> {
    build_to(message, occasion, from, to, now, language)
}

fn build_to(
    message: &ItipMessage,
    occasion: Occasion,
    from: &Identity,
    to: &[Party],
    now: i64,
    language: Language,
) -> Result<Vec<u8>, SetError> {
    let mut recipients = Vec::with_capacity(to.len());
    for party in to {
        let rcpt: RcptTo = party
            .address
            .parse()
            .map_err(|_| SetError::invalid_recipients(vec![party.address.clone()]))?;
        let name = party
            .name
            .as_deref()
            .map(|n| one_line(n, 100))
            .filter(|n| !n.is_empty());
        recipients.push(BuilderAddress::new_address(name, rcpt.as_str().to_owned()));
    }
    if recipients.is_empty() {
        return Err(SetError::new(SetErrorType::NoRecipients));
    }
    let sender: MailFrom = from
        .email
        .trim()
        .parse()
        .map_err(|_| SetError::new(SetErrorType::ForbiddenMailFrom))?;
    let name = one_line(&from.name, 100);
    let from_address = if name.is_empty() {
        BuilderAddress::new_address(None::<String>, sender.as_str().to_owned())
    } else {
        BuilderAddress::new_address(Some(name), sender.as_str().to_owned())
    };
    let to_address = if recipients.len() == 1 {
        recipients.remove(0)
    } else {
        BuilderAddress::new_list(recipients)
    };
    let body = MimePart::new(
        ContentType::new("multipart/alternative"),
        BodyPart::Multipart(vec![
            MimePart::new(
                ContentType::new("text/plain").attribute("charset", "utf-8"),
                BodyPart::Text(text_part(message, occasion, from, language).into()),
            ),
            MimePart::new(
                ContentType::new("text/calendar")
                    .attribute("method", message.method.as_str())
                    .attribute("charset", "utf-8"),
                BodyPart::Text(message.ics.clone().into()),
            ),
        ]),
    );
    MessageBuilder::new()
        .from(from_address)
        .to(to_address)
        .subject(Text::new(subject(message, occasion, language)))
        .date(Date::new(now))
        .message_id(MessageId::new(message_id(from)?))
        .header("Content-Language", HeaderType::Text(language.tag().into()))
        .body(body)
        .write_to_vec()
        .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("internal error"))
}

/// The identity that sends as the organizer (`address` without
/// `mailto:`): the one with that address.
///
/// # Errors
/// `forbiddenFrom` if the user has no identity with that address (the
/// write is refused before anything is changed).
pub(crate) async fn sender(ctx: &Ctx<'_>, address: &str) -> Result<Identity, SetError> {
    let all = identities(ctx)
        .await
        .map_err(|_| SetError::new(SetErrorType::Forbidden).with_description("database error"))?;
    all.into_iter()
        .find(|i| i.email.trim().eq_ignore_ascii_case(address.trim()))
        .ok_or_else(|| {
            SetError::new(SetErrorType::ForbiddenFrom)
                .with_description("no identity has the organizer's address")
        })
}

/// A message that went out: each recipient with the email they got.
type Went<'a> = (&'a ItipMessage, Vec<(&'a Party, Vec<u8>)>);

/// Sends every message to each of its recipients, and keeps one copy of
/// each message in Sent (see the module docs). Returns how many emails
/// went out.
pub(crate) async fn send_all(
    ctx: &Ctx<'_>,
    from: &Identity,
    messages: &[ItipMessage],
    occasion: Occasion,
) -> usize {
    // Every caller checks first; this is the last guard for any new path.
    if check_outgoing(messages).is_err() {
        tracing::warn!(
            account = ctx.id().get(),
            "itip: too many invitation emails for one write; nothing sent"
        );
        return 0;
    }
    let Some(login) = ctx.inner.smtp_login(ctx.id()) else {
        tracing::warn!(
            account = ctx.id().get(),
            "itip: not connected to SMTP; nothing sent"
        );
        return 0;
    };
    let Ok(now) = Ctx::now() else {
        return 0;
    };
    // The organizer's language: see `itip_text`.
    let language = Language::for_request(&ctx.languages);
    let mut sent = 0_usize;
    // Per message: the email that went out to each recipient.
    let mut copies: Vec<Went<'_>> = Vec::new();
    for message in messages {
        let mut went = Vec::new();
        for to in &message.to {
            let result = async {
                let raw = build(message, occasion, from, to, now.secs(), language)?;
                let rcpt: RcptTo = to
                    .address
                    .parse()
                    .map_err(|_| SetError::invalid_recipients(vec![]))?;
                let mail_from: MailFrom = from
                    .email
                    .trim()
                    .parse()
                    .map_err(|_| SetError::new(SetErrorType::ForbiddenMailFrom))?;
                let envelope = Envelope::new(mail_from, vec![rcpt])
                    .map_err(|_| SetError::new(SetErrorType::NoRecipients))?;
                let outgoing = OutgoingMessage::new(&raw)
                    .map_err(|_| SetError::new(SetErrorType::InvalidEmail))?;
                let mut lease = crate::email::build::lease(ctx)?;
                send::send(
                    ctx.inner,
                    &login,
                    &envelope,
                    std::sync::Arc::new(outgoing),
                    &mut lease,
                    None,
                )
                .await
                .map_err(|e| send::set_error(&e))?;
                Ok::<_, SetError>(raw)
            }
            .await;
            match result {
                Ok(raw) => {
                    sent = sent.saturating_add(1);
                    went.push((to, raw));
                }
                Err(error) => tracing::warn!(
                    account = ctx.id().get(),
                    method = message.method.as_str(),
                    kind = ?error.kind(),
                    "itip: an invitation email was not sent"
                ),
            }
        }
        if !went.is_empty() {
            copies.push((message, went));
        }
    }
    drop(login);
    tracing::info!(
        account = ctx.id().get(),
        sent,
        "itip: invitation emails sent"
    );
    save_copies(ctx, from, copies, occasion, (now.secs(), language)).await;
    sent
}

/// One copy in Sent per message that went out: the email itself if it
/// had one recipient, else the same email to all who got it. Nothing with
/// `sent_copy = "server"` (the server keeps what goes through its SMTP).
async fn save_copies(
    ctx: &Ctx<'_>,
    from: &Identity,
    copies: Vec<Went<'_>>,
    occasion: Occasion,
    (now, language): (i64, Language),
) {
    if copies.is_empty() {
        return;
    }
    let Some(sent) = sent_mailbox(ctx).await else {
        return;
    };
    let Ok(mail) = ctx.mail() else {
        tracing::warn!("itip: not connected to the mail server; no copy in Sent");
        return;
    };
    for (message, mut went) in copies {
        let copy = if went.len() == 1 {
            went.pop().map(|(_, raw)| raw)
        } else {
            let to: Vec<Party> = went.iter().map(|(party, _)| (*party).clone()).collect();
            sent_copy(message, occasion, from, &to, now, language).ok()
        };
        if let Some(copy) = copy {
            append_sent(&mail, sent, &copy).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use nebula_store::IdentityId;

    use super::*;

    fn identity() -> Identity {
        Identity {
            id: IdentityId::from_raw(1).unwrap(),
            name: "Alice\r\nBcc: evil@example.net".to_owned(),
            email: "alice@example.com".to_owned(),
            reply_to: None,
            bcc: None,
            text_signature: String::new(),
            html_signature: String::new(),
            may_delete: false,
        }
    }

    fn message(title: &str) -> ItipMessage {
        ItipMessage {
            method: Method::Request,
            subject: Subject::Event,
            organizer: "alice@example.com".to_owned(),
            to: vec![Party {
                address: "bob@example.com".to_owned(),
                name: Some("Bob\nX-Evil: 1".to_owned()),
            }],
            ics: "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n".to_owned(),
            title: title.to_owned(),
            when: Some("2026-10-19 10:00 (Europe/Madrid)".to_owned()),
            comment: None,
        }
    }

    fn headers(raw: &[u8]) -> Vec<String> {
        let text = String::from_utf8_lossy(raw);
        let head = text.split("\r\n\r\n").next().unwrap_or_default().to_owned();
        head.split("\r\n").map(str::to_owned).collect()
    }

    #[test]
    fn one_line_strips_breaks() {
        assert_eq!(one_line("a\r\nBcc: x\t y  z", 100), "a Bcc: x y z");
        assert_eq!(one_line("  x  ", 100), "x");
        assert_eq!(one_line(&"é".repeat(300), 200).chars().count(), 200);
    }

    /// A title, an organizer name or an attendee name with CR/LF never
    /// starts a header of its own.
    #[test]
    fn no_header_injection() {
        let message = message("Sync\r\nBcc: evil@example.net\r\nX-Injected: yes");
        let raw = build(
            &message,
            Occasion::Created,
            &identity(),
            &message.to[0],
            0,
            Language::English,
        )
        .unwrap();
        let lines = headers(&raw);
        for line in &lines {
            let lower = line.to_ascii_lowercase();
            assert!(
                !lower.starts_with("bcc:")
                    && !lower.starts_with("x-injected:")
                    && !lower.starts_with("x-evil:"),
                "{lines:?}"
            );
        }
        let text = String::from_utf8_lossy(&raw);
        // A user's own action, not an automatic process (RFC 3834 §5).
        assert!(!text.contains("Auto-Submitted"), "{text}");
        assert!(
            text.contains("method=REQUEST") || text.contains("method=\"REQUEST\""),
            "{text}"
        );
        assert!(text.contains("text/plain"));
        // An address with a line break is refused.
        let bad = Party {
            address: "bob@example.com\r\nBcc: evil@example.net".to_owned(),
            name: None,
        };
        assert_eq!(
            build(
                &message,
                Occasion::Created,
                &identity(),
                &bad,
                0,
                Language::English
            )
            .unwrap_err()
            .kind(),
            SetErrorType::InvalidRecipients
        );
    }

    /// The Sent copy of a message to several guests: one email with all
    /// of them in `To`, the same subject and calendar part; nobody with a
    /// line break gets in.
    #[test]
    fn one_sent_copy_for_all_guests() {
        let mut m = message("Sync");
        m.to.push(Party {
            address: "carol@example.com".to_owned(),
            name: Some("Carol".to_owned()),
        });
        let raw = sent_copy(
            &m,
            Occasion::Created,
            &identity(),
            &m.to,
            0,
            Language::English,
        )
        .unwrap();
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
        let to: Vec<&str> = parsed
            .to()
            .unwrap()
            .mailboxes()
            .filter_map(|a| a.address())
            .collect();
        assert_eq!(to, ["bob@example.com", "carol@example.com"]);
        assert_eq!(parsed.subject(), Some("Invitation: Sync"));
        assert!(String::from_utf8_lossy(&raw).contains("METHOD:REQUEST"));
        for line in headers(&raw) {
            assert!(!line.to_ascii_lowercase().starts_with("x-evil:"), "{line}");
        }
        // No recipients, or one that is not an address: no copy.
        assert_eq!(
            sent_copy(
                &m,
                Occasion::Created,
                &identity(),
                &[],
                0,
                Language::English
            )
            .unwrap_err()
            .kind(),
            SetErrorType::NoRecipients
        );
        let bad = [Party {
            address: "x@example.com\r\nBcc: evil@example.net".to_owned(),
            name: None,
        }];
        assert!(
            sent_copy(
                &m,
                Occasion::Created,
                &identity(),
                &bad,
                0,
                Language::English
            )
            .is_err()
        );
    }

    #[test]
    fn subjects() {
        let en = Language::English;
        let mut m = message("Sync");
        assert_eq!(subject(&m, Occasion::Created, en), "Invitation: Sync");
        assert_eq!(
            subject(&m, Occasion::Changed, en),
            "Updated invitation: Sync"
        );
        m.method = Method::Cancel;
        assert_eq!(subject(&m, Occasion::Changed, en), "Cancelled: Sync");
        m.title = String::new();
        assert_eq!(subject(&m, Occasion::Changed, en), "Cancelled");
    }

    /// A task: assigned, updated and
    /// taken back, with its due date and who assigned it.
    #[test]
    fn task_texts() {
        let mut m = message("Informe");
        m.subject = Subject::Task;
        m.when = Some("2026-11-06".to_owned());
        let es = Language::Spanish;
        assert_eq!(
            subject(&m, Occasion::Created, es),
            "Tarea asignada: Informe"
        );
        assert_eq!(
            subject(&m, Occasion::Changed, Language::English),
            "Task updated: Informe"
        );
        let mut alice = identity();
        alice.name = "Alice".to_owned();
        assert_eq!(
            text_part(&m, Occasion::Created, &alice, es),
            "Tarea asignada: Informe\r\n\r\nFecha límite: 2026-11-06\r\n\
             Asignada por: Alice <alice@example.com>\r\n\r\n\
             La tarea va adjunta (text/calendar).\r\n"
        );
        m.method = Method::Cancel;
        assert_eq!(
            subject(&m, Occasion::Changed, Language::English),
            "Task cancelled: Informe"
        );
    }

    /// The organizer's language: Spanish texts, the same calendar part.
    #[test]
    fn spanish() {
        let es = Language::Spanish;
        let mut m = message("Reunión");
        assert_eq!(subject(&m, Occasion::Created, es), "Invitación: Reunión");
        assert_eq!(
            subject(&m, Occasion::Changed, es),
            "Invitación actualizada: Reunión"
        );
        let mut alice = identity();
        alice.name = "Alice".to_owned();
        let text = text_part(&m, Occasion::Created, &alice, es);
        assert_eq!(
            text,
            "Invitación: Reunión\r\n\r\nCuándo: 2026-10-19 10:00 (Europe/Madrid)\r\n\
             Organizador: Alice <alice@example.com>\r\n\r\n\
             La invitación de calendario va adjunta (text/calendar).\r\n"
        );
        let raw = build(&m, Occasion::Created, &alice, &m.to[0], 0, es).unwrap();
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.subject(), Some("Invitación: Reunión"));
        assert_eq!(parsed.body_text(0).as_deref(), Some(text.as_str()));
        assert!(String::from_utf8_lossy(&raw).contains("Content-Language: es"));
        assert!(String::from_utf8_lossy(&raw).contains("METHOD:REQUEST"));
        m.method = Method::Cancel;
        assert_eq!(subject(&m, Occasion::Changed, es), "Cancelado: Reunión");
        let english = text_part(&m, Occasion::Changed, &alice, Language::English);
        assert!(english.starts_with("Cancelled: Reunión\r\n\r\nWhen: "));
        assert!(english.ends_with("The calendar invitation is attached (text/calendar).\r\n"));
    }

    /// A proposal (COUNTER) and its refusal: their subjects and texts, in
    /// both languages, with the note in the body and nowhere else.
    #[test]
    fn proposals() {
        let mut bob = identity();
        bob.name = "Bob".to_owned();
        bob.email = "bob@example.com".to_owned();
        let mut m = message("Sync");
        m.method = Method::Counter;
        m.when = Some("2026-10-20 08:00 (UTC)".to_owned());
        m.comment = Some("After lunch?\nBcc: evil@example.net".to_owned());
        let en = text_part(&m, Occasion::Changed, &bob, Language::English);
        assert_eq!(
            en,
            "New time proposed: Sync\r\n\r\nProposed time: 2026-10-20 08:00 (UTC)\r\n\
             From: Bob <bob@example.com>\r\n\r\nAfter lunch?\r\nBcc: evil@example.net\r\n\r\n\
             The calendar message is attached (text/calendar).\r\n"
        );
        let es = text_part(&m, Occasion::Changed, &bob, Language::Spanish);
        assert!(
            es.starts_with("Nueva hora propuesta: Sync\r\n\r\nHora propuesta: "),
            "{es}"
        );
        assert!(es.contains("De: Bob <bob@example.com>"), "{es}");
        assert!(es.ends_with("El mensaje de calendario va adjunto (text/calendar).\r\n"));
        let raw = build(&m, Occasion::Changed, &bob, &m.to[0], 0, Language::English).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(
            text.contains("method=COUNTER") || text.contains("method=\"COUNTER\""),
            "{text}"
        );
        assert!(
            !headers(&raw)
                .iter()
                .any(|l| l.to_ascii_lowercase().starts_with("bcc:"))
        );
        m.method = Method::DeclineCounter;
        m.when = None;
        assert_eq!(
            subject(&m, Occasion::Changed, Language::English),
            "Proposal declined: Sync"
        );
        assert_eq!(
            subject(&m, Occasion::Changed, Language::Spanish),
            "Propuesta rechazada: Sync"
        );
        let decline = text_part(&m, Occasion::Changed, &bob, Language::English);
        assert!(decline.contains("Organizer: Bob"), "{decline}");
        assert!(!decline.contains("When"), "{decline}");
    }

    fn invitation(guests: usize) -> ItipMessage {
        let mut message = message("Sync");
        message.to = (0..guests)
            .map(|i| Party {
                address: format!("guest{i}@example.com"),
                name: None,
            })
            .collect();
        message
    }

    /// At most 20 different recipients and 100 emails per write: one
    /// message per override counts once per recipient.
    #[test]
    fn outgoing_limits() {
        assert!(check_outgoing(&[invitation(MAX_RECIPIENTS)]).is_ok());
        let error = check_outgoing(&[invitation(MAX_RECIPIENTS + 1)]).unwrap_err();
        assert_eq!(error.properties(), ["participants"]);
        let five: Vec<ItipMessage> = (0..5).map(|_| invitation(20)).collect();
        assert!(check_outgoing(&five).is_ok());
        let six: Vec<ItipMessage> = (0..6).map(|_| invitation(20)).collect();
        assert!(check_outgoing(&six).is_err());
    }

    /// Deleting an event with too many guests: the cancellations break the
    /// limit (checked before anything is deleted).
    #[test]
    fn cancelling_many_guests_is_refused() {
        let address = |user: &str| format!("{user}@example.com");
        let guests = (0..25).fold(String::new(), |mut out, i| {
            out.push_str("ATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:");
            out.push_str(&address(&format!("g{i}")));
            out.push_str("\r\n");
            out
        });
        let alice = address("alice");
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//t//EN\r\nBEGIN:VEVENT\r\n\
             UID:e1\r\nDTSTAMP:20261001T000000Z\r\nSEQUENCE:1\r\n\
             DTSTART:20261019T100000Z\r\nDURATION:PT1H\r\nSUMMARY:Sync\r\n\
             ORGANIZER:mailto:{alice}\r\n{guests}END:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let plan = nebula_groupware::model::itip::organizer_messages(
            Some(text.as_bytes()),
            None,
            &[alice],
            0,
        )
        .unwrap();
        assert_eq!(plan.recipients().len(), 25);
        assert!(check_outgoing(&plan.messages).is_err());
    }
}
