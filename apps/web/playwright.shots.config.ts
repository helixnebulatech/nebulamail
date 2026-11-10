// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Screenshots for the README (`bun run screenshots`): the production build
// against the fake JMAP server of the e2e tests, with a demo mailbox. No
// real account is touched. Writes docs/images/*.png.

import { defineConfig } from "@playwright/test";

import base from "./playwright.config";

export default defineConfig({
  ...base,
  projects: [{ name: "shots" }],
  retries: 0,
  testDir: "e2e",
  testMatch: "*.shots.ts",
});
