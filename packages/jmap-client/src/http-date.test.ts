// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { parseHttpDate } from "./http-date";

const NOW = Date.parse("2026-11-06T12:00:00Z");
const EXAMPLE = Date.parse("1994-11-06T08:49:37Z");

describe(parseHttpDate, () => {
  it.each([
    // RFC 9110 §5.6.7's own examples, all three formats.
    ["Sun, 06 Nov 1994 08:49:37 GMT", EXAMPLE],
    ["Sunday, 06-Nov-94 08:49:37 GMT", EXAMPLE],
    ["Sun Nov  6 08:49:37 1994", EXAMPLE],
    ["Sun Nov 16 08:49:37 1994", Date.parse("1994-11-16T08:49:37Z")],
    ["  Sat, 31 Oct 2026 10:00:03 GMT ", Date.parse("2026-10-31T10:00:03Z")],
    // A leap second.
    ["Wed, 31 Dec 2025 23:59:60 GMT", Date.parse("2026-01-01T00:00:00Z")],
  ])("reads %j", (value, expected) => {
    expect(parseHttpDate(value, NOW)).toBe(expected);
  });

  it("reads a two-digit year as at most 50 years ahead", () => {
    expect(parseHttpDate("Thursday, 01-Jan-76 00:00:00 GMT", NOW)).toBe(
      Date.parse("2076-01-01T00:00:00Z")
    );
    expect(parseHttpDate("Monday, 01-Jan-77 00:00:00 GMT", NOW)).toBe(
      Date.parse("1977-01-01T00:00:00Z")
    );
  });

  it.each([
    "0",
    "-1",
    "120",
    "",
    "2026-11-06T12:00:00Z",
    "Sun, 6 Nov 1994 08:49:37 GMT",
    "Sun, 06 Nov 1994 08:49:37 UTC",
    "sun, 06 nov 1994 08:49:37 GMT",
    "Sun, 31 Feb 1994 08:49:37 GMT",
    "Sun, 06 Nov 1994 24:00:00 GMT",
    "Sun Nov 6 08:49:37 1994",
    "Sun, 06-Nov-94 08:49:37 GMT",
  ])("refuses %j", (value) => {
    expect(parseHttpDate(value, NOW)).toBeNull();
  });
});
