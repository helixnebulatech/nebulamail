// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! vCard from a DAV server: calcard never panics parsing it or converting
//! it to JSContact and back, also written back in the stored card's form
//! (version and Apple's `X-ADDRESSBOOKSERVER-*` groups).

#![no_main]

use libfuzzer_sys::fuzz_target;
use nebula_groupware::ConvertError;
use nebula_groupware::model::convert::{
    jscontact_to_vcard, jscontact_to_vcard_in, vcard_form, vcard_to_jscontact,
};

fn check<T>(result: Result<T, ConvertError>) -> Option<T> {
    match result {
        Err(ConvertError::Panicked { format }) => panic!("calcard panicked on {format}"),
        other => other.ok(),
    }
}

fuzz_target!(|data: &[u8]| {
    if let Some(json) = check(vcard_to_jscontact(data)) {
        let json = json.to_string();
        check(jscontact_to_vcard(&json));
        if let Some(form) = vcard_form(data) {
            check(jscontact_to_vcard_in(&json, form));
        }
    }
});
