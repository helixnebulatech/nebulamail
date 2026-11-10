// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use lettre::transport::smtp::commands::{Data, Mail, Rcpt, Rset};
use lettre::transport::smtp::extension::{
    Extension, MailBodyParameter, MailParameter, RcptParameter,
};

use crate::address::Envelope;
use crate::core::session::{self, AuthenticatedSession, ServerReply, timed};
use crate::dsn::{EnvelopeId, RcptDsn};
use crate::error::{self, RecipientRejection, ReplyCode, SmtpError, Stage};
use crate::message::OutgoingMessage;

/// A message the server took.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Sent {
    /// The server's reply to the message.
    pub reply: ServerReply,
    /// The `ENVID` sent with `MAIL FROM`, if any: the envelope's or the
    /// caller's own (see [`AuthenticatedSession::send_tracked`]).
    pub envid: Option<EnvelopeId>,
}

impl AuthenticatedSession {
    /// Sends one message: `MAIL FROM`, a `RCPT TO` per recipient, `DATA`.
    ///
    /// lettre's own `send` stops at the first refused recipient; here every
    /// recipient is tried so the error lists all the refused ones. If any is
    /// refused, the transaction is cancelled (`RSET`) and nothing is sent:
    /// sending to only some recipients would surprise the user, and sending
    /// again after fixing the address would duplicate the message for the
    /// others. This matches JMAP's `invalidRecipients` error.
    ///
    /// # Errors
    ///
    /// [`SmtpError::RecipientsRejected`], [`SmtpError::Rejected`],
    /// [`SmtpError::SmtpUtf8Unsupported`],
    /// [`SmtpError::EightBitMimeUnsupported`] and the network errors. After
    /// a network error, timeout or protocol error the session can't be used
    /// again. Those errors at [`Stage::DataEnd`] (after the message was
    /// written) have an [unknown outcome](SmtpError::outcome_unknown): the
    /// message may have been sent, so they are not transient.
    pub async fn send(
        &mut self,
        envelope: &Envelope,
        message: &OutgoingMessage,
    ) -> Result<ServerReply, SmtpError> {
        self.send_tracked(envelope, message, None)
            .await
            .map(|sent| sent.reply)
    }

    /// Like [`send`](Self::send), with `own` as the `ENVID` when the
    /// envelope has none and the server announced `DSN` in the `EHLO` of
    /// this connection (never an older list: a server that dropped DSN
    /// would refuse the command). Without DSN the message goes out without
    /// `ENVID`. [`Sent::envid`] says which one was sent, if any, for
    /// matching the delivery reports later.
    ///
    /// # Errors
    ///
    /// As [`send`](Self::send).
    pub async fn send_tracked(
        &mut self,
        envelope: &Envelope,
        message: &OutgoingMessage,
        own: Option<EnvelopeId>,
    ) -> Result<Sent, SmtpError> {
        if self.broken || self.conn.has_broken() {
            return Err(SmtpError::Network {
                stage: Stage::MailFrom,
            });
        }
        let envid = envelope.mail_dsn().envid_to_send(&self.extensions, own);
        let parameters = self.mail_parameters(envelope, message, envid.as_ref())?;
        let result = self
            .transaction(envelope, message, parameters)
            .await
            .map(|reply| Sent { reply, envid });
        match &result {
            Ok(_) => {}
            Err(
                SmtpError::Network { .. } | SmtpError::Timeout { .. } | SmtpError::Protocol { .. },
            ) => {
                self.broken = true;
            }
            Err(_) => self.reset().await,
        }
        result
    }

    /// Ends the session politely with `QUIT`. Errors don't matter here: the
    /// connection is closed either way.
    pub async fn quit(mut self) {
        if !self.broken {
            session::quit(&mut self.conn).await;
        }
    }

    /// The `MAIL FROM` parameters the envelope and message need.
    fn mail_parameters(
        &self,
        envelope: &Envelope,
        message: &OutgoingMessage,
        envid: Option<&EnvelopeId>,
    ) -> Result<Vec<MailParameter>, SmtpError> {
        let server = self.conn.server_info();
        let mut parameters = Vec::new();
        // RFC 6531 §3.4: SMTPUTF8 when the envelope *or the message* needs
        // it (UTF-8 header fields, RFC 6532); §3.2: never to a server that
        // doesn't announce it.
        let utf8_headers = message.has_8bit_headers();
        if envelope.needs_smtputf8() || utf8_headers {
            if !server.supports_feature(Extension::SmtpUtfEight) {
                return Err(if envelope.needs_smtputf8() {
                    SmtpError::SmtpUtf8Unsupported
                } else {
                    SmtpError::SmtpUtf8HeadersUnsupported
                });
            }
            parameters.push(MailParameter::SmtpUtfEight);
        }
        // RFC 1870: SIZE= with the message's size when announced, and
        // nothing bigger than a fixed maximum it announced (§6: refused
        // before sending any of it, instead of a 552 after the upload).
        if let Some(arguments) = self.extensions.arguments("SIZE") {
            let size = message.as_bytes().len();
            let maximum = arguments
                .first()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|&maximum| maximum > 0);
            if let Some(maximum) = maximum
                && size > maximum
            {
                return Err(SmtpError::TooLargeForServer { maximum });
            }
            parameters.push(MailParameter::Size(size));
        }
        if message.is_8bit() {
            if !server.supports_feature(Extension::EightBitMime) {
                return Err(SmtpError::EightBitMimeUnsupported);
            }
            parameters.push(MailParameter::Body(MailBodyParameter::EightBitMime));
        }
        if envelope.uses_dsn() && !self.extensions.has("DSN") {
            return Err(SmtpError::DsnUnsupported);
        }
        let dsn = envelope.mail_dsn();
        if let Some(ret) = dsn.ret {
            parameters.push(MailParameter::Other {
                keyword: "RET".to_owned(),
                value: Some(ret.as_str().to_owned()),
            });
        }
        if let Some(envid) = envid {
            parameters.push(MailParameter::Other {
                keyword: "ENVID".to_owned(),
                value: Some(envid.as_str().to_owned()),
            });
        }
        Ok(parameters)
    }

    /// The `RCPT TO` parameters of one recipient (DSN, checked above).
    fn rcpt_parameters(dsn: &RcptDsn) -> Vec<RcptParameter> {
        let mut parameters = Vec::new();
        if let Some(notify) = dsn.notify {
            parameters.push(RcptParameter::Other {
                keyword: "NOTIFY".to_owned(),
                value: Some(notify.to_value()),
            });
        }
        if let Some(orcpt) = &dsn.orcpt {
            parameters.push(RcptParameter::Other {
                keyword: "ORCPT".to_owned(),
                value: Some(orcpt.to_value()),
            });
        }
        parameters
    }

    async fn transaction(
        &mut self,
        envelope: &Envelope,
        message: &OutgoingMessage,
        parameters: Vec<MailParameter>,
    ) -> Result<ServerReply, SmtpError> {
        let command = self.timeouts.command;
        let mail = Mail::new(Some(envelope.from().address().clone()), parameters);
        timed(Stage::MailFrom, command, self.conn.command(mail))
            .await?
            .map_err(|error| SmtpError::from_lettre(Stage::MailFrom, &error))?;

        let mut rejected = Vec::new();
        for (recipient, dsn) in envelope.recipients() {
            let rcpt = Rcpt::new(recipient.address().clone(), Self::rcpt_parameters(dsn));
            match timed(Stage::RcptTo, command, self.conn.command(rcpt)).await? {
                Ok(_) => {}
                Err(error) => match error::reply(&error) {
                    Some((code, text)) => rejected.push(RecipientRejection {
                        recipient: recipient.clone(),
                        // RFC 5321 §4.5.3.1.10: a 552 to RCPT is the old
                        // "too many recipients", to be taken as the
                        // temporary 452 (the rest go in another message).
                        code: if code.get() == 552 {
                            ReplyCode::TOO_MANY_RECIPIENTS
                        } else {
                            code
                        },
                        text,
                    }),
                    None => return Err(SmtpError::from_lettre(Stage::RcptTo, &error)),
                },
            }
        }
        if !rejected.is_empty() {
            return Err(SmtpError::RecipientsRejected { rejected });
        }

        timed(
            Stage::Data,
            self.timeouts.data_start,
            self.conn.command(Data),
        )
        .await?
        .map_err(|error| SmtpError::from_lettre(Stage::Data, &error))?;
        // From here on the server may get the whole message: a failure
        // before its reply leaves the outcome unknown (`Stage::DataEnd`).
        // No limit on the whole transfer (RFC 5321 §4.5.3.2):
        // only on a stall while sending, and on the wait for the reply.
        let response = self
            .activity
            .watch(
                self.timeouts.data_block,
                self.timeouts.data_end,
                self.conn.message(message.data()),
            )
            .await?
            .map_err(|error| SmtpError::from_lettre(Stage::DataEnd, &error))?;
        Ok(ServerReply::from_lettre(&response))
    }

    /// Cancels the transaction after a refusal so the session can be used
    /// again. If even that fails, the session is marked broken.
    async fn reset(&mut self) {
        let reset = timed(
            Stage::MailFrom,
            self.timeouts.command,
            self.conn.command(Rset),
        )
        .await;
        if !matches!(reset, Ok(Ok(_))) {
            self.broken = true;
        }
    }
}
