// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Misleading senders: a display name that shows
// another address gets a red label in the reader (a brand name alone
// doesn't: there is no brand list); a sender with no earlier mail and not in the contacts gets
// «First time writing to you». One search per opened email, none in the
// list.

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { FakeEmail, FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test.use({ locale: "en-US" });

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName !== "chromium" || info.project.name !== "desktop",
    "the header is the same at every width"
  );
});

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

/** Builds an address (keeps the test free of address literals). */
const at = (local: string, domain: string): string => `${local}@${domain}`;

const inbox = (
  id: string,
  subject: string,
  from: FakeEmail["from"],
  receivedAt: string
): FakeEmail => ({
  from,
  html: `<p>${subject}</p>`,
  id,
  keywords: {},
  mailboxIds: { inbox: true },
  receivedAt,
  subject,
});

const reader = (page: Page) => page.getByRole("region", { name: "Message" });

/** The first-contact searches: Email/query with `before` and From/To/Cc. */
const firstContactSearches = (page: Page) => {
  const seen: unknown[] = [];
  page.on("request", (request) => {
    const body = request.postData() ?? "";
    if (
      request.url().startsWith("http://jmap.test") &&
      body.includes('"Email/query"') &&
      body.includes('"before"') &&
      body.includes('{"cc":')
    ) {
      seen.push(body);
    }
  });
  return seen;
};

test("a brand name from a new address: only «first time», no brand label", async ({
  page,
}) => {
  jmap.emails.unshift(
    inbox(
      "w-brand",
      "Your account is limited",
      { email: at("alerta", "cuentas-seguras.example"), name: "PayPal" },
      "2026-11-03T09:00:00Z"
    )
  );
  const searches = firstContactSearches(page);
  await page.goto("/mail/inbox");
  await expect(page.getByText("Your account is limited").first()).toBeVisible();
  await page.waitForLoadState("networkidle");
  // The list alone asks nothing about senders.
  expect(searches).toHaveLength(0);

  await page.goto("/mail/inbox/w-brand");
  const header = reader(page);
  await expect(header.getByText("First time writing to you")).toBeVisible();
  await expect.poll(() => searches.length).toBe(1);
  await expect(header.locator("[data-sender-warning]")).toHaveCount(0);
});

test("a name showing another address, and a sender who wrote before", async ({
  page,
}) => {
  const sender = at("ops", "tienda.example");
  jmap.emails.unshift(
    inbox(
      "w-second",
      "Second message",
      { email: sender, name: at("director", "banco.example") },
      "2026-11-03T09:00:00Z"
    ),
    inbox(
      "w-first",
      "First message",
      { email: sender, name: "Ops" },
      "2026-11-02T09:00:00Z"
    )
  );

  await page.goto("/mail/inbox/w-second");
  const header = reader(page);
  await expect(header.getByText("Name shows another address")).toBeVisible();
  // It wrote before (w-first): no «first time».
  await page.waitForLoadState("networkidle");
  await expect(header.getByText("First time writing to you")).toHaveCount(0);

  // The first one was the first time, and stays so.
  await page.goto("/mail/inbox/w-first");
  await expect(
    reader(page).getByText("First time writing to you")
  ).toBeVisible();
  await expect(reader(page).locator("[data-sender-warning]")).toHaveCount(0);
});

test("a brand writing from a country domain gets no label", async ({
  page,
}) => {
  jmap.emails.unshift(
    inbox(
      "w-real",
      "Receipt",
      { email: at("service", "paypal.com.mx"), name: "PayPal" },
      "2026-11-03T09:00:00Z"
    )
  );
  await page.goto("/mail/inbox/w-real");
  await expect(reader(page).getByText("Receipt").first()).toBeVisible();
  await page.waitForLoadState("networkidle");
  await expect(reader(page).locator("[data-sender-warning]")).toHaveCount(0);
});

test("with «Flag first-time senders» off: no hint and no search", async ({
  page,
}) => {
  jmap.emails.unshift(
    inbox(
      "w-new-off",
      "Hello from a stranger",
      { email: at("someone", "new.example"), name: "Someone" },
      "2026-11-03T09:00:00Z"
    )
  );
  const searches = firstContactSearches(page);

  // Only in Settings › General (on by default), not in quick settings.
  await page.goto("/settings/general");
  const toggle = page.getByRole("switch", { name: "Flag first-time senders" });
  await expect(toggle).toBeChecked();
  await page.waitForLoadState("networkidle");
  await toggle.click();
  await expect(toggle).not.toBeChecked();

  await page.goto("/mail/inbox/w-new-off");
  await expect(
    reader(page).getByText("Hello from a stranger").first()
  ).toBeVisible();
  await page.waitForLoadState("networkidle");
  await expect(reader(page).locator("[data-sender-new]")).toHaveCount(0);
  expect(searches).toHaveLength(0);

  // On again: the same email gets it (and its one search).
  await page.goto("/settings/general");
  await page.getByRole("switch", { name: "Flag first-time senders" }).click();
  await page.goto("/mail/inbox/w-new-off");
  await expect(
    reader(page).getByText("First time writing to you")
  ).toBeVisible();
  await expect.poll(() => searches.length).toBe(1);
});
