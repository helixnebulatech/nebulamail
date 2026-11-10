// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A JMAP request body from a client must never panic the parser, the
//! reference resolver or the typed argument parsers; a parsed request
//! serializes back to one that parses the same.
//!
//! The calls are resolved in order as `run()` in nebula-jmap does, with
//! one `ReferenceBudget` for the whole request, and each one is answered
//! as `Core/echo` would (its resolved arguments, under its own name), so
//! later calls can copy earlier answers. What the copies add can't pass
//! the budget: a copied byte is at most 6 bytes of JSON (`\u0000`).

#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value};
use nebula_jmap_proto::{
    Capability, MethodCall, MethodLimits, ReferenceBudget, Request, RequestLimits,
    ResponseInvocation, ResponseName, resolve_result_references,
};

const REQUEST: RequestLimits = RequestLimits {
    max_size_request: 10_000_000,
    max_calls_in_request: 16,
};
const METHOD: MethodLimits = MethodLimits {
    max_objects_in_get: 500,
    max_objects_in_set: 500,
    max_size_request: 10_000_000,
};
/// Small, so inputs reach it.
const BUDGET: usize = 64 * 1024;
/// The most JSON text one byte of the budget can turn into.
const JSON_PER_BYTE: usize = 6;
const SUPPORTED: &[Capability] = &[
    Capability::Core,
    Capability::Mail,
    Capability::Submission,
    Capability::Quota,
    Capability::Mdn,
    Capability::Contacts,
    Capability::ContactsParse,
    Capability::Calendars,
    Capability::CalendarsParse,
];

fn json_len(arguments: &Map<String, Value>) -> usize {
    serde_json::to_vec(arguments)
        .expect("arguments serialize")
        .len()
}

fuzz_target!(|data: &[u8]| {
    let Ok(request) = Request::parse(data, &REQUEST, SUPPORTED) else {
        return;
    };
    let text = serde_json::to_vec(&request).expect("a request serializes");
    let again = Request::parse(&text, &REQUEST, SUPPORTED).expect("and parses again");
    assert_eq!(again, request);
    // One budget per request, as in `run()`.
    let mut budget = ReferenceBudget::new(BUDGET);
    let mut responses: Vec<ResponseInvocation> = Vec::new();
    let mut sent = 0_usize;
    let mut resolved = 0_usize;
    for invocation in request.method_calls.iter().cloned() {
        let _ = request.is_method_enabled(&invocation.name);
        sent += json_len(&invocation.arguments);
        let mut arguments = invocation.arguments;
        match resolve_result_references(&mut arguments, &responses, &mut budget) {
            Ok(()) => {
                resolved += json_len(&arguments);
                assert!(
                    resolved <= JSON_PER_BYTE * BUDGET + sent,
                    "references made {resolved} bytes of JSON from {sent}"
                );
                let _ = MethodCall::parse(&invocation.name, arguments.clone(), &METHOD);
                responses.push(ResponseInvocation {
                    name: ResponseName::Method(invocation.name),
                    arguments,
                    call_id: invocation.call_id,
                });
            }
            Err(error) => responses.push(ResponseInvocation::error(&error, invocation.call_id)),
        }
    }
});
