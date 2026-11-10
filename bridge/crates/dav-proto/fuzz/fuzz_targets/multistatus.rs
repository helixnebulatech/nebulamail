// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A multistatus or error body from a hostile server never panics the
//! parser, and every href it accepts stays canonical.

#![no_main]

use libfuzzer_sys::fuzz_target;
use url::Url;
use nebula_dav_proto::{Href, OcPrincipal, XmlLimits, parse_error_body, parse_multistatus};

fuzz_target!(|data: &[u8]| {
    let Ok(base) = Url::parse("https://dav.example.com/dav/cal/alice/") else {
        return;
    };
    let limits = XmlLimits {
        max_bytes: 1024 * 1024,
        max_nodes: 100_000,
        ..XmlLimits::default()
    };
    if let Ok(multistatus) = parse_multistatus(data, &base, &limits) {
        for response in &multistatus.responses {
            for href in response.hrefs() {
                let again = Href::resolve(href.path(), href).ok();
                assert_eq!(again.as_ref(), Some(href));
                assert!(!href.path().split('/').any(|s| s == "." || s == ".."));
            }
            for share in response
                .properties
                .oc_invite
                .iter()
                .flat_map(|invite| &invite.users)
            {
                let again = OcPrincipal::parse(&share.principal.to_string());
                assert_eq!(again.as_ref(), Some(&share.principal));
            }
        }
    }
    let _ = parse_error_body(data);
});
