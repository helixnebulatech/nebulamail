// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! One SMTP submission: `net-guard` connection pinned to a checked
//! address, TLS (implicit or required STARTTLS, in `nebula-smtp`), `AUTH`
//! with the account's own login and password, the message, `QUIT`.
//!
//! A new connection per message: the web app sends one at a time, and a
//! pooled connection would keep credentials in lettre for longer.
//!
//! 8 bits: decided with the `EHLO` of this same connection. With 8BITMIME
//! the message goes as it is (`BODY=8BITMIME`); without it, each 8-bit
//! `message/rfc822` (or `message/global`) part becomes a `message/global`
//! in base64 ([`prepared_for`]) and, if 8 bits remain elsewhere, the send
//! fails as before (`invalidEmail`). The copy kept in Sent is the original.

use std::sync::Arc;

use nebula_jmap_proto::{SetError, SetErrorType, UnsignedInt};
use nebula_mail::{Lease, MailError};
use nebula_smtp::{
    Credentials, Envelope, EnvelopeId, OutgoingMessage, Sent, ServerExtensions, SmtpError, Stage,
};
use net_guard::GuardError;

use crate::core::backend::{Inner, SmtpLogin};
use crate::email::encapsulate::transferable;

/// Why a message wasn't sent.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SendError {
    #[error(transparent)]
    Connect(#[from] GuardError),
    #[error(transparent)]
    Smtp(#[from] SmtpError),
    /// No memory for the 7-bit copy in time (retryable).
    #[error(transparent)]
    Busy(MailError),
    /// A line over 998 octets (RFC 5322 §2.1.1) that re-encoding couldn't
    /// fix (in a header field, or an unparsable message): it can't be sent
    /// as it is.
    #[error("a line is longer than 998 octets")]
    LineTooLong,
}

/// Sends `message`. `own_envid` is the `ENVID` to use when the envelope has
/// none, if this connection's `EHLO` announces DSN (decided in
/// `nebula-smtp` from that fresh list, not from the one kept for the
/// session); [`Sent::envid`] says what was sent.
///
/// A message with 8-bit content goes as it is when the server announces
/// 8BITMIME (on this connection); else it is first made 7-bit
/// ([`prepared_for`]), its copy counted in `lease`.
pub(crate) async fn send(
    inner: &Inner,
    login: &SmtpLogin,
    envelope: &Envelope,
    message: Arc<OutgoingMessage>,
    lease: &mut Lease,
    own_envid: Option<EnvelopeId>,
) -> Result<Sent, SendError> {
    let smtp = &inner.config.smtp;
    let stream = net_guard::connect(&smtp.target, &smtp.policy, inner.resolver.as_ref()).await?;
    let credentials = Credentials::new(login.login.clone(), login.password.clone())?;
    let session = smtp.client.connect(stream.into_stream()).await?;
    inner.set_smtp_extensions(session.extensions().clone());
    let mut session = session.authenticate(&credentials).await?;
    drop(credentials);
    let message = prepared_for(message, session.accepts_8bit(), lease).await?;
    let sent = session.send_tracked(envelope, &message, own_envid).await;
    session.quit().await;
    Ok(sent?)
}

/// The message as it can go to this server (`accepts_8bit`, decided as
/// `nebula-smtp` decides `BODY=8BITMIME`): the same one if it already
/// can; else the parts that can't are re-encoded ([`transferable`]: a NUL
/// or a line over 998 octets, RFC 5322 §2.1.1 and RFC 6152 §3, or 8 bits
/// for a server without 8BITMIME), the copy counted in `lease`. If that
/// isn't possible (a header field over 998 octets, 8-bit header fields
/// without 8BITMIME), [`SmtpError::EightBitMimeUnsupported`] or
/// [`SendError::LineTooLong`]: never sent broken.
pub(crate) async fn prepared_for(
    message: Arc<OutgoingMessage>,
    accepts_8bit: bool,
    lease: &mut Lease,
) -> Result<Arc<OutgoingMessage>, SendError> {
    // Both checks read the whole message (up to tens of MB): off the
    // runtime, like the conversion.
    let checked = Arc::clone(&message);
    let (needs_8bit, breaks_limits) = tokio::task::spawn_blocking(move || {
        (
            !accepts_8bit && checked.is_8bit(),
            checked.breaks_line_limits(),
        )
    })
    .await
    // The closure can't panic; if the task fails anyway, convert (a
    // conversion that isn't needed only costs time).
    .unwrap_or((!accepts_8bit, true));
    if !needs_8bit && !breaks_limits {
        return Ok(message);
    }
    // base64 and its line breaks: about 1.4 times the parts it encodes.
    let len = u64::try_from(message.as_bytes().len()).unwrap_or(u64::MAX);
    lease
        .reserve(len.saturating_mul(7) / 5)
        .await
        .map_err(SendError::Busy)?;
    let converted = tokio::task::spawn_blocking(move || {
        transferable(message.as_bytes(), accepts_8bit)
            .and_then(|raw| OutgoingMessage::new(&raw).ok())
    })
    .await
    .ok()
    .flatten();
    converted.map(Arc::new).ok_or(if needs_8bit {
        SendError::Smtp(SmtpError::EightBitMimeUnsupported)
    } else {
        SendError::LineTooLong
    })
}

/// Connects without logging in, to learn the server's extensions (for
/// `submissionExtensions` in the session). Run once at start.
pub(crate) async fn probe(inner: &Inner) -> Result<ServerExtensions, SendError> {
    let smtp = &inner.config.smtp;
    let stream = net_guard::connect(&smtp.target, &smtp.policy, inner.resolver.as_ref()).await?;
    let session = smtp.client.connect(stream.into_stream()).await?;
    let extensions = session.extensions().clone();
    // RFC 5321 §4.1.1.10: QUIT before closing, also here.
    session.quit().await;
    Ok(extensions)
}

/// Whether the message may have gone out anyway (see
/// [`SmtpError::outcome_unknown`]).
pub(crate) fn outcome_unknown(error: &SendError) -> bool {
    matches!(error, SendError::Smtp(error) if error.outcome_unknown())
}

/// The JMAP error for a failed send (RFC 8621 §7.5).
///
/// `SetError` has no "try again later" type, so temporary failures
/// (network, timeouts, 4xx replies) are `forbiddenToSend` with a
/// description that says so. Nothing was sent, except for an
/// [unknown outcome](SmtpError::outcome_unknown) (the connection failed
/// after the message was written): that one says the message may have gone
/// out, and is never "temporary", so nobody sends it twice.
pub(crate) fn set_error(error: &SendError) -> SetError {
    let temporary = |what: String| {
        SetError::new(SetErrorType::ForbiddenToSend)
            .with_description(format!("temporary failure, try again later: {what}"))
    };
    let error = match error {
        SendError::Connect(error) => {
            tracing::warn!(error = %error, "SMTP connection failed");
            return temporary("the mail server can't be reached".to_owned());
        }
        SendError::Busy(_) => {
            tracing::warn!("bridge busy with other large messages");
            return temporary("the server is busy with other large messages".to_owned());
        }
        SendError::LineTooLong => {
            return SetError::new(SetErrorType::InvalidEmail).with_description(
                "a line is longer than 998 octets (RFC 5322 §2.1.1) and the message could not be re-encoded",
            );
        }
        SendError::Smtp(error) => error,
    };
    tracing::warn!(error = %error, "SMTP submission failed");
    match error {
        SmtpError::RecipientsRejected { rejected } => {
            if rejected.iter().all(|r| !r.code.is_permanent()) {
                return temporary(format!("{} recipient(s) deferred", rejected.len()));
            }
            SetError::invalid_recipients(rejected.iter().map(|r| r.recipient.to_string()).collect())
        }
        SmtpError::RecipientCount { max } => {
            SetError::too_many_recipients(UnsignedInt::try_from(*max).unwrap_or(UnsignedInt::MAX))
        }
        // RFC 8621 §7.5 `tooLarge`, with `maxSize` when the server said it
        // (`SIZE`, RFC 1870). Before the `MAIL FROM` branch: a 552 there
        // with SIZE is the size, not the sender.
        SmtpError::TooLargeForServer { maximum } => SetError::too_large_to_send(
            UnsignedInt::try_from(*maximum).unwrap_or(UnsignedInt::MAX),
        ),
        SmtpError::Rejected {
            permanent: true,
            text,
            ..
        } if error.is_too_large() => {
            SetError::new(SetErrorType::TooLarge).with_description(text.to_string())
        }
        SmtpError::Rejected {
            stage: Stage::MailFrom,
            permanent: true,
            text,
            ..
        } => SetError::new(SetErrorType::ForbiddenMailFrom).with_description(text.to_string()),
        SmtpError::Rejected {
            permanent: true,
            text,
            ..
        } => SetError::new(SetErrorType::ForbiddenToSend).with_description(text.to_string()),
        SmtpError::Auth { code, .. } if !code.is_permanent() => {
            temporary("authentication deferred".to_owned())
        }
        SmtpError::Auth { .. } | SmtpError::AuthUnavailable => {
            SetError::new(SetErrorType::ForbiddenToSend)
                .with_description("the SMTP server refused the login")
        }
        SmtpError::SmtpUtf8Unsupported => SetError::invalid_recipients(Vec::new())
            .with_description("the server can't take non-ASCII addresses"),
        SmtpError::SmtpUtf8HeadersUnsupported => SetError::new(SetErrorType::InvalidEmail)
            .with_description(
                "the server can't take non-ASCII header fields (it doesn't support SMTPUTF8)",
            ),
        SmtpError::EightBitMimeUnsupported => SetError::new(SetErrorType::InvalidEmail)
            .with_description("the server can't take 8-bit messages"),
        SmtpError::DsnUnsupported => SetError::invalid_properties(["envelope"])
            .with_description("the SMTP server does not support DSN parameters"),
        other if other.outcome_unknown() => SetError::new(SetErrorType::ForbiddenToSend)
            .with_description(
                "the server did not confirm the message: it may have been sent; check before sending again",
            ),
        other if other.is_transient() => temporary(other.to_string()),
        _ => SetError::new(SetErrorType::ForbiddenToSend)
            .with_description("the message could not be sent"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_failures_say_so() {
        let error = SendError::Smtp(SmtpError::Network { stage: Stage::Data });
        let set = set_error(&error);
        assert_eq!(set.kind(), SetErrorType::ForbiddenToSend);
        assert!(set.description().unwrap().starts_with("temporary"));
        let error = SendError::Smtp(SmtpError::RecipientCount { max: 100 });
        assert_eq!(set_error(&error).kind(), SetErrorType::TooManyRecipients);
    }

    #[test]
    fn unknown_outcome_is_not_temporary() {
        let error = SendError::Smtp(SmtpError::Timeout {
            stage: Stage::DataEnd,
            after: std::time::Duration::from_secs(120),
        });
        let set = set_error(&error);
        assert_eq!(set.kind(), SetErrorType::ForbiddenToSend);
        let description = set.description().unwrap();
        assert!(!description.starts_with("temporary"), "{description}");
        assert!(description.contains("may have been sent"), "{description}");
    }

    fn lease() -> (Arc<nebula_mail::MemoryBudget>, Lease) {
        let budget = nebula_mail::MemoryBudget::new(
            16 * 1024 * 1024,
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(30),
        );
        let lease = budget.lease();
        (budget, lease)
    }

    const FORWARD: &str = "From: a@x\r\nSubject: Fwd\r\n\
        Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
        --b\r\nContent-Type: text/plain\r\n\r\nmira\r\n\
        --b\r\nContent-Type: message/rfc822\r\nContent-Transfer-Encoding: 8bit\r\n\r\n\
        Subject: Hola\r\n\r\nÑandú\r\n--b--\r\n";

    #[tokio::test]
    async fn eight_bits_only_when_needed() {
        let (_budget, mut lease) = lease();
        let message = Arc::new(OutgoingMessage::new(FORWARD.as_bytes()).unwrap());
        // 8BITMIME: as it is.
        let same = prepared_for(Arc::clone(&message), true, &mut lease)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&same, &message));
        assert_eq!(lease.bytes(), 0);
        // ASCII: as it is, even without 8BITMIME.
        let ascii = Arc::new(OutgoingMessage::new(b"Subject: a\r\n\r\nb\r\n").unwrap());
        let same = prepared_for(Arc::clone(&ascii), false, &mut lease)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&same, &ascii));
    }

    #[tokio::test]
    async fn without_8bitmime_the_forward_goes_in_base64() {
        let (_budget, mut lease) = lease();
        let message = Arc::new(OutgoingMessage::new(FORWARD.as_bytes()).unwrap());
        let seven = prepared_for(message, false, &mut lease).await.unwrap();
        assert!(!seven.is_8bit());
        let text = String::from_utf8_lossy(seven.as_bytes());
        assert!(text.contains("Content-Type: message/global"), "{text}");
        assert!(
            text.contains("--b\r\nContent-Type: text/plain\r\n\r\nmira\r\n"),
            "{text}"
        );
        // Its copy is counted.
        assert!(lease.bytes() >= u64::try_from(FORWARD.len()).unwrap());
    }

    #[tokio::test]
    async fn other_8bit_still_fails() {
        let (_budget, mut lease) = lease();
        let raw = FORWARD.replace("Subject: Fwd", "Subject: Reenvío");
        let message = Arc::new(OutgoingMessage::new(raw.as_bytes()).unwrap());
        let error = prepared_for(message, false, &mut lease).await.unwrap_err();
        assert!(matches!(
            error,
            SendError::Smtp(SmtpError::EightBitMimeUnsupported)
        ));
        assert_eq!(set_error(&error).kind(), SetErrorType::InvalidEmail);
    }
}
