// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { OAUTH_SCOPE, oauthScope, parseOAuthServices } from "./oauth-scope";

describe("OAuth services", () => {
  it("asks for all four when the operator doesn't choose", () => {
    expect(parseOAuthServices(undefined)).toStrictEqual([
      "mail",
      "contacts",
      "calendars",
      "files",
    ]);
    expect(parseOAuthServices("  ")).toStrictEqual(
      parseOAuthServices(undefined)
    );
    expect(OAUTH_SCOPE).toBe(
      "urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:contacts urn:ietf:params:oauth:scope:calendars urn:ietf:params:oauth:scope:files"
    );
  });

  it("keeps only the chosen ones, in a fixed order, without repeats", () => {
    expect(oauthScope(parseOAuthServices("calendars, mail mail"))).toBe(
      "urn:ietf:params:oauth:scope:mail urn:ietf:params:oauth:scope:calendars"
    );
  });

  it("refuses unknown names and a list without mail", () => {
    expect(() => parseOAuthServices("mail calendar")).toThrow(/calendar/u);
    expect(() => parseOAuthServices("contacts files")).toThrow(/mail/u);
  });
});
