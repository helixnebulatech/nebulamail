// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Browser tests against the real bridge: `bun run
// test:bridge`. Needs Dovecot, Stalwart and Radicale (calendars and
// contacts) from docker/compose.yaml (profile "bridge", see
// docker/README.md) and the bridge binary built (`cargo build -p
// nebula-bridge` in bridge/); the tests tagged `@postfix` also need the
// test Postfix (`E2E_BRIDGE_SMTP=postfix`, see e2e-bridge/env.ts). Unlike
// `test:browser`, the login, the mail, the sending and the DAV server are
// real.

import { defineConfig, devices } from "@playwright/test";

import { BRIDGE_URL, WEB_ENV, WEB_PORT } from "./e2e-bridge/env";

// The worker's requests as context events (web-push.e2e.ts counts them:
// mail inside the push is shown with none).
process.env.PW_EXPERIMENTAL_SERVICE_WORKER_NETWORK_EVENTS ??= "1";

export default defineConfig({
  forbidOnly: process.env.CI === "true",
  projects: [{ name: "desktop", use: { ...devices["Desktop Chrome"] } }],
  reporter: process.env.CI === "true" ? "github" : "list",
  retries: process.env.CI === "true" ? 1 : 0,
  testDir: "e2e-bridge",
  // With the [ALERT] proxy in front of Dovecot (e2e-bridge/env.ts), only
  // the file that starts it.
  testMatch:
    process.env.E2E_BRIDGE_ALERT_PROXY === "1"
      ? "server-alerts.e2e.ts"
      : "*.e2e.ts",
  // The tests share one mailbox: one at a time.
  workers: 1,
  use: {
    baseURL: WEB_ENV.APP_URL,
    // Always sandboxed (the bundled Chromium lags behind security fixes).
    launchOptions: { chromiumSandbox: true },
  },
  webServer: [
    {
      command: "bun e2e-bridge/start-bridge.ts",
      // The bridge logs to stdout: show it, to see why a request failed.
      stdout: "pipe",
      reuseExistingServer: false,
      timeout: 30_000,
      url: `${BRIDGE_URL}/.well-known/oauth-authorization-server`,
    },
    {
      command: `bun run build && bunx vite preview --port ${WEB_PORT} --strictPort`,
      env: WEB_ENV,
      reuseExistingServer: false,
      timeout: 180_000,
      url: `${WEB_ENV.APP_URL}/login`,
    },
  ],
});
