// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Result references: `"#ids": {"resultOf", "name", "path"}` (RFC 8620
//! §3.7).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::JsonPointer;
use super::budget::ReferenceBudget;
use super::jsptr::evaluate_bounded;
use crate::error::MethodError;
use crate::request::MethodName;
use crate::response::{ResponseInvocation, ResponseName};
use crate::types::CallId;

/// A reference to (part of) the result of an earlier method call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResultReference {
    /// The call id of the earlier call.
    pub result_of: CallId,
    /// The expected response name (fails if the call returned an error).
    pub name: MethodName,
    /// Where in the response arguments to take the value from.
    pub path: JsonPointer,
}

impl ResultReference {
    /// Evaluates the reference against the responses so far, paying the
    /// copy from `budget`.
    ///
    /// # Errors
    /// `invalidResultReference` if no earlier response has that call id, the
    /// first one has a different name, or the path does not match;
    /// `requestTooLarge` if the copy doesn't fit in `budget`.
    pub fn evaluate(
        &self,
        responses: &[ResponseInvocation],
        budget: &mut ReferenceBudget,
    ) -> Result<Value, MethodError> {
        let response = responses
            .iter()
            .find(|response| response.call_id == self.result_of)
            .ok_or_else(|| {
                MethodError::invalid_result_reference(format!(
                    "no response with call id {:?}",
                    self.result_of.as_str()
                ))
            })?;
        if response.name != ResponseName::Method(self.name.clone()) {
            return Err(MethodError::invalid_result_reference(format!(
                "response {:?} is {:?}, not {:?}",
                self.result_of.as_str(),
                response.name.as_str(),
                self.name.as_str()
            )));
        }
        // The pointer applies to the arguments object, read in place: only
        // what it points at is copied, once paid for.
        let not_found = || {
            MethodError::invalid_result_reference(format!(
                "path {:?} not found",
                self.path.to_string()
            ))
        };
        let over = |_| {
            MethodError::request_too_large(
                "the result references of this request copy more than maxSizeRequest",
            )
        };
        let Some((first, rest)) = self.path.tokens().split_first() else {
            budget.charge_map(&response.arguments).map_err(over)?;
            return Ok(Value::Object(response.arguments.clone()));
        };
        let start = response
            .arguments
            .get(first.as_str())
            .ok_or_else(not_found)?;
        evaluate_bounded(start, rest, budget)
            .map_err(over)?
            .ok_or_else(not_found)
    }
}

/// Replaces every `"#name": ResultReference` argument with `"name": value`.
/// `budget` is shared by all the calls of a request.
///
/// # Errors
/// `invalidResultReference` if a reference can't be parsed or evaluated;
/// `invalidArguments` if both `name` and `#name` are given (RFC 8620 §3.7);
/// `requestTooLarge` if the copies pass `budget`.
pub fn resolve_result_references(
    arguments: &mut Map<String, Value>,
    responses: &[ResponseInvocation],
    budget: &mut ReferenceBudget,
) -> Result<(), MethodError> {
    let keys: Vec<String> = arguments
        .keys()
        .filter(|key| key.starts_with('#'))
        .cloned()
        .collect();
    for key in keys {
        let Some(name) = key.strip_prefix('#') else {
            continue;
        };
        if arguments.contains_key(name) {
            return Err(MethodError::invalid_arguments(format!(
                "both {name:?} and {key:?} are given"
            )));
        }
        let Some(raw) = arguments.remove(&key) else {
            continue;
        };
        let reference: ResultReference = serde_json::from_value(raw).map_err(|error| {
            MethodError::invalid_result_reference(format!(
                "{key:?} is not a ResultReference: {error}"
            ))
        })?;
        let value = reference.evaluate(responses, budget)?;
        arguments.insert(name.to_owned(), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::error::MethodErrorType::{
        InvalidArguments, InvalidResultReference, RequestTooLarge,
    };

    fn invocation(value: &Value) -> ResponseInvocation {
        serde_json::from_value(value.clone()).unwrap()
    }

    fn arguments(value: &Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn unlimited() -> ReferenceBudget {
        ReferenceBudget::new(usize::MAX)
    }

    /// `Core/echo` answers its arguments: the response to `call`.
    fn echo(arguments: Map<String, Value>, call: &str) -> ResponseInvocation {
        ResponseInvocation {
            name: ResponseName::Method("Core/echo".parse().unwrap()),
            arguments,
            call_id: CallId::new(call),
        }
    }

    /// 100 references to the whole response of `call`.
    fn hundred_copies_of(call: &str) -> Map<String, Value> {
        (0..100)
            .map(|n| {
                (
                    format!("#k{n}"),
                    json!({"resultOf": call, "name": "Core/echo", "path": ""}),
                )
            })
            .collect()
    }

    /// The amplification of the deepsec finding: each call copies the
    /// previous response 100 times, so c4 would hold 100⁴ copies of c0.
    #[test]
    fn chained_echo_references_hit_the_budget() {
        let mut budget = ReferenceBudget::new(1_000_000);
        let mut responses = vec![echo(arguments(&json!({"x": "a".repeat(1000)})), "c0")];
        let mut refused = None;
        for n in 1..5 {
            let mut args = hundred_copies_of(&format!("c{}", n - 1));
            match resolve_result_references(&mut args, &responses, &mut budget) {
                Ok(()) => responses.push(echo(args, &format!("c{n}"))),
                Err(error) => {
                    assert_eq!(error.kind(), RequestTooLarge);
                    refused = Some(n);
                    break;
                }
            }
        }
        assert!(matches!(refused, Some(1..=3)), "{refused:?}");
        // What was copied stayed within the budget.
        let copied: usize = responses
            .iter()
            .map(|r| serde_json::to_vec(&r.arguments).unwrap().len())
            .sum();
        assert!(copied < 1_000_000, "{copied}");
    }

    #[test]
    fn budget_counts_nodes() {
        // Small as JSON (3 bytes per item), big in memory.
        let empties = Value::Array(vec![json!([]); 10_000]);
        let responses = [echo(arguments(&json!({"list": empties})), "c0")];
        let mut budget = ReferenceBudget::new(1_000_000);
        let mut args = arguments(&json!({
            "#a": {"resultOf": "c0", "name": "Core/echo", "path": ""},
            "#b": {"resultOf": "c0", "name": "Core/echo", "path": ""},
            "#c": {"resultOf": "c0", "name": "Core/echo", "path": ""},
            "#d": {"resultOf": "c0", "name": "Core/echo", "path": ""}
        }));
        let error = resolve_result_references(&mut args, &responses, &mut budget).unwrap_err();
        assert_eq!(error.kind(), RequestTooLarge);
    }

    #[test]
    fn references_within_budget_still_work() {
        let responses = [invocation(&json!(["Email/get", {
            "list": [{"id": "m1", "threadId": "t1"}, {"id": "m2", "threadId": null}]
        }, "g"]))];
        let mut budget = ReferenceBudget::new(10_000);
        let mut args = arguments(&json!({
            "#ids": {"resultOf": "g", "name": "Email/get", "path": "/list/*/threadId"}
        }));
        resolve_result_references(&mut args, &responses, &mut budget).unwrap();
        assert_eq!(args["ids"], json!(["t1"]));
        assert!(budget.remaining() < 10_000);
        // `*` charges inside: a tiny budget refuses the same reference.
        let mut tiny = ReferenceBudget::new(40);
        let mut args = arguments(&json!({
            "#ids": {"resultOf": "g", "name": "Email/get", "path": "/list/*/threadId"}
        }));
        let error = resolve_result_references(&mut args, &responses, &mut tiny).unwrap_err();
        assert_eq!(error.kind(), RequestTooLarge);
    }

    /// RFC 8620 §3.7, first example.
    #[test]
    fn rfc_simple_example() {
        let responses = [invocation(&json!(["Foo/changes", {
            "accountId": "A1", "oldState": "abcdef", "newState": "123456",
            "hasMoreChanges": false, "created": ["f1", "f4"], "updated": [], "destroyed": []
        }, "t0"]))];
        let mut args = arguments(&json!({
            "accountId": "A1",
            "#ids": {"resultOf": "t0", "name": "Foo/changes", "path": "/created"}
        }));
        resolve_result_references(&mut args, &responses, &mut unlimited()).unwrap();
        assert_eq!(
            Value::Object(args),
            json!({"accountId": "A1", "ids": ["f1", "f4"]})
        );
    }

    /// RFC 8620 §3.7, second example (Email/query → Email/get → Thread/get
    /// → Email/get).
    #[test]
    fn rfc_chained_example() {
        let responses = [
            invocation(&json!(["Email/query", {
                "accountId": "A1", "queryState": "abcdefg", "canCalculateChanges": true,
                "position": 0, "total": 101,
                "ids": ["msg1023", "msg223", "msg110", "msg93", "msg91",
                        "msg38", "msg36", "msg33", "msg11", "msg1"]
            }, "t0"])),
            invocation(&json!(["Email/get", {
                "accountId": "A1", "state": "123456",
                "list": [
                    {"id": "msg1023", "threadId": "trd194"},
                    {"id": "msg223", "threadId": "trd114"},
                    {"id": "msg110", "threadId": "trd99"},
                    {"id": "msg93", "threadId": "trd99"},
                    {"id": "msg91", "threadId": "trd99"},
                    {"id": "msg38", "threadId": "trd99"},
                    {"id": "msg36", "threadId": "trd99"},
                    {"id": "msg33", "threadId": "trd99"},
                    {"id": "msg11", "threadId": "trd99"},
                    {"id": "msg1", "threadId": "trd99"}
                ],
                "notFound": []
            }, "t1"])),
            invocation(&json!(["Thread/get", {
                "accountId": "A1", "state": "123456",
                "list": [
                    {"id": "trd194", "emailIds": ["msg1020", "msg1021", "msg1023"]},
                    {"id": "trd114", "emailIds": ["msg201", "msg223"]},
                    {"id": "trd99", "emailIds": ["msg110"]}
                ],
                "notFound": []
            }, "t2"])),
        ];
        let mut thread_get = arguments(&json!({
            "accountId": "A1",
            "#ids": {"resultOf": "t1", "name": "Email/get", "path": "/list/*/threadId"}
        }));
        resolve_result_references(&mut thread_get, &responses, &mut unlimited()).unwrap();
        assert_eq!(
            thread_get["ids"],
            json!([
                "trd194", "trd114", "trd99", "trd99", "trd99", "trd99", "trd99", "trd99", "trd99",
                "trd99"
            ])
        );
        let mut email_get = arguments(&json!({
            "accountId": "A1",
            "#ids": {"resultOf": "t2", "name": "Thread/get", "path": "/list/*/emailIds"},
            "properties": ["from", "receivedAt", "subject"]
        }));
        resolve_result_references(&mut email_get, &responses, &mut unlimited()).unwrap();
        assert_eq!(
            email_get["ids"],
            json!([
                "msg1020", "msg1021", "msg1023", "msg201", "msg223", "msg110"
            ])
        );
    }

    #[test]
    fn failures() {
        let responses = [
            invocation(&json!(["Email/query", {"ids": ["a"]}, "q"])),
            invocation(&json!(["error", {"type": "serverFail"}, "e"])),
            invocation(&json!(["Email/get", {"list": []}, "q"])),
        ];
        let resolve = |args: Value| {
            let mut args = arguments(&args);
            resolve_result_references(&mut args, &responses, &mut unlimited()).map(|()| args)
        };
        let kind = |args: Value| resolve(args).unwrap_err().kind();
        // Unknown call id, name mismatch, error response, bad path.
        for reference in [
            json!({"resultOf": "x", "name": "Email/query", "path": "/ids"}),
            json!({"resultOf": "q", "name": "Email/get", "path": "/ids"}),
            json!({"resultOf": "e", "name": "Email/query", "path": "/ids"}),
            json!({"resultOf": "q", "name": "Email/query", "path": "/nope"}),
            json!({"resultOf": "q", "name": "Email/query", "path": "ids"}),
            json!({"resultOf": "q", "name": "Email/query"}),
            json!("not an object"),
        ] {
            assert_eq!(kind(json!({"#ids": reference})), InvalidResultReference);
        }
        // The first response with the call id is used, even if a later one
        // would match.
        assert_eq!(
            kind(json!({"#ids": {"resultOf": "q", "name": "Email/get", "path": "/list"}})),
            InvalidResultReference
        );
        assert_eq!(
            kind(json!({
                "ids": [],
                "#ids": {"resultOf": "q", "name": "Email/query", "path": "/ids"}
            })),
            InvalidArguments
        );
        let ok = resolve(json!({"#ids": {"resultOf": "q", "name": "Email/query", "path": "/ids"}}));
        assert_eq!(ok.unwrap()["ids"], json!(["a"]));
    }
}
