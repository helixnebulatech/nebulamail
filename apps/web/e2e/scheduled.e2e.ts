// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Scheduled send: a quick time from the send menu goes out
// as HOLDUNTIL in the envelope (the user's zone, in UTC), the «Programados»
// view lists it, changes its time (cancel, then submit again) and cancels
// it (back to Drafts). ⌘⇧↵ opens the date and time dialog without sending,
// a server that refuses the time keeps the composer open and teaches its
// limit, and without FUTURERELEASE nothing of this is offered.

import { CalendarDateTime, now, toZoned } from "@internationalized/date";
import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

const ZONE = "Europe/Madrid";

test.use({ locale: "en-US", timezoneId: ZONE });

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName !== "chromium" || info.project.name !== "desktop",
    "the menus and the view are the same at every width"
  );
});

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
  jmap.futureRelease = true;
});

/** Builds an address (keeps the test free of address literals). */
const addr = (local: string): string => `${local}@example.com`;

/** HOLDUNTIL of tomorrow at `hour`:00 in the test's zone. */
const tomorrowAt = (hour: number): string => {
  const day = now(ZONE).add({ days: 1 });
  return toZoned(
    new CalendarDateTime(day.year, day.month, day.day, hour, 0),
    ZONE
  )
    .toDate()
    .toISOString()
    .replace(".000Z", "Z");
};

const Send = z.object({
  create: z.record(
    z.string(),
    z.object({
      emailId: z.string(),
      envelope: z.object({
        mailFrom: z.object({
          parameters: z.record(z.string(), z.string()).nullable(),
        }),
        rcptTo: z.array(z.object({ email: z.string() })),
      }),
    })
  ),
});

const holdOfSend = (send: unknown): string | undefined =>
  Object.values(Send.parse(send).create)[0]?.envelope.mailFrom.parameters
    ?.HOLDUNTIL;

const notices = (page: Page) =>
  page.getByRole("region", { name: "Notifications" });

/** Opens a composer and writes a short message to Bob. */
const write = async (page: Page, subject: string) => {
  await page.goto("/mail/inbox");
  await page.waitForLoadState("networkidle");
  await page
    .getByRole("button", { name: /^(?:New message|Compose)/u })
    .filter({ visible: true })
    .first()
    .click();
  const composer = page.locator("section[data-composer]");
  await expect(composer).toBeVisible();
  const to = composer.getByRole("combobox", { name: "To" });
  await to.fill(addr("bob"));
  await to.press("Enter");
  await composer.getByRole("textbox", { name: "Subject" }).fill(subject);
  await composer
    .getByRole("textbox", { name: "Message" })
    .pressSequentially("See you");
  return composer;
};

const sendMenu = async (page: Page) => {
  await page
    .locator("section[data-composer]")
    .getByRole("button", { name: "More send options" })
    .click();
  return page.getByRole("menu");
};

test("schedule from the menu, change the time and cancel from «Scheduled»", async ({
  page,
}) => {
  const composer = await write(page, "Weekly report");
  const menu = await sendMenu(page);
  await menu.getByRole("menuitem", { name: /^Tomorrow morning/u }).click();

  await expect(composer).toBeHidden();
  await expect(
    notices(page)
      .getByText(/^Scheduled for /u)
      .first()
  ).toBeVisible();
  expect(jmap.mailSends).toHaveLength(1);
  expect(holdOfSend(jmap.mailSends[0])).toBe(tomorrowAt(8));
  expect(jmap.submissions[0]?.undoStatus).toBe("pending");

  await page
    .getByRole("navigation", { name: /./u })
    .getByRole("link", { name: "Scheduled" })
    .first()
    .click();
  await expect(page).toHaveURL(/\/mail\/scheduled$/u);
  const list = page.getByRole("list", { name: "Scheduled" });
  const row = list.getByRole("listitem").filter({ hasText: "Weekly report" });
  await expect(row).toBeVisible();
  await expect(row).toContainText(`To: ${addr("bob")}`);
  await expect(row).toContainText(/Sends .*8:00/u);

  // Change the time: the old submission is cancelled, a new one made.
  await row.getByRole("button", { name: "Change time" }).click();
  const dialog = page.getByRole("dialog", { name: "Change send time" });
  await dialog.getByRole("button", { name: /^Tomorrow afternoon/u }).click();
  await expect(
    notices(page)
      .getByText(/^Time changed: sends /u)
      .first()
  ).toBeVisible();
  await expect(dialog).toBeHidden();
  expect(jmap.submissions.map((item) => item.undoStatus)).toStrictEqual([
    "canceled",
    "pending",
  ]);
  expect(jmap.submissions[1]?.envelope?.mailFrom.parameters).toStrictEqual({
    HOLDUNTIL: tomorrowAt(14),
  });
  await expect(row).toContainText(/Sends .*2:00/u);

  // Cancel: back to Drafts, editable again.
  await row.getByRole("button", { name: "Cancel send" }).click();
  await expect(
    notices(page).getByText("Send cancelled. The message is in Drafts.")
  ).toBeVisible();
  expect(jmap.submissions[1]?.undoStatus).toBe("canceled");
  const email = jmap.emails.find(
    (item) => item.id === jmap.submissions[1]?.emailId
  );
  expect(email?.mailboxIds).toStrictEqual({ drafts: true });
  expect(email?.keywords.$draft).toBe(true);
  await expect(page.getByText("No scheduled messages")).toBeVisible();
});

test("in Sent a scheduled message says «Scheduled» until it goes out", async ({
  page,
}) => {
  await write(page, "Not yet");
  const menu = await sendMenu(page);
  await menu.getByRole("menuitem", { name: /^Tomorrow morning/u }).click();
  await expect(
    notices(page)
      .getByText(/^Scheduled for /u)
      .first()
  ).toBeVisible();

  await page.goto("/mail/sent");
  const row = page
    .getByRole("list", { name: "Messages" })
    .getByRole("listitem")
    .filter({ hasText: "Not yet" });
  const mark = row.locator("[data-scheduled]");
  await expect(mark).toBeVisible();
  await expect(mark).toHaveText(/^Scheduled · .*8:00/u);
  await expect(mark).toHaveAttribute("data-scheduled", tomorrowAt(8));

  // The open email says so too, with the way to change or cancel it.
  await row.getByRole("link").first().click();
  const notice = page.locator("article [data-scheduled]");
  await expect(notice).toContainText(/Scheduled · .*8:00/u);
  await notice.getByRole("link", { name: "Open in Scheduled" }).click();
  await expect(page).toHaveURL(/\/mail\/scheduled$/u);
  await expect(
    page.getByRole("list", { name: "Scheduled" }).getByText("Not yet")
  ).toBeVisible();

  // Once it has gone out (no longer pending), no mark.
  const [submission] = jmap.submissions;
  if (submission !== undefined) {
    submission.undoStatus = "final";
  }
  await page.goto("/mail/sent");
  await expect(
    page.getByRole("list", { name: "Messages" }).getByText("Not yet")
  ).toBeVisible();
  await expect(page.locator("[data-scheduled]")).toHaveCount(0);
});

test("⌘⇧↵ opens the date and time dialog without sending", async ({ page }) => {
  const composer = await write(page, "Shortcut");
  await composer
    .getByRole("textbox", { name: "Subject" })
    .press("ControlOrMeta+Shift+Enter");

  const dialog = page.getByRole("dialog", { name: "Schedule send" });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("Your server keeps scheduled messages");
  expect(jmap.mailSends).toHaveLength(0);

  await dialog.getByRole("button", { name: /^Tomorrow morning/u }).click();
  await expect(composer).toBeHidden();
  expect(holdOfSend(jmap.mailSends[0])).toBe(tomorrowAt(8));
});

test("a time the server refuses keeps the message and shows its limit", async ({
  page,
}) => {
  // Like Stalwart 1.0.1: it says 30 days, its queue takes less.
  jmap.releaseMax = tomorrowAt(10);
  const composer = await write(page, "Too far");
  const menu = await sendMenu(page);
  await menu.getByRole("menuitem", { name: /^Tomorrow afternoon/u }).click();

  await expect(
    notices(page)
      .getByText(/^The server only allows scheduling until /u)
      .first()
  ).toBeVisible();
  await expect(composer).toBeVisible();
  expect(jmap.submissions).toHaveLength(0);

  // Now the menu knows: that time is offered no more.
  const again = await sendMenu(page);
  await expect(
    again.getByRole("menuitem", { name: /^Tomorrow afternoon/u })
  ).toBeDisabled();
  await expect(
    again.getByRole("menuitem", { name: /^Tomorrow morning/u })
  ).toBeEnabled();
});

test("without FUTURERELEASE nothing is offered", async ({ page }) => {
  jmap.futureRelease = false;
  await write(page, "Now only");
  const menu = await sendMenu(page);

  await expect(menu.getByText("Schedule send")).toHaveCount(0);
  await expect(menu.getByRole("menuitem", { name: /^Tomorrow/u })).toHaveCount(
    0
  );
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("navigation").getByRole("link", { name: "Scheduled" })
  ).toHaveCount(0);
});

/** The bridge's vendor property (sent after sign-out). */
const SIGNED_OUT = "https://helixnebula.tech/jmap/bridge/sendWhenSignedOut";

const signedOutOfSend = (send: unknown): unknown =>
  Object.values(
    z
      .object({
        create: z.record(z.string(), z.record(z.string(), z.unknown())),
      })
      .parse(send).create
  )[0]?.[SIGNED_OUT];

test("on the bridge: send after sign-out from Settings, per message, and a warning at sign-out", async ({
  page,
}) => {
  jmap.sendWhenSignedOut = true;
  // The setting, off by default.
  await page.goto("/settings/general");
  const setting = page.getByRole("switch", {
    name: "Send scheduled mail even after signing out",
  });
  await expect(setting).not.toBeChecked();
  await expect(
    page.getByText("the server keeps your password, encrypted")
  ).toBeVisible();
  // A click before the page has hydrated does nothing.
  await expect(async () => {
    await setting.click();
    await expect(setting).toBeChecked({ timeout: 1000 });
  }).toPass();

  await write(page, "Leaves anyway");
  const menu = await sendMenu(page);
  await menu.getByRole("menuitem", { name: /^Tomorrow morning/u }).click();
  await expect(
    notices(page)
      .getByText(/^Scheduled for /u)
      .first()
  ).toBeVisible();
  expect(signedOutOfSend(jmap.mailSends[0])).toBe(true);

  // «Scheduled» shows it, and it can be turned off for this one.
  await page.goto("/mail/scheduled");
  const row = page
    .getByRole("list", { name: "Scheduled" })
    .getByRole("listitem")
    .filter({ hasText: "Leaves anyway" });
  const perMessage = row.getByRole("switch", {
    name: "Sent even if you sign out",
  });
  await expect(perMessage).toBeChecked();
  await perMessage.click();
  await expect(perMessage).not.toBeChecked();
  await expect.poll(() => jmap.submissions[0]?.[SIGNED_OUT]).toBe(false);

  // Now it would wait for the next sign-in: signing out asks first.
  await page.getByRole("button", { name: /^Account/u }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
  const confirm = page.getByRole("alertdialog", { name: "Sign out?" });
  await expect(confirm).toContainText(
    "1 scheduled message that will not be sent until you sign in again"
  );
  await confirm.getByRole("button", { name: "Cancel" }).click();
  await expect(confirm).toBeHidden();
  await expect(page).toHaveURL(/\/mail\/scheduled$/u);
});

test("elsewhere the choice is not offered", async ({ page }) => {
  await page.goto("/settings/general");
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await expect(
    page.getByRole("switch", {
      name: "Send scheduled mail even after signing out",
    })
  ).toHaveCount(0);
  await write(page, "Plain");
  const menu = await sendMenu(page);
  await menu.getByRole("menuitem", { name: /^Tomorrow morning/u }).click();
  await expect(
    notices(page)
      .getByText(/^Scheduled for /u)
      .first()
  ).toBeVisible();
  expect(signedOutOfSend(jmap.mailSends[0])).toBeUndefined();
});
