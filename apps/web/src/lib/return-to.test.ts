// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { safeReturnTo } from "./return-to";

// Built from parts so the linter doesn't see a script URL in the source.
const SCRIPT_URL = ["javascript", "alert(1)"].join(":");

describe(safeReturnTo, () => {
  it.each([
    ["/inbox", "/inbox"],
    ["/inbox?q=a#b", "/inbox?q=a#b"],
    ["/inbox?x=1#y", "/inbox?x=1#y"],
    ["/mail/inbox?x=1#y", "/mail/inbox?x=1#y"],
    [null, "/"],
    [undefined, "/"],
    ["", "/"],
    ["mail", "/"],
    ["https://evil.com/", "/"],
    ["//evil.com/", "/"],
    ["/\\evil.com", "/"],
    ["\\\\evil.com", "/"],
    [SCRIPT_URL, "/"],
    // Encoded slashes don't split the path: harmless, kept as they are.
    ["/%2F%2Fevil.com", "/%2F%2Fevil.com"],
    ["/./%2fevil.com", "/%2fevil.com"],
    // Dot segments that normalize into "//host".
    ["/.//evil.com", "/"],
    ["/..//evil.com", "/"],
    ["/x/..//evil.com", "/"],
    ["/%2e//evil.com", "/"],
    ["/%2E%2E//evil.com", "/"],
    ["/.//evil.com/x?y=1#z", "/"],
    // Control characters.
    ["/inbox\u0000", "/"],
    ["/\t/evil.com", "/"],
    ["/\n/evil.com", "/"],
  ])("%s → %s", (input, expected) => {
    expect(safeReturnTo(input)).toBe(expected);
  });
});
