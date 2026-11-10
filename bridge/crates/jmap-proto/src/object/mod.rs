// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The JMAP Mail data types (RFC 8621), Quota (RFC 9425), Calendars
//! (draft-ietf-jmap-calendars), Contacts (RFC 9610) and Principals
//! (RFC 9670).
//!
//! Response objects (`Mailbox`, `Email`…) have every property optional:
//! only the requested `properties` are sent. Objects to create
//! (`MailboxCreate`, `EmailCreate`…) and updates (`MailboxPatch`,
//! `EmailPatch`…) are parsed from JSON one by one, so a bad object only
//! fails itself with a `SetError`.

mod address_book;
pub mod bounded;
mod calendar;
mod calendar_counter;
mod calendar_event;
mod calendar_trash;
mod contact_card;
pub mod email;
mod email_submission;
mod identity;
pub mod js_body;
mod mailbox;
mod map_patch;
mod mdn;
mod participant_identity;
mod principal;
mod push_subscription;
mod quota;
mod search_snippet;
mod sieve_script;
mod thread;
mod vacation_response;

pub use address_book::{
    AddressBook, AddressBookCreate, AddressBookPatch, AddressBookProperty, AddressBookRights,
    AddressBookSetArguments,
};
pub use calendar::{
    Alert, COMPONENTS_PROPERTY, Calendar, CalendarComponent, CalendarCreate, CalendarPatch,
    CalendarProperty, CalendarRights, CalendarSetArguments, DefaultAlerts, IS_PUBLISHED_PROPERTY,
    IncludeInAvailability, MAX_SUBSCRIPTION_URL, PUBLIC_URL_PROPERTY, SUBSCRIPTION_URL_PROPERTY,
};
pub use calendar_counter::{
    CalendarEventCounterArguments, CalendarEventDeclineCounterArguments, CalendarItipSentResponse,
    CounterSubject,
};
pub use calendar_event::{
    CalendarEvent, CalendarEventCreate, CalendarEventFilterCondition, CalendarEventGetArguments,
    CalendarEventParseArguments, CalendarEventParseResponse, CalendarEventPatch,
    CalendarEventProperty, CalendarEventQueryArguments, CalendarEventQueryChangesArguments,
    CalendarEventSetArguments, CalendarEventSort,
};
pub use calendar_trash::{
    CalendarTrashGetArguments, CalendarTrashGetResponse, CalendarTrashSetArguments,
    CalendarTrashSetResponse, TrashId, TrashKind, TrashedCalendar, TrashedEvent,
};
pub use contact_card::{
    ContactCard, ContactCardCreate, ContactCardFilterCondition, ContactCardParseArguments,
    ContactCardParseResponse, ContactCardPatch, ContactCardProperty, ContactCardQueryArguments,
    ContactCardQueryChangesArguments, ContactCardSort,
};
pub use email::{
    Email, EmailCreate, EmailField, EmailFilterCondition, EmailGetArguments, EmailParseArguments,
    EmailParseResponse, EmailPatch, EmailProperty, EmailQueryArguments, EmailQueryChangesArguments,
    EmailSort,
};
pub use email_submission::{
    Delivered, DeliveryStatus, Displayed, EmailSubmission, EmailSubmissionCreate,
    EmailSubmissionFilterCondition, EmailSubmissionPatch, EmailSubmissionProperty,
    EmailSubmissionQueryArguments, EmailSubmissionQueryChangesArguments,
    EmailSubmissionSetArguments, EmailSubmissionSort, Envelope, EnvelopeAddress, EnvelopeError,
    SEND_WHEN_SIGNED_OUT_PROPERTY, SmtpParameterName, SmtpParameterValue, SmtpParameters,
    SubmissionAddress, UndoStatus,
};
pub use identity::{
    Identity, IdentityCreate, IdentityFields, IdentityPatch, IdentityProperty,
    MAX_IDENTITY_ADDRESS, MAX_IDENTITY_ADDRESSES, MAX_IDENTITY_NAME, MAX_IDENTITY_SIGNATURE,
};
pub use mailbox::{
    IMAP_NAME_PROPERTY, Mailbox, MailboxCreate, MailboxFilterCondition, MailboxPatch,
    MailboxProperty, MailboxQueryArguments, MailboxQueryChangesArguments, MailboxRights,
    MailboxRole, MailboxSetArguments, MailboxSort,
};
pub use map_patch::MapPatch;
pub use mdn::{
    ActionMode, Disposition, DispositionType, Mdn, MdnCreate, MdnParseArguments, MdnParseResponse,
    MdnSendArguments, MdnSendResponse, MdnSent, SendingMode,
};
pub use participant_identity::{ParticipantIdentity, ParticipantIdentityProperty};
pub use principal::{
    BusyPeriod, BusyStatus, Principal, PrincipalCalendars, PrincipalCapabilities,
    PrincipalFilterCondition, PrincipalGetAvailabilityArguments, PrincipalGetAvailabilityResponse,
    PrincipalProperty, PrincipalQueryArguments, PrincipalSort, PrincipalType,
};
pub use push_subscription::{
    EmailPushConfig, EmailPushMap, PushKeys, PushSubscription, PushSubscriptionCreate,
    PushSubscriptionGetArguments, PushSubscriptionGetResponse, PushSubscriptionPatch,
    PushSubscriptionProperty, PushSubscriptionSetArguments, PushSubscriptionSetResponse,
    PushUrgency, PushVerification,
};
pub use quota::{
    Quota, QuotaChangesResponse, QuotaFilterCondition, QuotaProperty, QuotaQueryArguments,
    QuotaQueryChangesArguments, QuotaScope, QuotaSort, ResourceType,
};
pub use search_snippet::{SearchSnippet, SearchSnippetGetArguments, SearchSnippetGetResponse};
pub use sieve_script::{
    SieveScript, SieveScriptCreate, SieveScriptFilterCondition, SieveScriptPatch,
    SieveScriptProperty, SieveScriptQueryArguments, SieveScriptSetArguments, SieveScriptSort,
    SieveScriptValidateArguments, SieveScriptValidateResponse,
};
pub use thread::{Thread, ThreadProperty};
pub use vacation_response::{
    VACATION_SINGLETON, VacationResponse, VacationResponsePatch, VacationResponseProperty,
};
