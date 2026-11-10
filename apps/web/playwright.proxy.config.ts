// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Browser tests of the full BFF mode against a real Stalwart
// `bun run test:proxy`, skipped unless MAIL_LIVE_*
// are set (e2e-proxy/proxy.e2e.ts). Like test:live it needs Stalwart from
// docker/compose.yaml after `bun run dev:setup` (OAuth client for
// http://localhost:3001 and apps/web/.env.local), and the app runs with
// JMAP_PROXY=on and JMAP_INTERNAL_URL on another name of Stalwart, so the
// rewriting of its URLs is tested too.

import { defineConfig, devices } from "@playwright/test";

/** The only redirect `dev:setup` registers for the app. */
const APP_URL = "http://localhost:3001";
/** Where the app really listens (see playwright.live.config.ts). */
const PORT = process.env.E2E_LIVE_PORT ?? "3001";
/** Stalwart under another name than JMAP_SERVER (localhost:8080). */
const LOOPBACK = ["127", "0", "0", "1"].join(".");
const INTERNAL_URL =
  process.env.E2E_JMAP_INTERNAL_URL ?? `http://${LOOPBACK}:8080`;

// The service worker's requests show up in the context (the test checks
// that none of them carries a token either).
process.env.PW_EXPERIMENTAL_SERVICE_WORKER_NETWORK_EVENTS ??= "1";

export default defineConfig({
  forbidOnly: process.env.CI === "true",
  projects: [{ name: "desktop", use: { ...devices["Desktop Chrome"] } }],
  reporter: process.env.CI === "true" ? "github" : "list",
  // No retries: each try sends real mail.
  retries: 0,
  testDir: "e2e-proxy",
  testMatch: "*.e2e.ts",
  workers: 1,
  use: {
    baseURL: APP_URL,
    launchOptions: {
      // Always sandboxed (the bundled Chromium lags behind security fixes).
      chromiumSandbox: true,
      args:
        PORT === "3001"
          ? []
          : [`--host-resolver-rules=MAP localhost:3001 ${LOOPBACK}:${PORT}`],
    },
  },
  webServer: {
    command: `bun run build && bunx vite preview --host ${LOOPBACK} --port ${PORT} --strictPort`,
    env: { JMAP_INTERNAL_URL: INTERNAL_URL, JMAP_PROXY: "on" },
    // Never a server started without the proxy (test:live's, `bun run dev`).
    reuseExistingServer: false,
    timeout: 180_000,
    url: `http://${LOOPBACK}:${PORT}/login`,
  },
});
