// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Browser tests of the mail UI at phone and tablet widths, and of the PWA
//`bun run test:browser`. They run the production
// build with a fake session and a fake JMAP server (e2e/fake-jmap.ts), so
// no login and no mail server are needed.

import { defineConfig, devices } from "@playwright/test";

import { TEST_ENV, TEST_PORT } from "./e2e/env";

// The service worker's requests show up in the context and go through
// `context.route` (e2e/web-push.e2e.ts: the worker talks to the fake JMAP).
process.env.PW_EXPERIMENTAL_SERVICE_WORKER_NETWORK_EVENTS ??= "1";

const touch = { hasTouch: true, isMobile: true };

/**
 * Printing differs most between engines, so
 * its tests also run in Firefox and WebKit; the rest of the UI only in
 * Chromium. WebKit needs system libraries some Linux distributions don't
 * have (Gentoo, Arch): it runs on CI (`playwright install --with-deps`)
 * or with E2E_WEBKIT=1.
 */
const PRINT_TESTS = "print.e2e.ts";
const webkit = process.env.CI === "true" || process.env.E2E_WEBKIT === "1";

export default defineConfig({
  forbidOnly: process.env.CI === "true",
  fullyParallel: true,
  projects: [
    {
      name: "phone-360",
      use: {
        ...devices["Galaxy S9+"],
        ...touch,
        viewport: { height: 740, width: 360 },
      },
    },
    { name: "phone-412", use: { ...devices["Pixel 7"], ...touch } },
    { name: "tablet", use: { ...devices["Galaxy Tab S4"], ...touch } },
    { name: "desktop", use: { ...devices["Desktop Chrome"] } },
    {
      name: "desktop-firefox",
      testMatch: PRINT_TESTS,
      use: { ...devices["Desktop Firefox"] },
    },
    ...(webkit
      ? [
          {
            name: "desktop-webkit",
            testMatch: PRINT_TESTS,
            use: { ...devices["Desktop Safari"] },
          },
        ]
      : []),
  ],
  reporter: process.env.CI === "true" ? "github" : "list",
  // One retry on CI: a test that only passes on retry is reported as
  // "flaky" (not hidden), and a slow runner doesn't fail the build.
  retries: process.env.CI === "true" ? 1 : 0,
  testDir: "e2e",
  testMatch: "*.e2e.ts",
  use: {
    baseURL: TEST_ENV.APP_URL,
    // Always sandboxed (the bundled Chromium lags behind security fixes).
    launchOptions: { chromiumSandbox: true },
  },
  webServer: {
    command: `bun run build && bunx vite preview --port ${TEST_PORT} --strictPort`,
    env: TEST_ENV,
    reuseExistingServer: process.env.CI !== "true",
    timeout: 180_000,
    url: `${TEST_ENV.APP_URL}/login`,
  },
});
