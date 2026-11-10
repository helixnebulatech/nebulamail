// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Real-browser hostile-email tests: `bun run test:browser`.

import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  forbidOnly: process.env.CI === "true",
  fullyParallel: true,
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
  ],
  reporter: process.env.CI === "true" ? "github" : "list",
  testDir: "e2e",
  testMatch: "*.e2e.ts",
  // Always sandboxed: these tests open hostile emails on purpose.
  use: { launchOptions: { chromiumSandbox: true } },
});
