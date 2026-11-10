// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hrefs and `Location` values: never panic, and an accepted one is
//! canonical (resolving its own path gives it back), ASCII, with no dot,
//! empty or slash-hiding segment. Nextcloud's `principal:` names too:
//! an accepted one writes back the same, and its location stays below
//! the DAV root and names it again.

#![no_main]

use libfuzzer_sys::fuzz_target;
use url::Url;
use nebula_dav_proto::{Href, OcPrincipal};

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(url) = Url::parse("https://dav.example.com/dav/cal/alice/work/1.ics") else {
        return;
    };
    let Ok(base) = Href::from_url(&url) else {
        return;
    };
    if let Some(principal) = OcPrincipal::parse(text) {
        let written = principal.to_string();
        assert_eq!(OcPrincipal::parse(&written).as_ref(), Some(&principal));
        if let Some(at) = principal.href(&base) {
            assert!(at.is_inside(&base) && at.same_origin(&base));
            assert_eq!(OcPrincipal::of(&at, &base).as_ref(), Some(&principal));
            assert!(OcPrincipal::dav_root(&at).is_some());
        }
    }
    if let Ok(href) = Href::resolve(text, &base) {
        let path = href.path();
        assert!(path.is_ascii() && path.starts_with('/') && !path.contains("//"));
        for segment in path.split('/') {
            let lower = segment.to_ascii_lowercase();
            // `.` is never encoded; `/` and backslash never inside a segment.
            assert!(lower != "." && lower != "..");
            assert!(!lower.contains("%2e") && !lower.contains("%2f") && !lower.contains("%5c"));
        }
        assert_eq!(Href::resolve(path, &href).as_ref(), Ok(&href));
        assert_eq!(Href::resolve(href.url().as_str(), &base).as_ref(), Ok(&href));
        if href.is_member_of(&base) || href.is_inside(&base) {
            assert!(href.same_origin(&base));
        }
    }
});
