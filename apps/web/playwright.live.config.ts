// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Browser tests against a real Stalwart: `bun run
// test:live`, skipped unless MAIL_LIVE_* are set (e2e-live/stalwart.e2e.ts).
// Needs Stalwart from docker/compose.yaml after `bun run dev:setup`, which
// registers the OAuth client for http://localhost:3001 and writes the
// app's apps/web/.env.local (read by the app itself). The login, the mail
// and the sending are real.

import { defineConfig, devices } from "@playwright/test";

/** The only redirect `dev:setup` registers for the app. */
const APP_URL = "http://localhost:3001";
/**
 * Where the app really listens, when something else holds 3001
 * (scripts/ci-local.sh): the browser still sees APP_URL, for the OAuth
 * redirect.
 */
const PORT = process.env.E2E_LIVE_PORT ?? "3001";

export default defineConfig({
  forbidOnly: process.env.CI === "true",
  projects: [{ name: "desktop", use: { ...devices["Desktop Chrome"] } }],
  reporter: process.env.CI === "true" ? "github" : "list",
  // No retries: each try sends real mail (Stalwart limits alice → bob).
  retries: 0,
  testDir: "e2e-live",
  testMatch: "*.e2e.ts",
  // The tests share the accounts: one at a time.
  workers: 1,
  use: {
    baseURL: APP_URL,
    launchOptions: {
      // Always sandboxed (the bundled Chromium lags behind security fixes).
      chromiumSandbox: true,
      args:
        PORT === "3001"
          ? []
          : [`--host-resolver-rules=MAP localhost:3001 127.0.0.1:${PORT}`],
    },
  },
  webServer: {
    command: `bun run build && bunx vite preview --host 127.0.0.1 --port ${PORT} --strictPort`,
    // `bun run dev` on 3001 works as well (same app, same Stalwart).
    reuseExistingServer: process.env.CI !== "true",
    timeout: 180_000,
    url: `http://127.0.0.1:${PORT}/login`,
  },
});
