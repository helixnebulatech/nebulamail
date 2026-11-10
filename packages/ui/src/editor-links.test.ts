// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable no-script-url -- the tests check that javascript: links are refused

import { describe, expect, it } from "vitest";

import { isAllowedLink, normalizeLink } from "./editor-links";

describe(normalizeLink, () => {
  it.each([
    ["example.com", "https://example.com/"],
    [" https://example.com/a?b=1 ", "https://example.com/a?b=1"],
    ["http://example.com", "http://example.com/"],
    ["ana@example.com", "mailto:ana@example.com"],
    ["mailto:ana@example.com", "mailto:ana@example.com"],
    ["example.com:8080/x", "https://example.com:8080/x"],
    ["localhost:3000", "https://localhost:3000/"],
    ["javascript:alert(1)", null],
    ["data:text/html,hi", null],
    ["file:///etc/passwd", null],
    ["foo", null],
    ["two words.com", null],
    ["", null],
  ])("%j → %j", (input, expected) => {
    expect(normalizeLink(input)).toBe(expected);
  });
});

describe(isAllowedLink, () => {
  it.each([
    ["https://example.com", true],
    ["mailto:a@b.com", true],
    ["javascript:alert(1)", false],
    ["JaVaScRiPt:alert(1)", false],
    ["/relative", false],
    ["vbscript:x", false],
  ])("%j → %s", (href, expected) => {
    expect(isAllowedLink(href)).toBe(expected);
  });
});
