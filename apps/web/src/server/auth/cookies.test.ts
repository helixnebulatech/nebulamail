// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { SESSION_COOKIE, cookieName } from "./cookies";

describe(cookieName, () => {
  it("keeps the plain name on a real host", () => {
    expect(
      cookieName(SESSION_COOKIE, new URL("https://mail.example.com"))
    ).toBe("__Host-wm-session");
    expect(
      cookieName(SESSION_COOKIE, new URL("https://mail.example.com:8443"))
    ).toBe("__Host-wm-session");
  });

  it("adds the port on localhost, so two dev copies don't clash", () => {
    expect(cookieName(SESSION_COOKIE, new URL("http://localhost:3001"))).toBe(
      "__Host-wm-session-3001"
    );
    expect(cookieName(SESSION_COOKIE, new URL("http://127.0.0.1:3012"))).toBe(
      "__Host-wm-session-3012"
    );
  });
});
