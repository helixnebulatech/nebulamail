// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Calendar and contact objects from a client never panic the parsers:
//! the method arguments of every Calendar, CalendarEvent,
//! ParticipantIdentity, AddressBook, ContactCard and Principal method, the objects to
//! create and the patches. A patch that parses applies to a body without
//! panicking, and the result is still a valid body.

#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::{Map, Value};
use nebula_jmap_proto::method::PatchObject;
use nebula_jmap_proto::object::js_body::{JsBody, JsCalendar, JsContact};
use nebula_jmap_proto::object::{
    AddressBookCreate, AddressBookPatch, CalendarCreate, CalendarEventCreate, CalendarEventPatch,
    CalendarPatch, ContactCardCreate, ContactCardPatch,
};
use nebula_jmap_proto::request::from_slice_strict;
use nebula_jmap_proto::{MethodCall, MethodLimits, MethodName};

const METHOD: MethodLimits = MethodLimits {
    max_objects_in_get: 500,
    max_objects_in_set: 500,
    max_size_request: 10_000_000,
};

const METHODS: &[&str] = &[
    "Calendar/get",
    "Calendar/changes",
    "Calendar/set",
    "CalendarEvent/get",
    "CalendarEvent/changes",
    "CalendarEvent/query",
    "CalendarEvent/set",
    "CalendarEvent/parse",
    "ParticipantIdentity/get",
    "AddressBook/get",
    "AddressBook/changes",
    "AddressBook/set",
    "ContactCard/get",
    "ContactCard/changes",
    "ContactCard/query",
    "ContactCard/set",
    "ContactCard/parse",
    "Principal/get",
    "Principal/query",
    "Principal/getAvailability",
];

fuzz_target!(|data: &[u8]| {
    // The first byte picks the method; the rest is the JSON.
    let Some((selector, json)) = data.split_first() else {
        return;
    };
    let Ok(Value::Object(object)) = from_slice_strict(json) else {
        return;
    };
    let index = usize::from(*selector) % METHODS.len();
    if let Some(name) = METHODS.get(index) {
        let name: MethodName = name.parse().expect("known name");
        let _ = MethodCall::parse(&name, object.clone(), &METHOD);
    }
    let _ = CalendarCreate::parse(object.clone());
    let _ = AddressBookCreate::parse(object.clone());
    let _ = CalendarEventCreate::parse(object.clone());
    let _ = ContactCardCreate::parse(object.clone());
    let patch = PatchObject::new(object.clone());
    let _ = CalendarPatch::parse(&patch);
    let _ = AddressBookPatch::parse(&patch);
    if let Ok(event) = CalendarEventPatch::parse(&patch) {
        let mut body = JsBody::<JsCalendar>::new(Map::new()).expect("empty body");
        if body.apply(&event.body).is_ok() {
            JsBody::<JsCalendar>::new(body.into_map()).expect("still valid");
        }
    }
    if let Ok(card) = ContactCardPatch::parse(&patch) {
        let mut body = JsBody::<JsContact>::new(Map::new()).expect("empty body");
        if body.apply(&card.body).is_ok() {
            JsBody::<JsContact>::new(body.into_map()).expect("still valid");
        }
    }
});
