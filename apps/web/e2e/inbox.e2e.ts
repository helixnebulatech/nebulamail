// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The split inbox tabs: the unread count is on "Unread",
// where you'd look for it, not on "Important".

import { expect, test } from "@playwright/test";

import { deliver, installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test("the unread count is on the Unread tab", async ({ context, page }) => {
  await signIn(context);
  const jmap = await installFakeJmap(page);
  const unread = jmap.emails.filter(
    (email) => email.mailboxIds.inbox === true && email.keywords.$seen !== true
  ).length;
  expect(unread).toBeGreaterThan(0);

  await page.goto("/mail/inbox");
  const tabs = page.getByRole("tablist");
  await expect(
    tabs.getByRole("tab", { name: `Unread ${unread}` })
  ).toBeVisible();
  await expect(tabs.getByRole("tab", { name: "Important" })).toBeVisible();
});

test("a search the server refuses says «no preview», without Retry", async ({
  context,
  page,
}) => {
  await signIn(context);
  const jmap = await installFakeJmap(page);
  // Like a search over the server's limits (the box keeps queries short).
  jmap.refuseSearches = true;

  // No automatic retry either: unit test in mail/list-status.test.ts (the
  // app asks again for other reasons, so counting requests here can't tell).
  await page.goto("/mail/inbox?q=quarterly");
  await expect(
    page.getByText("No preview for this search", { exact: false })
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toHaveCount(0);
});

// 2026-11-08, full BFF mode on Vercel: a fresh login stayed on «Loading
// messages…» with no request to /jmap-proxy/. The token endpoint (the
// first request, before /jmap-proxy/grant) failed twice; the folders query
// gave up, and the list (which waits for the folders) stayed "pending".
test("if the folders can't load, the list says so and Retry works", async ({
  context,
  page,
}) => {
  await signIn(context);
  const jmap = await installFakeJmap(page);
  let failing = true;
  await page.route("**/auth/token*", async (route) => {
    await (failing
      ? route.fulfill({ body: "Bad gateway", status: 502 })
      : route.fallback());
  });

  await page.goto("/mail/inbox");
  const list = page.getByRole("main");
  await expect(list.getByText("Couldn't load your messages.")).toBeVisible();
  await expect(list.getByText("Loading messages…")).toHaveCount(0);

  failing = false;
  await list.getByRole("button", { name: "Try again" }).click();
  await expect(list.getByText(jmap.emails[0]?.subject ?? "")).toBeVisible();
});

test("without push, new mail still shows up within a minute", async ({
  context,
  page,
}) => {
  await signIn(context);
  const jmap = await installFakeJmap(page);
  // The server's event source fails.
  jmap.pushDown = true;
  await page.clock.install();
  await page.goto("/mail/inbox");
  const list = page.getByRole("main");
  await expect(list.getByText(jmap.emails[0]?.subject ?? "")).toBeVisible();

  deliver(jmap, {
    from: { email: "ana@example.com", name: "Ana Ruiz" },
    html: "<p>Hi</p>",
    id: "polled-1",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: new Date().toISOString(),
    subject: "Arrived while push was down",
  });
  await page.clock.runFor(30_000);
  await expect(list.getByText("Arrived while push was down")).toHaveCount(0);
  await page.clock.runFor(31_000);
  await expect(list.getByText("Arrived while push was down")).toBeVisible();
});

test("right-click on a row: its actions, and «Move to…» lists the folders", async ({
  context,
  page,
}) => {
  test.skip(
    test.info().project.name !== "desktop",
    "a mouse right-click (touch opens it with a long press)"
  );
  await signIn(context);
  const jmap = await installFakeJmap(page);
  await page.goto("/mail/inbox");
  const row = page
    .getByRole("region", { name: "Messages" })
    .getByRole("listitem")
    .filter({ hasText: "Weekly digest" });
  await row.click({ button: "right" });
  const menu = page.getByRole("menu");
  await Promise.all(
    ["Archive", "Delete", "Report spam", "Move to…", "Star"].map(
      async (name) => {
        await expect(
          menu.getByRole("menuitem", { name: new RegExp(`^${name}`, "u") })
        ).toBeVisible();
      }
    )
  );
  await menu.getByRole("menuitem", { name: /^Move to/u }).hover();
  await expect(
    page.getByRole("menu").last().getByRole("menuitem", { name: "Trash" })
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");

  const before = jmap.updates.length;
  await row.click({ button: "right" });
  await page.getByRole("menuitem", { name: /^Archive/u }).click();
  await expect.poll(() => jmap.updates.length).toBeGreaterThan(before);
  await expect(row).toHaveCount(0);
});
