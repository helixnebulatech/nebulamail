// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed method responses.

use serde::Serialize;
use serde_json::{Map, Value};

use super::{ResponseInvocation, ResponseName};
use crate::error::MethodError;
use crate::method::{
    ChangesResponse, EmailImportResponse, GetResponse, MailboxChangesResponse,
    QueryChangesResponse, QueryResponse, SetResponse,
};
use crate::object::{
    AddressBook, Calendar, CalendarEvent, CalendarEventParseResponse, CalendarItipSentResponse,
    CalendarTrashGetResponse, CalendarTrashSetResponse, ContactCard, ContactCardParseResponse,
    Email, EmailSubmission, Identity, Mailbox, MdnParseResponse, MdnSendResponse,
    ParticipantIdentity, Principal, PrincipalGetAvailabilityResponse, PushSubscriptionGetResponse,
    PushSubscriptionSetResponse, Quota, QuotaChangesResponse, SearchSnippetGetResponse,
    SieveScript, SieveScriptValidateResponse, Thread, VacationResponse,
};
use crate::request::MethodName;
use crate::types::{
    AddressBookId, CalendarEventId, CalendarId, CallId, ContactCardId, EmailId, EmailSubmissionId,
    IdentityId, MailboxId, ParticipantIdentityId, PrincipalId, QuotaId, SieveScriptId, ThreadId,
    VacationResponseId,
};

/// The response of one method, typed.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum MethodResponse {
    /// `Core/echo`.
    CoreEcho(Map<String, Value>),
    /// `Mailbox/get`.
    MailboxGet(GetResponse<Mailbox, MailboxId>),
    /// `Mailbox/changes`.
    MailboxChanges(MailboxChangesResponse),
    /// `Mailbox/query`.
    MailboxQuery(QueryResponse<MailboxId>),
    /// `Mailbox/queryChanges`.
    MailboxQueryChanges(QueryChangesResponse<MailboxId>),
    /// `Mailbox/set`.
    MailboxSet(SetResponse<Mailbox, MailboxId>),
    /// `Email/get`.
    EmailGet(GetResponse<Email, EmailId>),
    /// `Email/changes`.
    EmailChanges(ChangesResponse<EmailId>),
    /// `Email/query`.
    EmailQuery(QueryResponse<EmailId>),
    /// `Email/queryChanges`.
    EmailQueryChanges(QueryChangesResponse<EmailId>),
    /// `Email/set` (also the implicit one after `EmailSubmission/set`).
    EmailSet(SetResponse<Email, EmailId>),
    /// `Email/import`.
    EmailImport(EmailImportResponse),
    /// `Email/parse`.
    EmailParse(crate::object::EmailParseResponse),
    /// `Blob/copy`.
    BlobCopy(crate::method::BlobCopyResponse),
    /// `Thread/get`.
    ThreadGet(GetResponse<Thread, ThreadId>),
    /// `Thread/changes`.
    ThreadChanges(ChangesResponse<ThreadId>),
    /// `SearchSnippet/get`.
    SearchSnippetGet(SearchSnippetGetResponse),
    /// `Identity/get`.
    IdentityGet(GetResponse<Identity, IdentityId>),
    /// `Identity/changes`.
    IdentityChanges(ChangesResponse<IdentityId>),
    /// `Identity/set`.
    IdentitySet(SetResponse<Identity, IdentityId>),
    /// `EmailSubmission/get`.
    EmailSubmissionGet(GetResponse<EmailSubmission, EmailSubmissionId>),
    /// `EmailSubmission/query`.
    EmailSubmissionQuery(QueryResponse<EmailSubmissionId>),
    /// `EmailSubmission/queryChanges`.
    EmailSubmissionQueryChanges(QueryChangesResponse<EmailSubmissionId>),
    /// `EmailSubmission/set`.
    EmailSubmissionSet(SetResponse<EmailSubmission, EmailSubmissionId>),
    /// `Quota/get`.
    QuotaGet(GetResponse<Quota, QuotaId>),
    /// `Quota/changes`, with `updatedProperties` (RFC 9425 §4.3).
    QuotaChanges(QuotaChangesResponse),
    /// `Quota/query`.
    QuotaQuery(QueryResponse<QuotaId>),
    /// `Quota/queryChanges` (RFC 9425 §4.5).
    QuotaQueryChanges(QueryChangesResponse<QuotaId>),
    /// `MDN/send`.
    MdnSend(MdnSendResponse),
    /// `MDN/parse`.
    MdnParse(MdnParseResponse),
    /// `Calendar/get`.
    CalendarGet(GetResponse<Calendar, CalendarId>),
    /// `Calendar/changes`.
    CalendarChanges(ChangesResponse<CalendarId>),
    /// `Calendar/set`.
    CalendarSet(SetResponse<Calendar, CalendarId>),
    /// `CalendarEvent/get`.
    CalendarEventGet(GetResponse<CalendarEvent, CalendarEventId>),
    /// `CalendarEvent/changes`.
    CalendarEventChanges(ChangesResponse<CalendarEventId>),
    /// `CalendarEvent/query`.
    CalendarEventQuery(QueryResponse<CalendarEventId>),
    /// `CalendarEvent/queryChanges`.
    CalendarEventQueryChanges(QueryChangesResponse<CalendarEventId>),
    /// `CalendarEvent/set`.
    CalendarEventSet(SetResponse<CalendarEvent, CalendarEventId>),
    /// `CalendarEvent/parse`.
    CalendarEventParse(CalendarEventParseResponse),
    /// `CalendarEvent/counter`.
    CalendarEventCounter(CalendarItipSentResponse),
    /// `CalendarEvent/declineCounter`.
    CalendarEventDeclineCounter(CalendarItipSentResponse),
    /// `CalendarTrash/get`.
    CalendarTrashGet(CalendarTrashGetResponse),
    /// `CalendarTrash/set`.
    CalendarTrashSet(CalendarTrashSetResponse),
    /// `ParticipantIdentity/get`.
    ParticipantIdentityGet(GetResponse<ParticipantIdentity, ParticipantIdentityId>),
    /// `AddressBook/get`.
    AddressBookGet(GetResponse<AddressBook, AddressBookId>),
    /// `AddressBook/changes`.
    AddressBookChanges(ChangesResponse<AddressBookId>),
    /// `AddressBook/set`.
    AddressBookSet(SetResponse<AddressBook, AddressBookId>),
    /// `ContactCard/get`.
    ContactCardGet(GetResponse<ContactCard, ContactCardId>),
    /// `ContactCard/changes`.
    ContactCardChanges(ChangesResponse<ContactCardId>),
    /// `ContactCard/query`.
    ContactCardQuery(QueryResponse<ContactCardId>),
    /// `ContactCard/queryChanges`.
    ContactCardQueryChanges(QueryChangesResponse<ContactCardId>),
    /// `ContactCard/set`.
    ContactCardSet(SetResponse<ContactCard, ContactCardId>),
    /// `ContactCard/parse`.
    ContactCardParse(ContactCardParseResponse),
    /// `Principal/get`.
    PrincipalGet(GetResponse<Principal, PrincipalId>),
    /// `Principal/query`.
    PrincipalQuery(QueryResponse<PrincipalId>),
    /// `Principal/changes`.
    PrincipalChanges(ChangesResponse<PrincipalId>),
    /// `Principal/getAvailability`.
    PrincipalGetAvailability(PrincipalGetAvailabilityResponse),
    /// `PushSubscription/get`.
    PushSubscriptionGet(PushSubscriptionGetResponse),
    /// `SieveScript/get`.
    SieveScriptGet(GetResponse<SieveScript, SieveScriptId>),
    /// `SieveScript/changes`.
    SieveScriptChanges(ChangesResponse<SieveScriptId>),
    /// `SieveScript/query`.
    SieveScriptQuery(QueryResponse<SieveScriptId>),
    /// `SieveScript/set`.
    SieveScriptSet(SetResponse<SieveScript, SieveScriptId>),
    /// `SieveScript/validate`.
    SieveScriptValidate(SieveScriptValidateResponse),
    /// `VacationResponse/get`.
    VacationResponseGet(GetResponse<VacationResponse, VacationResponseId>),
    /// `VacationResponse/set`.
    VacationResponseSet(SetResponse<VacationResponse, VacationResponseId>),
    /// `PushSubscription/set`.
    PushSubscriptionSet(PushSubscriptionSetResponse),
    /// A method-level error.
    Error(MethodError),
}

fn to_map<T: Serialize>(value: &T) -> Result<Map<String, Value>, serde_json::Error> {
    match serde_json::to_value(value)? {
        Value::Object(map) => Ok(map),
        _ => Err(serde::ser::Error::custom(
            "a response must be a JSON object",
        )),
    }
}

impl MethodResponse {
    /// The response name: the method name, or `error`.
    #[must_use]
    pub fn name(&self) -> ResponseName {
        ResponseName::Method(match self {
            Self::CoreEcho(_) => MethodName::CoreEcho,
            Self::MailboxGet(_) => MethodName::MailboxGet,
            Self::MailboxChanges(_) => MethodName::MailboxChanges,
            Self::MailboxQuery(_) => MethodName::MailboxQuery,
            Self::MailboxQueryChanges(_) => MethodName::MailboxQueryChanges,
            Self::MailboxSet(_) => MethodName::MailboxSet,
            Self::EmailGet(_) => MethodName::EmailGet,
            Self::EmailChanges(_) => MethodName::EmailChanges,
            Self::EmailQuery(_) => MethodName::EmailQuery,
            Self::EmailQueryChanges(_) => MethodName::EmailQueryChanges,
            Self::EmailSet(_) => MethodName::EmailSet,
            Self::EmailImport(_) => MethodName::EmailImport,
            Self::EmailParse(_) => MethodName::EmailParse,
            Self::BlobCopy(_) => MethodName::BlobCopy,
            Self::ThreadGet(_) => MethodName::ThreadGet,
            Self::ThreadChanges(_) => MethodName::ThreadChanges,
            Self::SearchSnippetGet(_) => MethodName::SearchSnippetGet,
            Self::IdentityGet(_) => MethodName::IdentityGet,
            Self::IdentityChanges(_) => MethodName::IdentityChanges,
            Self::IdentitySet(_) => MethodName::IdentitySet,
            Self::EmailSubmissionGet(_) => MethodName::EmailSubmissionGet,
            Self::EmailSubmissionQuery(_) => MethodName::EmailSubmissionQuery,
            Self::EmailSubmissionQueryChanges(_) => MethodName::EmailSubmissionQueryChanges,
            Self::EmailSubmissionSet(_) => MethodName::EmailSubmissionSet,
            Self::QuotaGet(_) => MethodName::QuotaGet,
            Self::QuotaChanges(_) => MethodName::QuotaChanges,
            Self::QuotaQuery(_) => MethodName::QuotaQuery,
            Self::QuotaQueryChanges(_) => MethodName::QuotaQueryChanges,
            Self::MdnSend(_) => MethodName::MdnSend,
            Self::MdnParse(_) => MethodName::MdnParse,
            Self::CalendarGet(_) => MethodName::CalendarGet,
            Self::CalendarChanges(_) => MethodName::CalendarChanges,
            Self::CalendarSet(_) => MethodName::CalendarSet,
            Self::CalendarEventGet(_) => MethodName::CalendarEventGet,
            Self::CalendarEventChanges(_) => MethodName::CalendarEventChanges,
            Self::CalendarEventQuery(_) => MethodName::CalendarEventQuery,
            Self::CalendarEventQueryChanges(_) => MethodName::CalendarEventQueryChanges,
            Self::CalendarEventSet(_) => MethodName::CalendarEventSet,
            Self::CalendarEventParse(_) => MethodName::CalendarEventParse,
            Self::CalendarEventCounter(_) => MethodName::CalendarEventCounter,
            Self::CalendarEventDeclineCounter(_) => MethodName::CalendarEventDeclineCounter,
            Self::CalendarTrashGet(_) => MethodName::CalendarTrashGet,
            Self::CalendarTrashSet(_) => MethodName::CalendarTrashSet,
            Self::ParticipantIdentityGet(_) => MethodName::ParticipantIdentityGet,
            Self::AddressBookGet(_) => MethodName::AddressBookGet,
            Self::AddressBookChanges(_) => MethodName::AddressBookChanges,
            Self::AddressBookSet(_) => MethodName::AddressBookSet,
            Self::ContactCardGet(_) => MethodName::ContactCardGet,
            Self::ContactCardChanges(_) => MethodName::ContactCardChanges,
            Self::ContactCardQuery(_) => MethodName::ContactCardQuery,
            Self::ContactCardQueryChanges(_) => MethodName::ContactCardQueryChanges,
            Self::ContactCardSet(_) => MethodName::ContactCardSet,
            Self::ContactCardParse(_) => MethodName::ContactCardParse,
            Self::PrincipalGet(_) => MethodName::PrincipalGet,
            Self::PrincipalQuery(_) => MethodName::PrincipalQuery,
            Self::PrincipalChanges(_) => MethodName::PrincipalChanges,
            Self::PrincipalGetAvailability(_) => MethodName::PrincipalGetAvailability,
            Self::PushSubscriptionGet(_) => MethodName::PushSubscriptionGet,
            Self::SieveScriptGet(_) => MethodName::SieveScriptGet,
            Self::SieveScriptChanges(_) => MethodName::SieveScriptChanges,
            Self::SieveScriptQuery(_) => MethodName::SieveScriptQuery,
            Self::SieveScriptSet(_) => MethodName::SieveScriptSet,
            Self::SieveScriptValidate(_) => MethodName::SieveScriptValidate,
            Self::VacationResponseGet(_) => MethodName::VacationResponseGet,
            Self::VacationResponseSet(_) => MethodName::VacationResponseSet,
            Self::PushSubscriptionSet(_) => MethodName::PushSubscriptionSet,
            Self::Error(_) => return ResponseName::Error,
        })
    }

    /// The response arguments as a JSON object.
    ///
    /// # Errors
    /// Only if serialization fails, which the types here never do (all map
    /// keys are strings); answer with `serverFail` then.
    pub fn to_arguments(&self) -> Result<Map<String, Value>, serde_json::Error> {
        match self {
            Self::CoreEcho(arguments) => Ok(arguments.clone()),
            Self::MailboxGet(r) => to_map(r),
            Self::MailboxChanges(r) => to_map(r),
            Self::MailboxQuery(r) => to_map(r),
            Self::MailboxQueryChanges(r) => to_map(r),
            Self::MailboxSet(r) => to_map(r),
            Self::EmailGet(r) => to_map(r),
            Self::EmailChanges(r) => to_map(r),
            Self::EmailQuery(r) => to_map(r),
            Self::EmailQueryChanges(r) => to_map(r),
            Self::EmailSet(r) => to_map(r),
            Self::EmailImport(r) => to_map(r),
            Self::EmailParse(r) => to_map(r),
            Self::BlobCopy(r) => to_map(r),
            Self::ThreadGet(r) => to_map(r),
            Self::ThreadChanges(r) => to_map(r),
            Self::SearchSnippetGet(r) => to_map(r),
            Self::IdentityGet(r) => to_map(r),
            Self::IdentityChanges(r) => to_map(r),
            Self::IdentitySet(r) => to_map(r),
            Self::EmailSubmissionGet(r) => to_map(r),
            Self::EmailSubmissionQuery(r) => to_map(r),
            Self::EmailSubmissionQueryChanges(r) => to_map(r),
            Self::EmailSubmissionSet(r) => to_map(r),
            Self::QuotaGet(r) => to_map(r),
            Self::QuotaChanges(r) => to_map(r),
            Self::QuotaQuery(r) => to_map(r),
            Self::QuotaQueryChanges(r) => to_map(r),
            Self::MdnSend(r) => to_map(r),
            Self::MdnParse(r) => to_map(r),
            Self::CalendarGet(r) => to_map(r),
            Self::CalendarChanges(r) => to_map(r),
            Self::CalendarSet(r) => to_map(r),
            Self::CalendarEventGet(r) => to_map(r),
            Self::CalendarEventChanges(r) => to_map(r),
            Self::CalendarEventQuery(r) => to_map(r),
            Self::CalendarEventQueryChanges(r) => to_map(r),
            Self::CalendarEventSet(r) => to_map(r),
            Self::CalendarEventParse(r) => to_map(r),
            Self::CalendarEventCounter(r) | Self::CalendarEventDeclineCounter(r) => to_map(r),
            Self::CalendarTrashGet(r) => to_map(r),
            Self::CalendarTrashSet(r) => to_map(r),
            Self::ParticipantIdentityGet(r) => to_map(r),
            Self::AddressBookGet(r) => to_map(r),
            Self::AddressBookChanges(r) => to_map(r),
            Self::AddressBookSet(r) => to_map(r),
            Self::ContactCardGet(r) => to_map(r),
            Self::ContactCardChanges(r) => to_map(r),
            Self::ContactCardQuery(r) => to_map(r),
            Self::ContactCardQueryChanges(r) => to_map(r),
            Self::ContactCardSet(r) => to_map(r),
            Self::ContactCardParse(r) => to_map(r),
            Self::PrincipalGet(r) => to_map(r),
            Self::PrincipalQuery(r) => to_map(r),
            Self::PrincipalChanges(r) => to_map(r),
            Self::PrincipalGetAvailability(r) => to_map(r),
            Self::PushSubscriptionGet(r) => to_map(r),
            Self::SieveScriptGet(r) => to_map(r),
            Self::SieveScriptChanges(r) => to_map(r),
            Self::SieveScriptQuery(r) => to_map(r),
            Self::SieveScriptSet(r) => to_map(r),
            Self::SieveScriptValidate(r) => to_map(r),
            Self::VacationResponseGet(r) => to_map(r),
            Self::VacationResponseSet(r) => to_map(r),
            Self::PushSubscriptionSet(r) => to_map(r),
            Self::Error(r) => to_map(r),
        }
    }

    /// The `[name, arguments, callId]` entry for `methodResponses`.
    ///
    /// # Errors
    /// See [`MethodResponse::to_arguments`].
    pub fn into_invocation(self, call_id: CallId) -> Result<ResponseInvocation, serde_json::Error> {
        Ok(ResponseInvocation {
            name: self.name(),
            arguments: self.to_arguments()?,
            call_id,
        })
    }
}

impl From<MethodError> for MethodResponse {
    fn from(error: MethodError) -> Self {
        Self::Error(error)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::State;

    #[test]
    fn invocations() {
        let response = MethodResponse::ThreadGet(GetResponse {
            account_id: "a".parse().unwrap(),
            state: State::new("s"),
            list: vec![Thread {
                id: Some("t1".parse().unwrap()),
                email_ids: Some(vec!["e1".parse().unwrap()]),
            }],
            not_found: vec!["t2".parse().unwrap()],
        });
        let invocation = response.into_invocation(CallId::new("2")).unwrap();
        assert_eq!(
            serde_json::to_value(&invocation).unwrap(),
            json!(["Thread/get", {
                "accountId": "a", "state": "s",
                "list": [{"id": "t1", "emailIds": ["e1"]}],
                "notFound": ["t2"]
            }, "2"])
        );
        let error = MethodResponse::from(MethodError::unknown_method())
            .into_invocation(CallId::new("3"))
            .unwrap();
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            json!(["error", {"type": "unknownMethod"}, "3"])
        );
    }
}
