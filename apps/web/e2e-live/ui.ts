// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the browser tests against real servers do in the app
// write and
// send an email, find it in Sent, and the parts of the reader. Shared by
// e2e-live (Stalwart) and e2e-bridge (the bridge).

import type { EmailId, JmapClient } from "@nebulamail/jmap-client";
import type { Locator, Page } from "@playwright/test";
import { expect } from "@playwright/test";

/**
 * After "Sign in" on Stalwart's page: Stalwart 1.0 then asks the user to
 * allow the app (its consent screen, `#allow-btn` in its login.html), the
 * first time and again once the grant is gone (revoked at sign-out, or 30
 * days unused). Clicks it if it shows up, until the page reaches `done`.
 */
export const allowIfAsked = async (
  page: Page,
  done: RegExp | string
): Promise<void> => {
  const allow = page.locator("#allow-btn");
  await expect(async () => {
    if (await allow.isVisible()) {
      await allow.click();
    }
    await expect(page).toHaveURL(done, { timeout: 1000 });
  }).toPass();
};

export const reader = (page: Page): Locator =>
  page.getByRole("region", { exact: true, name: "Message" });

export const row = (page: Page, subject: string): Locator =>
  page
    .getByRole("region", { name: "Messages" })
    .getByRole("listitem")
    .filter({ hasText: subject });

const composer = (page: Page): Locator =>
  page.locator("section[data-composer]");

/**
 * Opens the composer. A click before the page has hydrated does nothing,
 * and "networkidle" never comes (the push connection stays open): click
 * until it opens.
 */
export const openComposer = async (page: Page): Promise<Locator> => {
  const to = composer(page).getByRole("combobox", { name: "To" });
  await expect(async () => {
    await page
      .getByRole("button", { name: /^(?:New message|Compose)/u })
      .filter({ visible: true })
      .first()
      .click();
    await expect(to).toBeVisible({ timeout: 1000 });
  }).toPass();
  return composer(page);
};

/** The composer's "Request read receipt" item, with its menu open. */
export const receiptOption = async (page: Page): Promise<Locator> => {
  await composer(page)
    .getByRole("button", { name: "More send options" })
    .click();
  return page.getByRole("menuitemcheckbox", { name: "Request read receipt" });
};

export interface Outgoing {
  readonly to: string;
  readonly subject: string;
  readonly body: string;
  /**
   * "request": tick "Request read receipt" (the server must offer MDN);
   * "unavailable": check that the option isn't there (no MDN).
   */
  readonly receipt?: "request" | "unavailable";
}

/** Writes and sends an email from the app; waits for "Sent". */
export const send = async (page: Page, email: Outgoing): Promise<void> => {
  const box = await openComposer(page);
  await box.getByRole("combobox", { name: "To" }).fill(`${email.to},`);
  await box.getByRole("textbox", { name: "Subject" }).fill(email.subject);
  await box.getByRole("textbox", { name: "Message" }).fill(email.body);
  if (email.receipt !== undefined) {
    const option = await receiptOption(page);
    if (email.receipt === "request") {
      await expect(option).not.toBeChecked();
      await option.click();
      await expect(option).toBeChecked();
    } else {
      // The menu is open (it has "Send later"), without the option.
      await expect(page.getByRole("menu")).toBeVisible();
      await expect(option).toHaveCount(0);
    }
    await page.keyboard.press("Escape");
  }
  await box.getByRole("button", { name: /^Send/u }).click();
  await expect(
    page.getByRole("region", { name: "Notifications" })
  ).toContainText("Sent");
};

/** The ids of this account's emails with this subject (any folder). */
export const emailsWithSubject = async (
  client: JmapClient,
  subject: string
): Promise<EmailId[]> => {
  const [query] = await client.request((b) => [
    b.call("Email/query", {
      accountId: client.primaryAccountId(),
      filter: { subject },
    }),
  ]);
  return [...query.ids];
};

/**
 * Deletes for good every email of the account with this text anywhere
 * (subject or body: bounces quote the original only in the body).
 */
export const destroyMatching = async (
  client: JmapClient,
  text: string
): Promise<void> => {
  const [query] = await client.request((b) => [
    b.call("Email/query", {
      accountId: client.primaryAccountId(),
      filter: { text },
    }),
  ]);
  const { ids } = query;
  if (ids.length === 0) {
    return;
  }
  await client.request((b) => [
    b.call("Email/set", {
      accountId: client.primaryAccountId(),
      destroy: ids,
    }),
  ]);
};

/** Where screenshots for a manual look go (git-ignored test-results). */
export const shot = (name: string): { path: string } => ({
  path: `test-results/live-${name}.png`,
});
