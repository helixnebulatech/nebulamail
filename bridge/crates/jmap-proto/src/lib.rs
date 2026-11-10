// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JMAP types (RFC 8620 core, RFC 8621 mail, RFC 9425 quotas, RFC 9610
//! contacts, draft-ietf-jmap-calendars, RFC 7807 problem details), without
//! I/O.
//!
//! What the bridge's JMAP server parses and produces:
//!
//! - [`Request`] and [`Response`] (RFC 8620 §3), with the request-level
//!   limits and errors ([`RequestError`]).
//! - Result references (`#ids`, [`resolve_result_references`]) and
//!   creation references (`#k1`, [`IdOrRef`], [`CreatedIds`]).
//! - Typed method arguments ([`MethodCall`]) and responses
//!   ([`MethodResponse`]), with method-level errors ([`MethodError`]).
//! - The objects ([`object`]): Mailbox, Thread, Email (including
//!   `header:…` properties), `SearchSnippet`, Identity, `EmailSubmission`,
//!   Quota, Calendar, `CalendarEvent`, `ParticipantIdentity`,
//!   `AddressBook`, `ContactCard`; objects to create and patches, which
//!   fail one by one with a [`SetError`]. JSCalendar and JSContact bodies
//!   are bounded JSON with the members the bridge reads checked
//!   ([`object::js_body`]); converting them is `groupware`'s job.
//! - The [`Session`](session::Session) and push ([`push::StateChange`],
//!   [`push::CalendarAlert`]).
//!
//! Parsing is strict on types (an `UnsignedInt` is never `1.5`, an id is
//! never `"a b"`) and tolerant on member order. Values that end up in
//! messages or SMTP commands (header values, envelope addresses) reject
//! line breaks, so a client can't inject header lines or SMTP commands.
//!
//! Only `Serialize` is implemented for response types: the bridge
//! produces them and never reads them back.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// in `*-proto` crates arithmetic must not overflow
// silently; use `checked_*`/`saturating_*`.
#![deny(clippy::arithmetic_side_effects)]

mod macros;

pub mod error;
pub mod method;
pub mod object;
pub mod push;
pub mod references;
pub mod request;
pub mod response;
pub mod session;
pub mod types;

pub use error::{
    MethodError, MethodErrorType, RequestError, RequestErrorType, SetError, SetErrorType,
};
pub use references::{
    CreatedIds, IdOrRef, JsonPointer, OverBudget, ReferenceBudget, ResultReference,
    resolve_result_references,
};
pub use request::{
    Capability, Invocation, MethodCall, MethodLimits, MethodName, Request, RequestLimits,
};
pub use response::{MethodResponse, Response, ResponseInvocation, ResponseName};
pub use types::{
    AccountId, AddressBookId, BlobId, CalendarEventId, CalendarId, CallId, ContactCardId,
    CreationId, Date, Duration, EmailId, EmailSubmissionId, Id, IdentityId, Int, Keyword,
    LocalDateTime, MailboxId, ParticipantIdentityId, PrincipalId, PushSubscriptionId, QuotaId,
    SieveScriptId, State, ThreadId, TimeZoneId, UnsignedInt, UtcDate, VacationResponseId,
};
