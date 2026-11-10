// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The static zip served by Stalwart alone (docs/static.md):
// `bun run test:static`. Needs `bun run build:static` and the throwaway
// Stalwart of docker/compose.yaml (`stalwart-static`, port 8095, own
// compose project) started with STALWART_STATIC_RECOVERY_ADMIN; the global
// setup (e2e-static/setup.ts) configures it as the guide says and starts
// the recording proxy (e2e-static/proxy.ts). No server of ours runs: the
// browser only reaches Stalwart's origin.

import { defineConfig, devices } from "@playwright/test";

import { STALWART_URL } from "./e2e-static/env";
import { PROXY_URL } from "./e2e-static/proxy";

export default defineConfig({
  forbidOnly: process.env.CI === "true",
  globalSetup: "./e2e-static/setup.ts",
  projects: [{ name: "desktop", use: { ...devices["Desktop Chrome"] } }],
  reporter: process.env.CI === "true" ? "github" : "list",
  retries: process.env.CI === "true" ? 1 : 0,
  testDir: "e2e-static",
  testMatch: "*.e2e.ts",
  // One account and one login: one at a time.
  workers: 1,
  use: {
    baseURL: STALWART_URL,
    // Every request of the browser, localhost included (Playwright sends
    // loopback through the proxy too), goes through e2e-static/proxy.ts,
    // which forwards only to Stalwart and lists what it saw.
    // Always sandboxed (the bundled Chromium lags behind security fixes).
    launchOptions: { chromiumSandbox: true, proxy: { server: PROXY_URL } },
  },
});
