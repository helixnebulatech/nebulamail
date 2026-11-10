// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Typed method calls: the arguments of each method, parsed.

use serde_json::{Map, Value};

use super::MethodName;
use crate::error::MethodError;
use crate::method::{ChangesArguments, EmailImportArguments, GetArguments, SetArguments};
use crate::object::{
    AddressBookProperty, AddressBookSetArguments, CalendarEventCounterArguments,
    CalendarEventDeclineCounterArguments, CalendarEventGetArguments, CalendarEventParseArguments,
    CalendarEventQueryArguments, CalendarEventQueryChangesArguments, CalendarEventSetArguments,
    CalendarProperty, CalendarSetArguments, CalendarTrashGetArguments, CalendarTrashSetArguments,
    ContactCardParseArguments, ContactCardProperty, ContactCardQueryArguments,
    ContactCardQueryChangesArguments, EmailGetArguments, EmailQueryArguments,
    EmailQueryChangesArguments, EmailSubmissionProperty, EmailSubmissionQueryArguments,
    EmailSubmissionQueryChangesArguments, EmailSubmissionSetArguments, IdentityProperty,
    MailboxProperty, MailboxQueryArguments, MailboxSetArguments, MdnParseArguments,
    MdnSendArguments, ParticipantIdentityProperty, PrincipalGetAvailabilityArguments,
    PrincipalProperty, PrincipalQueryArguments, PushSubscriptionGetArguments,
    PushSubscriptionSetArguments, QuotaProperty, QuotaQueryArguments, QuotaQueryChangesArguments,
    SearchSnippetGetArguments, SieveScriptProperty, SieveScriptQueryArguments,
    SieveScriptSetArguments, SieveScriptValidateArguments, ThreadProperty,
    VacationResponseProperty,
};
use crate::types::{
    AddressBookId, CalendarId, ContactCardId, EmailId, EmailSubmissionId, IdentityId, MailboxId,
    ParticipantIdentityId, PrincipalId, QuotaId, SieveScriptId, ThreadId, VacationResponseId,
};

/// Per-method limits from the core capability (RFC 8620 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodLimits {
    /// `maxObjectsInGet`: most ids in one `/get` (and `SearchSnippet/get`).
    pub max_objects_in_get: usize,
    /// `maxObjectsInSet`: most creates + updates + destroys in one `/set`
    /// (and emails in one `Email/import`).
    pub max_objects_in_set: usize,
    /// `maxSizeRequest`: also the most the result references of one
    /// request may copy (see [`ReferenceBudget`](crate::ReferenceBudget)).
    pub max_size_request: usize,
}

/// A method call with typed arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MethodCall {
    /// `Core/echo`: the arguments are returned as they are.
    CoreEcho(Map<String, Value>),
    /// `Mailbox/get`.
    MailboxGet(GetArguments<MailboxId, MailboxProperty>),
    /// `Mailbox/changes`.
    MailboxChanges(ChangesArguments),
    /// `Mailbox/query`.
    MailboxQuery(MailboxQueryArguments),
    /// `Mailbox/queryChanges`.
    MailboxQueryChanges(crate::object::MailboxQueryChangesArguments),
    /// `Mailbox/set`.
    MailboxSet(MailboxSetArguments),
    /// `Email/get`.
    EmailGet(EmailGetArguments),
    /// `Email/changes`.
    EmailChanges(ChangesArguments),
    /// `Email/query`.
    EmailQuery(EmailQueryArguments),
    /// `Email/queryChanges`.
    EmailQueryChanges(EmailQueryChangesArguments),
    /// `Email/set` (creates parse with `EmailCreate::parse`, updates with
    /// `EmailPatch::parse`).
    EmailSet(SetArguments<EmailId>),
    /// `Email/import`.
    EmailImport(EmailImportArguments),
    /// `Email/copy`.
    EmailCopy(crate::method::CopyArguments),
    /// `Email/parse`.
    EmailParse(crate::object::EmailParseArguments),
    /// `Blob/copy`.
    BlobCopy(crate::method::BlobCopyArguments),
    /// `Thread/get`.
    ThreadGet(GetArguments<ThreadId, ThreadProperty>),
    /// `Thread/changes`.
    ThreadChanges(ChangesArguments),
    /// `SearchSnippet/get`.
    SearchSnippetGet(SearchSnippetGetArguments),
    /// `Identity/get`.
    IdentityGet(GetArguments<IdentityId, IdentityProperty>),
    /// `Identity/changes`.
    IdentityChanges(ChangesArguments),
    /// `Identity/set` (`IdentityCreate::parse`, `IdentityPatch::parse`).
    IdentitySet(SetArguments<IdentityId>),
    /// `EmailSubmission/get`.
    EmailSubmissionGet(GetArguments<EmailSubmissionId, EmailSubmissionProperty>),
    /// `EmailSubmission/query`.
    EmailSubmissionQuery(EmailSubmissionQueryArguments),
    /// `EmailSubmission/queryChanges`.
    EmailSubmissionQueryChanges(EmailSubmissionQueryChangesArguments),
    /// `EmailSubmission/set`.
    EmailSubmissionSet(EmailSubmissionSetArguments),
    /// `Quota/get`.
    QuotaGet(GetArguments<QuotaId, QuotaProperty>),
    /// `Quota/changes`.
    QuotaChanges(ChangesArguments),
    /// `Quota/query`.
    QuotaQuery(QuotaQueryArguments),
    /// `Quota/queryChanges`.
    QuotaQueryChanges(QuotaQueryChangesArguments),
    /// `MDN/send`.
    MdnSend(MdnSendArguments),
    /// `MDN/parse`.
    MdnParse(MdnParseArguments),
    /// `Calendar/get`.
    CalendarGet(GetArguments<CalendarId, CalendarProperty>),
    /// `Calendar/changes`.
    CalendarChanges(ChangesArguments),
    /// `Calendar/set`.
    CalendarSet(CalendarSetArguments),
    /// `CalendarEvent/get`.
    CalendarEventGet(CalendarEventGetArguments),
    /// `CalendarEvent/changes`.
    CalendarEventChanges(ChangesArguments),
    /// `CalendarEvent/query`.
    CalendarEventQuery(CalendarEventQueryArguments),
    /// `CalendarEvent/queryChanges`.
    CalendarEventQueryChanges(CalendarEventQueryChangesArguments),
    /// `CalendarEvent/set`.
    CalendarEventSet(CalendarEventSetArguments),
    /// `CalendarEvent/parse`.
    CalendarEventParse(CalendarEventParseArguments),
    /// `CalendarEvent/counter`.
    CalendarEventCounter(CalendarEventCounterArguments),
    /// `CalendarEvent/declineCounter`.
    CalendarEventDeclineCounter(CalendarEventDeclineCounterArguments),
    /// `CalendarTrash/get`.
    CalendarTrashGet(CalendarTrashGetArguments),
    /// `CalendarTrash/set`.
    CalendarTrashSet(CalendarTrashSetArguments),
    /// `ParticipantIdentity/get`.
    ParticipantIdentityGet(GetArguments<ParticipantIdentityId, ParticipantIdentityProperty>),
    /// `AddressBook/get`.
    AddressBookGet(GetArguments<AddressBookId, AddressBookProperty>),
    /// `AddressBook/changes`.
    AddressBookChanges(ChangesArguments),
    /// `AddressBook/set`.
    AddressBookSet(AddressBookSetArguments),
    /// `ContactCard/get`.
    ContactCardGet(GetArguments<ContactCardId, ContactCardProperty>),
    /// `ContactCard/changes`.
    ContactCardChanges(ChangesArguments),
    /// `ContactCard/query`.
    ContactCardQuery(ContactCardQueryArguments),
    /// `ContactCard/queryChanges`.
    ContactCardQueryChanges(ContactCardQueryChangesArguments),
    /// `ContactCard/set`.
    ContactCardSet(SetArguments<ContactCardId>),
    /// `ContactCard/parse`.
    ContactCardParse(ContactCardParseArguments),
    /// `Principal/get`.
    PrincipalGet(GetArguments<PrincipalId, PrincipalProperty>),
    /// `Principal/query`.
    PrincipalQuery(PrincipalQueryArguments),
    /// `Principal/changes`.
    PrincipalChanges(ChangesArguments),
    /// `Principal/getAvailability`.
    PrincipalGetAvailability(PrincipalGetAvailabilityArguments),
    /// `SieveScript/get`.
    SieveScriptGet(GetArguments<SieveScriptId, SieveScriptProperty>),
    /// `SieveScript/changes`.
    SieveScriptChanges(ChangesArguments),
    /// `SieveScript/query`.
    SieveScriptQuery(SieveScriptQueryArguments),
    /// `SieveScript/set` (`SieveScriptCreate::parse`, `SieveScriptPatch::parse`).
    SieveScriptSet(SieveScriptSetArguments),
    /// `SieveScript/validate`.
    SieveScriptValidate(SieveScriptValidateArguments),
    /// `VacationResponse/get`.
    VacationResponseGet(GetArguments<VacationResponseId, VacationResponseProperty>),
    /// `VacationResponse/set` (`VacationResponsePatch::parse`).
    VacationResponseSet(SetArguments<VacationResponseId>),
    /// `PushSubscription/get`.
    PushSubscriptionGet(PushSubscriptionGetArguments),
    /// `PushSubscription/set` (creates parse with
    /// `PushSubscriptionCreate::parse`, updates with
    /// `PushSubscriptionPatch::parse`).
    PushSubscriptionSet(PushSubscriptionSetArguments),
}

impl MethodCall {
    /// Parses the arguments of method `name`. Resolve result references
    /// first.
    ///
    /// # Errors
    /// `unknownMethod` for methods this crate doesn't know;
    /// `invalidArguments`, `requestTooLarge`, `unsupportedFilter` or
    /// `unsupportedSort` for bad arguments.
    #[expect(
        clippy::too_many_lines,
        reason = "one short arm per method: a flat match reads best"
    )]
    pub fn parse(
        name: &MethodName,
        arguments: Map<String, Value>,
        limits: &MethodLimits,
    ) -> Result<Self, MethodError> {
        let get = limits.max_objects_in_get;
        let set = limits.max_objects_in_set;
        Ok(match name {
            MethodName::CoreEcho => Self::CoreEcho(arguments),
            MethodName::MailboxGet => Self::MailboxGet(GetArguments::parse(arguments, get)?),
            MethodName::MailboxChanges => Self::MailboxChanges(ChangesArguments::parse(arguments)?),
            MethodName::MailboxQuery => {
                Self::MailboxQuery(MailboxQueryArguments::parse(arguments)?)
            }
            MethodName::MailboxQueryChanges => Self::MailboxQueryChanges(
                crate::object::MailboxQueryChangesArguments::parse(arguments)?,
            ),
            MethodName::MailboxSet => Self::MailboxSet(MailboxSetArguments::parse(arguments, set)?),
            MethodName::EmailGet => Self::EmailGet(EmailGetArguments::parse(arguments, get)?),
            MethodName::EmailChanges => Self::EmailChanges(ChangesArguments::parse(arguments)?),
            MethodName::EmailQuery => Self::EmailQuery(EmailQueryArguments::parse(arguments)?),
            MethodName::EmailQueryChanges => {
                Self::EmailQueryChanges(EmailQueryChangesArguments::parse(arguments)?)
            }
            MethodName::EmailSet => Self::EmailSet(SetArguments::parse(arguments, set)?),
            MethodName::EmailImport => {
                Self::EmailImport(EmailImportArguments::parse(arguments, set)?)
            }
            MethodName::EmailCopy => {
                Self::EmailCopy(crate::method::CopyArguments::parse(arguments, set)?)
            }
            MethodName::EmailParse => {
                Self::EmailParse(crate::object::EmailParseArguments::parse(arguments, get)?)
            }
            MethodName::BlobCopy => {
                Self::BlobCopy(crate::method::BlobCopyArguments::parse(arguments, set)?)
            }
            MethodName::ThreadGet => Self::ThreadGet(GetArguments::parse(arguments, get)?),
            MethodName::ThreadChanges => Self::ThreadChanges(ChangesArguments::parse(arguments)?),
            MethodName::SearchSnippetGet => {
                Self::SearchSnippetGet(SearchSnippetGetArguments::parse(arguments, get)?)
            }
            MethodName::IdentityGet => Self::IdentityGet(GetArguments::parse(arguments, get)?),
            MethodName::IdentityChanges => {
                Self::IdentityChanges(ChangesArguments::parse(arguments)?)
            }
            MethodName::IdentitySet => Self::IdentitySet(SetArguments::parse(arguments, set)?),
            MethodName::EmailSubmissionGet => {
                Self::EmailSubmissionGet(GetArguments::parse(arguments, get)?)
            }
            MethodName::EmailSubmissionQuery => {
                Self::EmailSubmissionQuery(EmailSubmissionQueryArguments::parse(arguments)?)
            }
            MethodName::EmailSubmissionQueryChanges => Self::EmailSubmissionQueryChanges(
                EmailSubmissionQueryChangesArguments::parse(arguments)?,
            ),
            MethodName::EmailSubmissionSet => {
                Self::EmailSubmissionSet(EmailSubmissionSetArguments::parse(arguments, set)?)
            }
            MethodName::QuotaGet => Self::QuotaGet(GetArguments::parse(arguments, get)?),
            MethodName::QuotaChanges => Self::QuotaChanges(ChangesArguments::parse(arguments)?),
            MethodName::QuotaQuery => Self::QuotaQuery(QuotaQueryArguments::parse(arguments)?),
            MethodName::QuotaQueryChanges => {
                Self::QuotaQueryChanges(QuotaQueryChangesArguments::parse(arguments)?)
            }
            MethodName::MdnSend => Self::MdnSend(MdnSendArguments::parse(arguments, set)?),
            MethodName::MdnParse => Self::MdnParse(MdnParseArguments::parse(arguments, get)?),
            MethodName::CalendarGet => Self::CalendarGet(GetArguments::parse(arguments, get)?),
            MethodName::CalendarChanges => {
                Self::CalendarChanges(ChangesArguments::parse(arguments)?)
            }
            MethodName::CalendarSet => {
                Self::CalendarSet(CalendarSetArguments::parse(arguments, set)?)
            }
            MethodName::CalendarEventGet => {
                Self::CalendarEventGet(CalendarEventGetArguments::parse(arguments, get)?)
            }
            MethodName::CalendarEventChanges => {
                Self::CalendarEventChanges(ChangesArguments::parse(arguments)?)
            }
            MethodName::CalendarEventQuery => {
                Self::CalendarEventQuery(CalendarEventQueryArguments::parse(arguments)?)
            }
            MethodName::CalendarEventQueryChanges => Self::CalendarEventQueryChanges(
                CalendarEventQueryChangesArguments::parse(arguments)?,
            ),
            MethodName::CalendarEventSet => {
                Self::CalendarEventSet(CalendarEventSetArguments::parse(arguments, set)?)
            }
            MethodName::CalendarEventParse => {
                Self::CalendarEventParse(CalendarEventParseArguments::parse(arguments, get)?)
            }
            MethodName::CalendarEventCounter => {
                Self::CalendarEventCounter(CalendarEventCounterArguments::parse(arguments)?)
            }
            MethodName::CalendarEventDeclineCounter => Self::CalendarEventDeclineCounter(
                CalendarEventDeclineCounterArguments::parse(arguments)?,
            ),
            MethodName::CalendarTrashGet => {
                Self::CalendarTrashGet(CalendarTrashGetArguments::parse(arguments)?)
            }
            MethodName::CalendarTrashSet => {
                Self::CalendarTrashSet(CalendarTrashSetArguments::parse(arguments, set)?)
            }
            MethodName::ParticipantIdentityGet => {
                Self::ParticipantIdentityGet(GetArguments::parse(arguments, get)?)
            }
            MethodName::AddressBookGet => {
                Self::AddressBookGet(GetArguments::parse(arguments, get)?)
            }
            MethodName::AddressBookChanges => {
                Self::AddressBookChanges(ChangesArguments::parse(arguments)?)
            }
            MethodName::AddressBookSet => {
                Self::AddressBookSet(AddressBookSetArguments::parse(arguments, set)?)
            }
            MethodName::ContactCardGet => {
                Self::ContactCardGet(GetArguments::parse(arguments, get)?)
            }
            MethodName::ContactCardChanges => {
                Self::ContactCardChanges(ChangesArguments::parse(arguments)?)
            }
            MethodName::ContactCardQuery => {
                Self::ContactCardQuery(ContactCardQueryArguments::parse(arguments)?)
            }
            MethodName::ContactCardQueryChanges => {
                Self::ContactCardQueryChanges(ContactCardQueryChangesArguments::parse(arguments)?)
            }
            MethodName::ContactCardSet => {
                Self::ContactCardSet(SetArguments::parse(arguments, set)?)
            }
            MethodName::ContactCardParse => {
                Self::ContactCardParse(ContactCardParseArguments::parse(arguments, get)?)
            }
            MethodName::PrincipalGet => Self::PrincipalGet(GetArguments::parse(arguments, get)?),
            MethodName::PrincipalQuery => {
                Self::PrincipalQuery(PrincipalQueryArguments::parse(arguments)?)
            }
            MethodName::PrincipalChanges => {
                Self::PrincipalChanges(ChangesArguments::parse(arguments)?)
            }
            MethodName::PrincipalGetAvailability => {
                Self::PrincipalGetAvailability(PrincipalGetAvailabilityArguments::parse(arguments)?)
            }
            MethodName::SieveScriptGet => {
                Self::SieveScriptGet(GetArguments::parse(arguments, get)?)
            }
            MethodName::SieveScriptChanges => {
                Self::SieveScriptChanges(ChangesArguments::parse(arguments)?)
            }
            MethodName::SieveScriptQuery => {
                Self::SieveScriptQuery(SieveScriptQueryArguments::parse(arguments)?)
            }
            MethodName::SieveScriptSet => {
                Self::SieveScriptSet(SieveScriptSetArguments::parse(arguments, set)?)
            }
            MethodName::SieveScriptValidate => {
                Self::SieveScriptValidate(SieveScriptValidateArguments::parse(arguments)?)
            }
            MethodName::VacationResponseGet => {
                Self::VacationResponseGet(GetArguments::parse(arguments, get)?)
            }
            MethodName::VacationResponseSet => {
                Self::VacationResponseSet(SetArguments::parse(arguments, set)?)
            }
            MethodName::PushSubscriptionGet => {
                Self::PushSubscriptionGet(PushSubscriptionGetArguments::parse(arguments, get)?)
            }
            MethodName::PushSubscriptionSet => {
                Self::PushSubscriptionSet(PushSubscriptionSetArguments::parse(arguments, set)?)
            }
            MethodName::Unknown(_) => return Err(MethodError::unknown_method()),
        })
    }

    /// For every standard `/get`: resolves the `#creationId`s `created`
    /// knows and drops repeated ids (RFC 8620 §5.1, see
    /// [`GetArguments::resolve_ids`]), so each id is answered once.
    pub fn resolve_get_ids(&mut self, created: &crate::CreatedIds) {
        match self {
            Self::MailboxGet(args) => args.resolve_ids(created),
            Self::EmailGet(args) => args.get.resolve_ids(created),
            Self::ThreadGet(args) => args.resolve_ids(created),
            Self::IdentityGet(args) => args.resolve_ids(created),
            Self::EmailSubmissionGet(args) => args.resolve_ids(created),
            Self::QuotaGet(args) => args.resolve_ids(created),
            Self::CalendarGet(args) => args.resolve_ids(created),
            Self::CalendarEventGet(args) => args.get.resolve_ids(created),
            Self::ParticipantIdentityGet(args) => args.resolve_ids(created),
            Self::AddressBookGet(args) => args.resolve_ids(created),
            Self::ContactCardGet(args) => args.resolve_ids(created),
            Self::PrincipalGet(args) => args.resolve_ids(created),
            Self::SieveScriptGet(args) => args.resolve_ids(created),
            Self::VacationResponseGet(args) => args.resolve_ids(created),
            // `PushSubscription/get` ids have no `#creationId` form: its
            // repeats are dropped when parsed.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType;
    use crate::references::resolve_result_references;
    use crate::request::Capability;
    use crate::request::{Request, RequestLimits};
    use crate::response::{MethodResponse, ResponseInvocation};

    const LIMITS: MethodLimits = MethodLimits {
        max_objects_in_get: 500,
        max_objects_in_set: 500,
        max_size_request: 10_000_000,
    };

    #[test]
    fn unknown_and_invalid() {
        let unknown = MethodCall::parse(&"Foo/bar".parse().unwrap(), Map::new(), &LIMITS);
        assert_eq!(unknown.unwrap_err().kind(), MethodErrorType::UnknownMethod);
        let invalid = MethodCall::parse(&MethodName::EmailGet, Map::new(), &LIMITS);
        assert_eq!(
            invalid.unwrap_err().kind(),
            MethodErrorType::InvalidArguments
        );
    }

    /// Runs a request body through parse, references and typed arguments,
    /// answering each call with `fake`.
    fn run(
        body: &Value,
        using: &[Capability],
        fake: impl Fn(&MethodCall) -> Value,
    ) -> Vec<MethodCall> {
        let request = Request::parse(
            body.to_string().as_bytes(),
            &RequestLimits {
                max_size_request: 1 << 20,
                max_calls_in_request: 16,
            },
            using,
        )
        .unwrap();
        let mut responses: Vec<ResponseInvocation> = Vec::new();
        let mut budget = crate::ReferenceBudget::new(LIMITS.max_size_request);
        let mut calls = Vec::new();
        let enabled: Vec<bool> = request
            .method_calls
            .iter()
            .map(|call| request.is_method_enabled(&call.name))
            .collect();
        assert!(enabled.iter().all(|on| *on), "{enabled:?}");
        for invocation in request.method_calls {
            assert!(
                invocation.name.capability().is_some(),
                "{}",
                invocation.name
            );
            let mut arguments = invocation.arguments;
            resolve_result_references(&mut arguments, &responses, &mut budget).unwrap();
            let call = MethodCall::parse(&invocation.name, arguments, &LIMITS).unwrap();
            responses.push(ResponseInvocation {
                name: crate::response::ResponseName::Method(invocation.name.clone()),
                arguments: fake(&call).as_object().unwrap().clone(),
                call_id: invocation.call_id,
            });
            calls.push(call);
        }
        calls
    }

    /// The calendar requests of apps/web/src/calendar (api.ts,
    /// invitations.ts), with the `using` jmap-client sends.
    #[test]
    fn web_client_calendar_requests() {
        let using = [
            Capability::Core,
            Capability::Calendars,
            Capability::CalendarsParse,
        ];
        let body = json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars",
                      "urn:ietf:params:jmap:calendars:parse"],
            "methodCalls": [
                ["Calendar/get", {"accountId": "c"}, "0"],
                ["ParticipantIdentity/get", {"accountId": "c"}, "1"],
                ["CalendarEvent/query", {
                    "accountId": "c", "expandRecurrences": true,
                    "filter": {"after": "2026-10-19T00:00:00", "before": "2026-11-03T00:00:00"},
                    "sort": [{"isAscending": true, "property": "start"}],
                    "timeZone": "Europe/Madrid"
                }, "2"],
                ["CalendarEvent/get", {
                    "accountId": "c",
                    "#ids": {"name": "CalendarEvent/query", "path": "/ids", "resultOf": "2"},
                    "properties": ["calendarIds", "title", "start", "duration", "utcStart", "utcEnd",
                                   "baseEventId", "recurrenceId", "status", "participants",
                                   "organizerCalendarAddress", "locations", "isOrigin"],
                    "timeZone": "Europe/Madrid"
                }, "3"],
                ["CalendarEvent/get", {
                    "#ids": {"name": "CalendarEvent/get", "path": "/list/*/baseEventId", "resultOf": "3"},
                    "accountId": "c",
                    "properties": ["calendarIds", "title", "showWithoutTime", "timeZone",
                                   "recurrenceRule", "organizerCalendarAddress", "participants",
                                   "status", "isOrigin"]
                }, "4"],
                ["CalendarEvent/parse", {"accountId": "c", "blobIds": ["Gb1"]}, "5"],
                ["CalendarEvent/set", {
                    "accountId": "c", "sendSchedulingMessages": true,
                    "update": {"h1m9leaaaaal": {"alerts/a1/acknowledged": "2026-10-26T20:49:00Z"}}
                }, "6"],
                ["Calendar/set", {"accountId": "c", "destroy": ["x"], "onDestroyRemoveEvents": true}, "7"]
            ]
        });
        let calls = run(&body, &using, |call| match call {
            MethodCall::CalendarEventQuery(_) => json!({"ids": ["h1m9leaaaaal", "n"]}),
            MethodCall::CalendarEventGet(get)
                if get.get.properties.as_ref().is_some_and(|p| p.len() == 13) =>
            {
                json!({"list": [{"id": "h1m9leaaaaal", "baseEventId": "l"}, {"id": "h1nihuaaaaam", "baseEventId": "m"}]})
            }
            _ => json!({}),
        });
        assert_eq!(calls.len(), 8);
        let MethodCall::CalendarEventGet(series) = &calls[4] else {
            panic!("expected CalendarEvent/get")
        };
        // Like Stalwart, every expanded occurrence (also of a single
        // event) must have `baseEventId`: a `null` in `/list/*/baseEventId`
        // would make the reference an invalid list of ids.
        assert_eq!(series.get.ids.as_ref().unwrap().len(), 2);
        let MethodCall::CalendarEventSet(set) = &calls[6] else {
            panic!("expected CalendarEvent/set")
        };
        let patch =
            crate::object::CalendarEventPatch::parse(set.set.update.values().next().unwrap())
                .unwrap();
        assert_eq!(patch.body.len(), 1);
        assert!(matches!(calls[7], MethodCall::CalendarSet(ref s) if s.on_destroy_remove_events));
    }

    /// Without `calendars:parse` in `using`, `CalendarEvent/parse` is
    /// `unknownMethod`; contacts likewise.
    #[test]
    fn parse_methods_need_their_capability() {
        let request = Request::parse(
            json!({
                "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:calendars",
                          "urn:ietf:params:jmap:contacts"],
                "methodCalls": [
                    ["CalendarEvent/parse", {"accountId": "c", "blobIds": []}, "0"],
                    ["ContactCard/parse", {"accountId": "c", "blobIds": []}, "1"],
                    ["CalendarEvent/query", {"accountId": "c"}, "2"]
                ]
            })
            .to_string()
            .as_bytes(),
            &RequestLimits {
                max_size_request: 1 << 20,
                max_calls_in_request: 16,
            },
            &[
                Capability::Core,
                Capability::Calendars,
                Capability::CalendarsParse,
                Capability::Contacts,
                Capability::ContactsParse,
            ],
        )
        .unwrap();
        let enabled: Vec<bool> = request
            .method_calls
            .iter()
            .map(|call| request.is_method_enabled(&call.name))
            .collect();
        assert_eq!(enabled, [false, false, true]);
    }

    /// The principal requests of the web: «Ocupado» in the event editor
    /// (calendar/api.ts `getAvailability`, with jmap-client's `using`) and
    /// the people to share with (mail/sharing.ts `listPeople`).
    #[test]
    fn web_client_principal_requests() {
        let using = [
            Capability::Core,
            Capability::Principals,
            Capability::PrincipalsAvailability,
            Capability::Calendars,
        ];
        let body = json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:principals",
                      "urn:ietf:params:jmap:principals:availability",
                      "urn:ietf:params:jmap:calendars"],
            "methodCalls": [
                ["Principal/query", {"accountId": "c", "filter": {"email": "bob@nebulamail.test"},
                                     "limit": 1}, "0"],
                ["Principal/getAvailability", {"accountId": "c", "id": "r12",
                    "utcStart": "2026-10-28T10:00:00Z", "utcEnd": "2026-10-28T11:00:00Z"}, "1"],
                ["Principal/query", {"accountId": "c", "limit": 500}, "2"],
                ["Principal/get", {"accountId": "c",
                    "#ids": {"resultOf": "2", "name": "Principal/query", "path": "/ids"},
                    "properties": ["name", "email", "type"]}, "3"]
            ]
        });
        let calls = run(&body, &using, |call| match call {
            MethodCall::PrincipalQuery(_) => json!({"ids": ["r12", "r40"]}),
            _ => json!({}),
        });
        assert!(matches!(calls[0], MethodCall::PrincipalQuery(_)));
        let MethodCall::PrincipalGetAvailability(availability) = &calls[1] else {
            panic!("{:?}", calls[1]);
        };
        assert_eq!(availability.id.as_str(), "r12");
        let MethodCall::PrincipalGet(get) = &calls[3] else {
            panic!("{:?}", calls[3]);
        };
        assert_eq!(get.ids.as_ref().map(Vec::len), Some(2));
        // Without `principals:availability` in `using`, getAvailability is
        // not enabled.
        let request = Request::parse(
            json!({"using": ["urn:ietf:params:jmap:principals"],
                   "methodCalls": [["Principal/getAvailability", {}, "0"]]})
            .to_string()
            .as_bytes(),
            &RequestLimits {
                max_size_request: 1 << 20,
                max_calls_in_request: 16,
            },
            &using,
        )
        .unwrap();
        assert!(!request.is_method_enabled(&request.method_calls[0].name));
    }

    /// The contact requests of apps/web/src/contacts and mail/contacts.ts.
    #[test]
    fn web_client_contact_requests() {
        let using = [
            Capability::Core,
            Capability::Contacts,
            Capability::ContactsParse,
        ];
        let body = json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:contacts",
                      "urn:ietf:params:jmap:contacts:parse"],
            "methodCalls": [
                ["AddressBook/get", {"accountId": "c", "properties": ["name", "isDefault", "myRights", "sortOrder"]}, "0"],
                ["ContactCard/query", {"accountId": "c", "calculateTotal": true, "limit": 200, "position": 0}, "1"],
                ["ContactCard/get", {
                    "#ids": {"name": "ContactCard/query", "path": "/ids", "resultOf": "1"},
                    "accountId": "c",
                    "properties": ["uid", "kind", "name", "emails", "phones", "organizations",
                                   "addressBookIds", "members"]
                }, "2"],
                ["ContactCard/parse", {"accountId": "c", "blobIds": ["G1", "G2"]}, "3"],
                ["ContactCard/set", {"accountId": "c", "create": {"c0": {
                    "@type": "Card", "version": "1.0", "addressBookIds": {"b": true},
                    "uid": "urn:uuid:1", "kind": "individual", "name": {"full": "X"}
                }}, "update": {"b": {"members": null}}, "destroy": ["c"]}, "4"],
                ["AddressBook/set", {"accountId": "c", "update": {"b": {"shareWith/d": null}}}, "5"],
                ["AddressBook/changes", {"accountId": "c", "sinceState": "s3i0a"}, "6"],
                ["ContactCard/changes", {"accountId": "c", "sinceState": "s1q0a", "maxChanges": 100}, "7"]
            ]
        });
        let calls = run(&body, &using, |call| match call {
            MethodCall::ContactCardQuery(_) => json!({"ids": ["b", "c"]}),
            _ => json!({}),
        });
        let MethodCall::ContactCardGet(get) = &calls[2] else {
            panic!("expected ContactCard/get")
        };
        assert_eq!(get.ids.as_ref().unwrap().len(), 2);
        let MethodCall::ContactCardSet(set) = &calls[4] else {
            panic!("expected ContactCard/set")
        };
        let create = set.create.values().next().unwrap().clone();
        assert!(crate::object::ContactCardCreate::parse(create).is_ok());
        assert!(matches!(calls[7], MethodCall::ContactCardChanges(_)));
    }

    /// Typed responses of the new methods turn into invocations.
    #[test]
    fn calendar_and_contact_responses() {
        use crate::method::{ChangesResponse, GetResponse};
        use crate::types::State;
        let response = MethodResponse::CalendarGet(GetResponse {
            account_id: "c".parse().unwrap(),
            state: State::new("s3u0a"),
            list: vec![crate::object::Calendar {
                id: Some("b".parse().unwrap()),
                name: Some("Personal".to_owned()),
                ..crate::object::Calendar::default()
            }],
            not_found: vec![],
        });
        let invocation = response
            .into_invocation(crate::types::CallId::new("0"))
            .unwrap();
        assert_eq!(
            serde_json::to_value(&invocation).unwrap(),
            json!(["Calendar/get", {"accountId": "c", "state": "s3u0a",
                   "list": [{"id": "b", "name": "Personal"}], "notFound": []}, "0"])
        );
        let changes = MethodResponse::ContactCardChanges(ChangesResponse {
            account_id: "c".parse().unwrap(),
            old_state: State::new("1"),
            new_state: State::new("2"),
            has_more_changes: false,
            created: vec!["k".parse().unwrap()],
            updated: vec![],
            destroyed: vec![],
        });
        assert_eq!(changes.name().as_str(), "ContactCard/changes");
        let parse = MethodResponse::CalendarEventParse(crate::object::CalendarEventParseResponse {
            account_id: "c".parse().unwrap(),
            parsed: std::collections::BTreeMap::new(),
            not_parsable: vec![],
            not_found: vec!["G".parse().unwrap()],
        });
        assert_eq!(
            parse.to_arguments().unwrap(),
            json!({"accountId": "c", "parsed": null, "notParsable": null, "notFound": ["G"]})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    /// The whole flow for the web client's mailbox list request: parse,
    /// resolve references, parse arguments.
    #[test]
    fn web_client_thread_list_request() {
        let body = json!({
            "using": ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            "methodCalls": [
                ["Email/query", {
                    "accountId": "a", "calculateTotal": true, "collapseThreads": true,
                    "filter": {"inMailbox": "m1"}, "limit": 50, "position": 0,
                    "sort": [{"isAscending": false, "property": "receivedAt"}]
                }, "0"],
                ["Email/get", {
                    "#ids": {"name": "Email/query", "path": "/ids", "resultOf": "0"},
                    "accountId": "a",
                    "properties": ["threadId", "subject", "from", "to", "receivedAt",
                                   "preview", "keywords", "hasAttachment", "mailboxIds"]
                }, "1"],
                ["Thread/get", {
                    "#ids": {"name": "Email/get", "path": "/list/*/threadId", "resultOf": "1"},
                    "accountId": "a", "properties": ["emailIds"]
                }, "2"]
            ]
        });
        let request = Request::parse(
            body.to_string().as_bytes(),
            &RequestLimits {
                max_size_request: 1 << 20,
                max_calls_in_request: 16,
            },
            &[Capability::Core, Capability::Mail],
        )
        .unwrap();
        let mut responses: Vec<ResponseInvocation> = Vec::new();
        let mut budget = crate::ReferenceBudget::new(LIMITS.max_size_request);
        let mut calls = Vec::new();
        for invocation in request.method_calls {
            assert!(
                request
                    .using
                    .contains(&invocation.name.capability().unwrap())
            );
            let mut arguments = invocation.arguments;
            resolve_result_references(&mut arguments, &responses, &mut budget).unwrap();
            let call = MethodCall::parse(&invocation.name, arguments, &LIMITS).unwrap();
            // Fake responses, enough for the next references.
            let response = match &call {
                MethodCall::EmailQuery(_) => json!({"ids": ["e1", "e2"]}),
                MethodCall::EmailGet(get) => {
                    assert_eq!(get.get.ids.as_ref().unwrap().len(), 2);
                    json!({"list": [{"id": "e1", "threadId": "t1"}, {"id": "e2", "threadId": "t2"}]})
                }
                MethodCall::ThreadGet(get) => {
                    assert_eq!(get.ids.as_ref().unwrap().len(), 2);
                    json!({"list": []})
                }
                other => panic!("unexpected {other:?}"),
            };
            responses.push(ResponseInvocation {
                name: crate::response::ResponseName::Method(invocation.name.clone()),
                arguments: response.as_object().unwrap().clone(),
                call_id: invocation.call_id,
            });
            calls.push(call);
        }
        assert_eq!(calls.len(), 3);
        // Typed responses turn into invocations too.
        let echo = MethodResponse::CoreEcho(Map::new())
            .into_invocation(crate::types::CallId::new("x"))
            .unwrap();
        assert_eq!(echo.name.as_str(), "Core/echo");
    }
}
