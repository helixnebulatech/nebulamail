// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A signed-in browser without a login: the session is a sealed cookie
// (server/auth/seal.ts), so the test seals one with the test key. Its
// access token only ever goes to the fake JMAP server.

import type { BrowserContext } from "@playwright/test";

import { SESSION_COOKIE, cookieName } from "../src/server/auth/cookies";
import { parseSealKeys, seal } from "../src/server/auth/seal";
import { TEST_ENV } from "./env";

export const signIn = async (context: BrowserContext): Promise<void> => {
  const now = Date.now();
  const value = await seal(
    await parseSealKeys(TEST_ENV.SESSION_KEYS),
    "session",
    {
      accessToken: "e2e-access-token",
      accessTokenExpiresAt: now + 3_600_000,
      createdAt: now,
      lastSeenAt: now,
      refreshToken: "e2e-refresh-token",
      v: 1,
    }
  );
  await context.addCookies([
    {
      httpOnly: true,
      name: cookieName(SESSION_COOKIE, new URL(TEST_ENV.APP_URL)),
      sameSite: "Lax",
      secure: true,
      // Chromium's CDP refuses a Secure cookie for an http: URL, but sends
      // one set for the same host to http://localhost (a secure context).
      url: TEST_ENV.APP_URL.replace(/^http:/u, "https:"),
      value,
    },
  ]);
};
