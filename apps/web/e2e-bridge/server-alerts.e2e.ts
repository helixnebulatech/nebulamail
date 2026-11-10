// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The mail server's `[ALERT]`s (RFC 3501 §7.1, RFC 9051 §7.1: they MUST be
// shown to the user) through the
// real bridge and Dovecot, with alert-proxy.ts adding them (Dovecot never
// sends one). Only with the proxy in front of Dovecot:
//
//   E2E_BRIDGE_ALERT_PROXY=1 bun run test:bridge
//
// (then only this file runs, see playwright.bridge.config.ts).

import { expect, test } from "@playwright/test";

import type { AlertProxy } from "./alert-proxy";
import { startAlertProxy } from "./alert-proxy";
import {
  ALERT_PROXY,
  ALERT_PROXY_PORT,
  DOVECOT,
  DOVECOT_IMAP_PORT,
} from "./env";
import { signIn } from "./session";

/** Server text that looks like HTML: it must show as plain text. */
const QUOTA = "Your mailbox is 95% full <b>delete</b> old mail";
const LOCKED = "Account locked by the administrator";

test.skip(!ALERT_PROXY, "needs E2E_BRIDGE_ALERT_PROXY=1 (see the top)");
test.describe.configure({ mode: "serial" });

let proxy: AlertProxy | undefined;

test.beforeAll(async () => {
  proxy = await startAlertProxy({
    lockedAlert: LOCKED,
    port: ALERT_PROXY_PORT,
    quotaAlert: QUOTA,
    upstreamPort: DOVECOT_IMAP_PORT,
  });
});

test.afterAll(async () => {
  await proxy?.close();
});

test("an alert while working shows until closed, as plain text", async ({
  page,
}) => {
  await signIn(page, "alice", DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  const notices = page.getByRole("region", { name: "Notifications" });
  // The toast is aria-hidden for Playwright's role queries: found by text.
  const notice = notices.getByText(QUOTA, { exact: true });
  await expect(notice).toBeVisible({ timeout: 30_000 });
  await expect(
    notices.getByText("Message from your mail server")
  ).toBeVisible();
  // Escaped, never markup.
  await expect(notices.locator("b")).toHaveCount(0);
  // Only one, although every SELECT of every connection repeats it.
  await expect(notices.getByText(QUOTA, { exact: true })).toHaveCount(1);
  // It stays (no timeout)…
  await page.waitForTimeout(8000);
  await expect(notice).toBeVisible();
  // …until closed, and then stays closed after a reload.
  // aria-hidden for role queries, like the toast text: by its label.
  await notices.locator('button[aria-label="Close"]').first().click();
  await expect(notice).toHaveCount(0);
  await page.reload();
  await expect(
    page.getByRole("button", { name: "Account: alice" })
  ).toBeVisible();
  // Moving between folders makes the bridge SELECT again: still closed.
  await page.goto("/mail/sent");
  await page.waitForTimeout(5000);
  await expect(
    page
      .getByRole("region", { name: "Notifications" })
      .getByText(QUOTA, { exact: true })
  ).toHaveCount(0);
});

test("an alert with a refused login shows on the sign-in page", async ({
  page,
}) => {
  await signIn(page, "locked", DOVECOT.password);
  const alert = page.getByRole("alert").filter({ hasText: LOCKED });
  await expect(alert).toBeVisible();
  await expect(alert).toContainText("Message from your mail server:");
  await expect(page).not.toHaveURL(/\/mail\//u);
});
