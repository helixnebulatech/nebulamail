// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Environment of the server the browser tests start. Fake hosts only:
// Playwright answers jmap.test itself and img.test is never contacted.

// E2E_PORT: another free port when several checkouts run tests at once.
export const TEST_PORT = Number(process.env.E2E_PORT ?? "3117");

export const TEST_ENV = {
  APP_URL: `http://localhost:${TEST_PORT}`,
  // No image proxy keys, whatever apps/web/.env.local has: with them the
  // server would sign tickets and ask jmap.test (unreachable from the
  // server) for the preload preference, and that request would never let
  // the page go idle. Tests that need the proxy answer its routes
  // themselves (remote-images.e2e.ts, settings.e2e.ts). Empty = unset
  // (server/env-setting.ts).
  IMAGE_PROXY_KEYS: "",
  IMAGE_PROXY_ORIGIN: "http://img.test",
  JMAP_SERVER: "http://jmap.test",
  OAUTH_CLIENT_ID: "e2e",
  OAUTH_CLIENT_SECRET: "fake-oauth-secret-for-browser-tests",
  // A fixed test key ("e2e-test-key-not-a-secret-123456"); it only seals
  // the fake session cookie of these tests.
  SESSION_KEYS: "1:ZTJlLXRlc3Qta2V5LW5vdC1hLXNlY3JldC0xMjM0NTY",
} as const;
