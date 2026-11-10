// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The boundary between HTTP and mail: what the `jmap` crate implements.
//!
//! The HTTP crate owns OAuth, tokens, the session object, limits and the
//! wire formats. Everything that needs the mail server (checking a
//! password, running JMAP methods, blobs, push) goes through [`Backend`].
//!
//! **Why generics and not `dyn`:** a process has exactly one backend, so
//! the router is built for one concrete type. Methods return
//! `impl Future + Send` (Rust 2024 return-position `impl Trait` in traits),
//! which keeps static dispatch and needs no boxing; `Send` is required
//! because axum runs handlers on a multi-threaded runtime.

use std::fmt;
use std::future::Future;
use std::str::FromStr;

use axum::body::{Body, BodyDataStream};
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{Stream, StreamExt as _};
use nebula_crypto::AccountKey;
use nebula_jmap_proto::push::{CalendarAlert, StateChange};
use nebula_jmap_proto::session::AccountCapabilities;
use nebula_jmap_proto::{
    AccountId as JmapAccountId, BlobId, MethodLimits, Request, Response, State,
};
use nebula_store::{AccountId, GrantId};
use secrecy::SecretString;

/// A login name as typed in the form: trimmed, 1 to 200 bytes, no control
/// characters. Whether it exists is for the mail server to say.
#[derive(Clone, PartialEq, Eq)]
pub struct LoginName(String);

impl LoginName {
    /// Longest accepted login, in bytes. It must fit the 255-byte account
    /// id used in the encryption AAD, with its prefix.
    pub const MAX_LEN: usize = 200;

    /// The login.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a login name or password from the form is not acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid login or password")]
pub struct InvalidCredentials;

impl FromStr for LoginName {
    type Err = InvalidCredentials;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() || text.len() > Self::MAX_LEN || text.chars().any(char::is_control) {
            return Err(InvalidCredentials);
        }
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Debug for LoginName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A login is personal data: keep it out of logs.
        f.write_str("LoginName(..)")
    }
}

/// A password from the login form: 1 to 1024 bytes, without NUL, CR or LF
/// (they can't be sent safely in an IMAP `LOGIN`).
pub(crate) fn parse_password(text: &str) -> Result<SecretString, InvalidCredentials> {
    if text.is_empty() || text.len() > 1024 || text.contains(['\0', '\r', '\n']) {
        return Err(InvalidCredentials);
    }
    Ok(SecretString::from(text))
}

/// What the mail server said about a login.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoginOutcome {
    /// The credentials work. `email` is the address of the default
    /// identity.
    Success {
        /// The account's address.
        email: String,
    },
    /// The server rejected the login or password.
    WrongCredentials {
        /// The server's `[ALERT]` with the refusal, shown in the form
        /// (RFC 3501 §7.1, RFC 9051 §7.1: it MUST reach the user).
        alert: Option<LoginAlert>,
    },
    /// The server could not be reached or failed; the user should retry.
    /// The attempt doesn't count against the login (it still counts
    /// against the IP).
    Unavailable,
    /// The server answered, but not with a known credentials failure: it
    /// counts as a failed attempt, and the user sees "unavailable".
    Rejected,
}

/// The text of a mail server's alert during a login check: plain text,
/// already cleaned by the backend (no control or invisible characters, a
/// few hundred characters at most). Pages escape it like any value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAlert(String);

impl LoginAlert {
    /// Wraps text that the backend cleaned.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A logged-in account, as the handlers pass it to the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountContext {
    /// The account in the store.
    pub id: AccountId,
    /// Its JMAP `accountId` (see [`jmap_account_id`]).
    pub jmap_id: JmapAccountId,
    /// The IMAP login.
    pub login: String,
    /// Address of the default identity.
    pub email: String,
}

/// The JMAP `accountId` of a store account: `a` and the row number. Stable
/// for the life of the account.
#[must_use]
pub fn jmap_account_id(id: AccountId) -> Option<JmapAccountId> {
    format!("a{}", id.get()).parse().ok()
}

/// The `accountId` of the calendars and address books someone else shares
/// with the user (RFC 9670 sharing: they are that person's account in the
/// session, as Stalwart shows them): the user's own id, `-` and a key the
/// backend chooses (letters and digits). `None` if the key does not make a
/// valid id.
#[must_use]
pub fn shared_account_id(own: &JmapAccountId, key: &str) -> Option<JmapAccountId> {
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    format!("{}-{key}", own.as_str()).parse().ok()
}

/// The key of a shared account of `own` ([`shared_account_id`]), or
/// `None` if `id` is not one.
#[must_use]
pub fn shared_account_key<'a>(own: &JmapAccountId, id: &'a JmapAccountId) -> Option<&'a str> {
    id.as_str()
        .strip_prefix(own.as_str())?
        .strip_prefix('-')
        .filter(|key| !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Someone else's account the user sees because things are shared with
/// them (only calendars and address books), for the session object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedAccount {
    /// Its `accountId` ([`shared_account_id`]).
    pub id: JmapAccountId,
    /// Its name (the owner's address or name).
    pub name: String,
    /// What it has.
    pub capabilities: AccountCapabilities,
}

/// A media type (`text/plain; charset=utf-8`): printable ASCII, no line
/// breaks, at most 255 bytes, with a `type/subtype` of token characters.
/// The type and subtype are lower-cased.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaType(String);

impl MediaType {
    /// `application/octet-stream`, when nothing better is known.
    #[must_use]
    pub fn octet_stream() -> Self {
        Self("application/octet-stream".to_owned())
    }

    /// The media type.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a media type is not acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid media type")]
pub struct InvalidMediaType;

impl FromStr for MediaType {
    type Err = InvalidMediaType;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.is_empty() || text.len() > 255 || !text.bytes().all(|b| (b' '..=b'~').contains(&b))
        {
            return Err(InvalidMediaType);
        }
        let (essence, params) = match text.split_once(';') {
            Some((essence, params)) => (essence.trim(), Some(params)),
            None => (text, None),
        };
        let (kind, subtype) = essence.split_once('/').ok_or(InvalidMediaType)?;
        if !is_token(kind) || !is_token(subtype) {
            return Err(InvalidMediaType);
        }
        let mut normal = format!(
            "{}/{}",
            kind.to_ascii_lowercase(),
            subtype.to_ascii_lowercase()
        );
        if let Some(params) = params {
            normal.push(';');
            normal.push_str(params);
        }
        Ok(Self(normal))
    }
}

/// RFC 9110 `token`.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

impl fmt::Debug for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MediaType").field(&self.0).finish()
    }
}

/// The languages a request prefers (its `Accept-Language`, RFC 9110
/// §12.5.4), for text the server writes on the user's behalf (the
/// bridge's invitation emails). Only primary tags (`es`, `en`) and their
/// weights are kept; a header longer than 256 bytes or entries past the
/// 16th are ignored. Empty: no preference.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcceptLanguage(Vec<(String, u16)>);

impl AcceptLanguage {
    /// Parses a header value; anything malformed is skipped.
    #[must_use]
    pub fn parse(header: &str) -> Self {
        if header.len() > 256 {
            return Self::default();
        }
        let mut list = Vec::new();
        for item in header.split(',').take(16) {
            let mut parts = item.split(';');
            let tag = parts.next().unwrap_or_default().trim();
            let primary = tag.split('-').next().unwrap_or_default();
            if primary.is_empty()
                || primary.len() > 8
                || !primary.bytes().all(|b| b.is_ascii_alphabetic())
            {
                continue;
            }
            let mut weight = 1000_u16;
            for param in parts {
                if let Some(q) = param.trim().strip_prefix("q=") {
                    weight = parse_weight(q.trim()).unwrap_or(0);
                }
            }
            if weight > 0 {
                list.push((primary.to_ascii_lowercase(), weight));
            }
        }
        // Highest weight first; equal weights keep their order.
        list.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        Self(list)
    }

    /// The most preferred of `supported` (lower-case primary tags), if any
    /// is accepted.
    #[must_use]
    pub fn best<'a>(&self, supported: &[&'a str]) -> Option<&'a str> {
        self.0.iter().find_map(|(tag, _)| {
            supported
                .iter()
                .find(|candidate| **candidate == tag.as_str())
                .copied()
        })
    }
}

/// A `q` value (`0`, `0.5`, `1.000`) in thousandths.
fn parse_weight(text: &str) -> Option<u16> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut thousandths: u16 = match whole {
        "0" => 0,
        "1" => 1000,
        _ => return None,
    };
    let mut scale = 100_u16;
    for digit in fraction.bytes() {
        thousandths = thousandths.saturating_add(u16::from(digit - b'0').saturating_mul(scale));
        scale /= 10;
    }
    (thousandths <= 1000).then_some(thousandths)
}

/// A JMAP API request, ready for method dispatch.
#[derive(Debug, Clone)]
pub struct ApiCall {
    /// The parsed request (request-level checks already done).
    pub request: Request,
    /// The session state to put in the response's `sessionState`.
    pub session_state: State,
    /// Limits for [`MethodCall::parse`](nebula_jmap_proto::MethodCall::parse).
    pub limits: MethodLimits,
    /// The request's `Accept-Language` (the web app sends its interface
    /// language).
    pub languages: AcceptLanguage,
    /// The login session (OAuth grant) of the access token. What belongs
    /// to one session (push subscriptions, RFC 8620 §7.2) is only seen
    /// and changed by it.
    pub session: GrantId,
}

/// A blob the backend stored after an upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedBlob {
    /// Its id.
    pub blob_id: BlobId,
    /// The media type stored.
    pub media_type: MediaType,
    /// Size in bytes.
    pub size: u64,
}

/// The body of an upload, read as it arrives (never whole in memory). It
/// ends with [`UploadBodyError::TooLarge`] as soon as it passes
/// `maxSizeUpload`; a declared `Content-Length` above it fails before
/// anything is read.
pub struct UploadBody {
    stream: BodyDataStream,
    declared: Option<u64>,
    limit: u64,
    seen: u64,
}

impl fmt::Debug for UploadBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadBody")
            .field("limit", &self.limit)
            .field("seen", &self.seen)
            .finish_non_exhaustive()
    }
}

/// Why an upload body could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UploadBodyError {
    /// It is larger than `maxSizeUpload`.
    #[error("upload larger than maxSizeUpload")]
    TooLarge,
    /// The client stopped sending, or sent a broken body.
    #[error("upload body could not be read")]
    Read,
}

impl UploadBody {
    /// A body of at most `limit` bytes; `declared` is its `Content-Length`.
    pub(crate) fn new(body: Body, declared: Option<u64>, limit: u64) -> Self {
        Self {
            stream: body.into_data_stream(),
            declared,
            limit,
            seen: 0,
        }
    }

    /// For tests of backends: a body made of `chunks`.
    #[must_use]
    pub fn from_chunks(chunks: Vec<Bytes>, limit: u64) -> Self {
        let stream = futures::stream::iter(chunks.into_iter().map(Ok::<_, std::io::Error>));
        Self::new(Body::from_stream(stream), None, limit)
    }

    /// The next piece of the body, `None` at the end.
    ///
    /// # Errors
    /// [`UploadBodyError::TooLarge`] once past the limit (nothing more is
    /// read), [`UploadBodyError::Read`] if the body broke off.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, UploadBodyError> {
        if self.declared.is_some_and(|length| length > self.limit) {
            return Err(UploadBodyError::TooLarge);
        }
        let Some(chunk) = self.stream.next().await else {
            return Ok(None);
        };
        let chunk = chunk.map_err(|_| UploadBodyError::Read)?;
        let len = u64::try_from(chunk.len()).map_err(|_| UploadBodyError::TooLarge)?;
        self.seen = self.seen.saturating_add(len);
        if self.seen > self.limit {
            return Err(UploadBodyError::TooLarge);
        }
        Ok(Some(chunk))
    }

    /// `maxSizeUpload`, in bytes.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// Why an upload failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UploadError {
    /// The body was too large or broke off: answer 413 or 400.
    #[error(transparent)]
    Body(#[from] UploadBodyError),
    /// The blob doesn't fit in the account's storage for uploads, even after
    /// deleting its oldest ones (RFC 8620 §6): answer 413 `overQuota`.
    #[error("upload storage of the account is full")]
    OverQuota,
    /// Another failure.
    #[error(transparent)]
    Backend(#[from] BackendError),
}

/// The content of a blob to download.
#[non_exhaustive]
pub enum BlobBody {
    /// Already in memory.
    Bytes(Bytes),
    /// Read piece by piece while it is sent. A failure in the middle cuts
    /// the response off (the client sees a short body, never wrong data:
    /// `Content-Length` is `size`).
    Stream {
        /// Total size in bytes.
        size: u64,
        /// The pieces, in order.
        chunks: BoxStream<'static, Result<Bytes, BackendError>>,
    },
}

impl fmt::Debug for BlobBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytes(bytes) => f.debug_tuple("Bytes").field(&bytes.len()).finish(),
            Self::Stream { size, .. } => f.debug_struct("Stream").field("size", size).finish(),
        }
    }
}

/// A blob to download.
#[derive(Debug)]
pub struct Blob {
    /// The content.
    pub body: BlobBody,
    /// Its media type, used when the client doesn't ask for one.
    pub media_type: MediaType,
}

/// A backend failure. Carries no user data, so it is safe to log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// The mail server is unreachable or busy: answer 503, try later.
    #[error("mail server unavailable")]
    Unavailable,
    /// A bug or an unexpected failure: answer 500.
    #[error("internal backend error: {0}")]
    Internal(&'static str),
    /// The mail server refused the account's password while starting it:
    /// its sessions were ended and its password deleted. Answer 401 (log
    /// in again).
    #[error("the mail server refused the password")]
    PasswordRefused,
}

/// Why [`Backend::account_started`] is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartReason {
    /// A login the mail server just accepted ([`Backend::verify_login`]):
    /// start now, whatever failed before.
    Login,
    /// With the stored password (first request after a restart, or at
    /// boot): earlier failures may make it wait.
    Restore,
}

/// What the push stream of an account carries (RFC 8620 §7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PushEvent {
    /// New states: a `state` event.
    State(StateChange),
    /// A calendar alert is due: a `calendarAlert` event, only for
    /// connections that asked for `CalendarAlert` (or `*`).
    CalendarAlert(CalendarAlert),
}

/// What the HTTP server needs from the mail side.
///
/// Contract:
/// - [`Backend::account_started`] is called after every successful login
///   and the first time a token of the account is used after a restart.
///   It may be called again for an account that is running (with a new
///   password after a re-login): it must be idempotent.
/// - [`Backend::account_stopped`] is called when the account's last login
///   session ends; its password has been deleted from the store.
/// - Every other method gets an account that was started.
pub trait Backend: Send + Sync + 'static {
    /// The push stream of one account.
    type Events: Stream<Item = PushEvent> + Send + 'static;

    /// Checks a login and password against the mail server (an IMAP login,
    /// like Roundcube does).
    fn verify_login(
        &self,
        login: &LoginName,
        password: &SecretString,
    ) -> impl Future<Output = LoginOutcome> + Send;

    /// Starts (or refreshes) the account: sync, IDLE… `key` is the
    /// account's data key, unwrapped, for data the backend keeps sealed
    /// while the account runs (the local copy of calendars and contacts)
    /// it lives in memory only.
    ///
    /// # Errors
    /// [`BackendError`] if it can't be started now;
    /// [`BackendError::PasswordRefused`] if the mail server refused the
    /// password (the backend already ended the account's sessions).
    fn account_started(
        &self,
        account: &AccountContext,
        password: SecretString,
        key: AccountKey,
        reason: StartReason,
    ) -> impl Future<Output = Result<(), BackendError>> + Send;

    /// Stops the account and forgets its password.
    fn account_stopped(&self, account: AccountId) -> impl Future<Output = ()> + Send;

    /// One login session of the account ended (revoked) while others
    /// remain, so the account keeps running: drop from memory whatever
    /// belonged to that session (its push subscriptions are already gone
    /// from the store).
    fn session_ended(&self, account: AccountId) -> impl Future<Output = ()> + Send {
        let _ = account;
        async {}
    }

    /// The account's capabilities for the session object (mail and
    /// submission limits, whether it has quotas).
    ///
    /// # Errors
    /// [`BackendError`] if they can't be known now.
    fn account_capabilities(
        &self,
        account: &AccountContext,
    ) -> impl Future<Output = Result<AccountCapabilities, BackendError>> + Send;

    /// The VAPID public key (base64url) if the backend sends Web Push:
    /// the session then announces `urn:ietf:params:jmap:webpush-vapid`
    /// with it as `applicationServerKey`.
    fn web_push_key(&self) -> Option<String> {
        None
    }

    /// The accounts of others the user sees in the session (calendars and
    /// address books shared with them). Calls with their ids go to
    /// [`Backend::jmap`] like any other; push events may name them.
    ///
    /// # Errors
    /// [`BackendError`] if they can't be known now.
    fn shared_accounts(
        &self,
        account: &AccountContext,
    ) -> impl Future<Output = Result<Vec<SharedAccount>, BackendError>> + Send {
        let _ = account;
        std::future::ready(Ok(Vec::new()))
    }

    /// Runs the method calls of a request.
    ///
    /// # Errors
    /// [`BackendError`] only for failures of the whole request; a failing
    /// method is a method-level error inside the response.
    fn jmap(
        &self,
        account: &AccountContext,
        call: ApiCall,
    ) -> impl Future<Output = Result<Response, BackendError>> + Send;

    /// Stores an uploaded blob while it arrives. The body stops with
    /// [`UploadBodyError::TooLarge`] past `maxSizeUpload`; whatever was
    /// stored of a failed upload must be deleted.
    ///
    /// # Errors
    /// [`UploadError`]: the body's error, `OverQuota`, or another failure.
    fn upload(
        &self,
        account: &AccountContext,
        media_type: MediaType,
        body: UploadBody,
    ) -> impl Future<Output = Result<UploadedBlob, UploadError>> + Send;

    /// A blob of the account, or `None` if it doesn't exist.
    ///
    /// # Errors
    /// [`BackendError`] if it can't be fetched now.
    fn download(
        &self,
        account: &AccountContext,
        blob_id: &BlobId,
    ) -> impl Future<Output = Result<Option<Blob>, BackendError>> + Send;

    /// A stream of the account's state changes and calendar alerts, for
    /// `EventSource`. It may include other accounts' events; the server
    /// filters them out (and the types the connection did not ask for).
    /// `alerts`: the connection asked for `CalendarAlert`; only then does
    /// it count as someone who sees the account's alerts (an alert is
    /// sent once, and one sent to a connection that drops it is lost).
    ///
    /// # Errors
    /// [`BackendError`] if push is not available now.
    fn events(
        &self,
        account: &AccountContext,
        alerts: bool,
    ) -> impl Future<Output = Result<Self::Events, BackendError>> + Send;

    /// The current state of every type the push stream of the account can
    /// change, in the account and its shared accounts (what
    /// [`Backend::events`] would send if all of them changed now). The
    /// event ids of `EventSource` encode it, so a client that reconnects
    /// gets what it missed (RFC 8620 §7.3).
    ///
    /// # Errors
    /// [`BackendError`] if the states can't be read now.
    fn push_states(
        &self,
        account: &AccountContext,
    ) -> impl Future<Output = Result<StateChange, BackendError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_names() {
        assert_eq!(" alice ".parse::<LoginName>().unwrap().as_str(), "alice");
        assert!("".parse::<LoginName>().is_err());
        assert!("   ".parse::<LoginName>().is_err());
        assert!("al\nice".parse::<LoginName>().is_err());
        assert!("a".repeat(201).parse::<LoginName>().is_err());
        assert_eq!(
            format!("{:?}", "alice".parse::<LoginName>().unwrap()),
            "LoginName(..)"
        );
    }

    #[test]
    fn passwords() {
        assert!(parse_password("hunter2 with spaces ").is_ok());
        assert!(parse_password("").is_err());
        assert!(parse_password("a\r\nb").is_err());
        assert!(parse_password("a\0").is_err());
    }

    #[test]
    fn media_types() {
        let parsed: MediaType = "Text/HTML; charset=UTF-8".parse().unwrap();
        assert_eq!(parsed.as_str(), "text/html; charset=UTF-8");
        assert!("image/png".parse::<MediaType>().is_ok());
        for bad in [
            "",
            "text",
            "text/",
            "/plain",
            "te xt/plain",
            "text/plain\r\nX: y",
            "téxt/plain",
        ] {
            assert!(bad.parse::<MediaType>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn accept_language() {
        let parsed = AcceptLanguage::parse("es-ES,es;q=0.9,en-US;q=0.8,en;q=0.7");
        assert_eq!(parsed.best(&["en", "es"]), Some("es"));
        assert_eq!(
            AcceptLanguage::parse("fr-FR, en;q=0.5, es;q=0.6").best(&["en", "es"]),
            Some("es")
        );
        assert_eq!(AcceptLanguage::parse("fr, de").best(&["en", "es"]), None);
        assert_eq!(
            AcceptLanguage::parse("es;q=0, en").best(&["en", "es"]),
            Some("en")
        );
        assert_eq!(AcceptLanguage::parse("ES").best(&["en", "es"]), Some("es"));
        assert_eq!(AcceptLanguage::parse("").best(&["en"]), None);
        assert_eq!(AcceptLanguage::parse("*").best(&["en"]), None);
        assert_eq!(AcceptLanguage::parse("es;q=2").best(&["es"]), None);
        assert_eq!(
            AcceptLanguage::parse(&"es,".repeat(200)).best(&["es"]),
            None
        );
        assert_eq!(parse_weight("0.125"), Some(125));
        assert_eq!(parse_weight("1.000"), Some(1000));
        assert_eq!(parse_weight("1.5"), None);
    }

    #[test]
    fn account_ids() {
        let id = AccountId::from_raw(42).unwrap();
        assert_eq!(jmap_account_id(id).unwrap().as_str(), "a42");
    }
}
