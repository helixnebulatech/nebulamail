// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// RFC 8621 §4.1.4: `htmlBody` lists the parts "to display (sequentially)",
// text/plain ones included. A list footer after the HTML part (a
// multipart/mixed) shows in its place, as text, and goes into the reply's
// quote (mail/api.ts `htmlBodyContent`).

import { expect, test } from "@playwright/test";

import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

let jmap: FakeJmap;

const FOOTER =
  "Sent to the list <b>team</b> & friends\nUnsubscribe: reply STOP";

test.beforeEach(async ({ context, page }) => {
  test.slow();
  await signIn(context);
  jmap = await installFakeJmap(page);
  jmap.emails.push({
    from: { email: "[EMAIL]", name: "Team list" },
    html: "<p>Meeting moved to <i>Friday</i>.</p>",
    id: "e90",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    plainFooter: FOOTER,
    receivedAt: "2020-01-01T10:00:00Z",
    subject: "Meeting",
  });
});

test("a text/plain part of htmlBody shows after the HTML, as text", async ({
  page,
}) => {
  await page.goto("/mail/inbox/e90");
  const reader = page.getByRole("region", { name: "Message" });
  const body = reader
    .locator("iframe[title]")
    .first()
    .contentFrame()
    .locator("body");
  await expect(body).toContainText("Meeting moved to Friday.", {
    timeout: 20_000,
  });
  // In order, escaped: the tags are text, not markup.
  await expect(body).toContainText(
    "Friday.Sent to the list <b>team</b> & friends"
  );
  await expect(body.locator("b")).toHaveCount(0);
  await expect(body).toContainText("Unsubscribe: reply STOP");

  await reader.getByRole("button", { name: /^Reply(?: R)?$/u }).click();
  const composer = page.locator("section[data-composer]");
  await composer.getByRole("button", { name: "Show quoted text" }).click();
  const quote = composer.locator("[data-quote]");
  await expect(quote).toContainText("Meeting moved to Friday.");
  await expect(quote).toContainText("Sent to the list <b>team</b> & friends");
  await expect(quote.locator("b")).toHaveCount(0);
});
