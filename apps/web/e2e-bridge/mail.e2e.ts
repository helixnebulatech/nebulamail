// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The app against the real bridge and Dovecot: signing in
// on the bridge's own page, sending and finding the message in Sent.

import { expect, test } from "@playwright/test";

import { DOVECOT } from "./env";
import { signIn } from "./session";

test("signs in, sends and finds the message in Sent", async ({ page }) => {
  await signIn(page, "alice", DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  await expect(
    page.getByRole("button", { name: "Account: alice" })
  ).toBeVisible();

  const subject = `Nebula bridge e2e ${Date.now()}`;
  // A click before the page has hydrated does nothing, and "networkidle"
  // never comes (the push connection stays open): click until it opens.
  const to = page.getByRole("combobox", { name: "To" });
  await expect(async () => {
    await page.getByRole("button", { name: /^New message/u }).click();
    await expect(to).toBeVisible({ timeout: 1000 });
  }).toPass();
  await to.fill("bob@nebulamail.test,");
  await page.getByRole("textbox", { name: "Subject" }).fill(subject);
  await page
    .getByRole("textbox", { name: "Message" })
    .fill("Sent by the bridge browser test. Safe to delete.");
  await page.getByRole("button", { name: /^Send/u }).click();
  await expect(
    page.getByRole("region", { name: "Notifications" })
  ).toContainText("Sent");

  await page.getByRole("link", { name: "Sent" }).click();
  const rows = page
    .getByRole("region", { name: "Messages" })
    .getByText(subject);
  // Exactly one copy: the draft moved to Sent, no second APPEND.
  await expect(rows).toHaveCount(1);
  await rows.click();
  // The body is shown in the sandboxed iframe of the reader.
  const reader = page.getByRole("region", { exact: true, name: "Message" });
  await expect(
    reader
      .locator("iframe")
      .contentFrame()
      .getByText("Sent by the bridge browser test.")
  ).toBeVisible();

  // Clean up: to Trash.
  await reader.getByRole("button", { name: "Delete" }).click();
  await expect(rows).toHaveCount(0);
});

test("a wrong password stays on the bridge's page", async ({ page }) => {
  await signIn(page, "alice", "definitely-wrong");
  await expect(page.getByText("Wrong username or password.")).toBeVisible();
});
