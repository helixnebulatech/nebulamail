// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Nextcloud's calendar extras through the bridge
// publishing a calendar at a public link, subscribing to a
// calendar URL and the trash bin. Shown only where the account's server
// offers them (the bridge's account capability).

import type { Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { CALENDAR_ID, PUBLIC_LINK } from "./fake-calendar";
import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test.use({ timezoneId: "UTC" });

const OWN = "Stalwart Calendar (alice@nebulamail.test)";
const PHONE_MAX = 640;

/** The sidebar: in place, or in the drawer on phones. */
const sidebar = async (page: Page): Promise<Locator> => {
  if ((page.viewportSize()?.width ?? 0) <= PHONE_MAX) {
    const drawer = page.getByRole("dialog", { name: "Calendar menu" });
    // Still open after a dialog opened from it closes.
    if (!(await drawer.isVisible())) {
      await page.getByRole("button", { name: "Calendar menu" }).click();
    }
    return drawer;
  }
  return page.getByRole("navigation", { name: "Calendar menu" });
};

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

const openShare = async (page: Page): Promise<Locator> => {
  const side = await sidebar(page);
  await side.getByRole("button", { name: `Options for ${OWN}` }).click();
  await page.getByRole("menuitem", { name: "Share…" }).click();
  return page.getByRole("dialog", { name: `Share “${OWN}”` });
};

test("publish a calendar at a public link, then stop", async ({ page }) => {
  jmap.calendar.bridge = true;
  jmap.calendar.nextcloud = true;
  await page.goto("/calendar");
  const dialog = await openShare(page);
  const toggle = dialog.getByRole("switch", {
    name: "Publish at a public link",
  });
  await expect(toggle).not.toBeChecked();
  await expect(
    dialog.getByText("Anyone with the link can see every event")
  ).toBeVisible();
  await toggle.click();
  await expect(toggle).toBeChecked();
  await expect(
    dialog.getByRole("textbox", { name: "Public link" })
  ).toHaveValue(PUBLIC_LINK);
  await expect(dialog.getByRole("button", { name: "Copy link" })).toBeVisible();
  expect(jmap.calendar.calendarSets.at(-1)).toMatchObject({
    update: {
      [CALENDAR_ID]: {
        "https:~1~1helixnebula.tech~1jmap~1bridge~1isPublished": true,
      },
    },
  });
  await toggle.click();
  await expect(toggle).not.toBeChecked();
  await expect(
    dialog.getByRole("textbox", { name: "Public link" })
  ).toHaveCount(0);
});

test("elsewhere: no publishing, subscriptions or trash", async ({ page }) => {
  jmap.calendar.bridge = true;
  await page.goto("/calendar");
  const dialog = await openShare(page);
  await expect(
    dialog.getByText("Only you can see this calendar.")
  ).toBeVisible();
  await expect(dialog.getByRole("switch")).toHaveCount(0);
  await page.keyboard.press("Escape");
  const side = await sidebar(page);
  await expect(
    side.getByRole("button", { name: "More calendar options" })
  ).toHaveCount(0);
});

test("subscribe to a calendar URL, then unsubscribe", async ({ page }) => {
  jmap.calendar.bridge = true;
  jmap.calendar.nextcloud = true;
  await page.goto("/calendar");
  let side = await sidebar(page);
  await side.getByRole("button", { name: "More calendar options" }).click();
  await page
    .getByRole("menuitem", { name: "Subscribe to a calendar…" })
    .click();
  const dialog = page.getByRole("dialog", { name: "Subscribe to a calendar" });
  const address = dialog.getByRole("textbox", {
    name: "Address (https:// or webcal://)",
  });
  await address.fill("http://example.com/holidays.ics");
  await dialog.getByRole("textbox", { name: "Name" }).fill("Festivos");
  await dialog.getByRole("button", { name: "Subscribe" }).click();
  await expect(dialog.getByRole("alert")).toHaveText(
    "The address must start with https://, webcal:// or webcals://."
  );
  await address.fill("https://me:secret@example.com/holidays.ics");
  await dialog.getByRole("button", { name: "Subscribe" }).click();
  await expect(dialog.getByRole("alert")).toHaveText(
    "The address can’t contain a user name or password."
  );
  expect(
    jmap.calendar.calendarSets.filter((set) => "create" in set)
  ).toHaveLength(0);
  await address.fill("webcal://example.com/holidays.ics");
  await dialog.getByRole("button", { name: "Subscribe" }).click();
  await expect(dialog).toHaveCount(0);
  expect(jmap.calendar.calendarSets.at(-1)).toMatchObject({
    create: {
      subscription: {
        "https://helixnebula.tech/jmap/bridge/subscriptionUrl":
          "webcal://example.com/holidays.ics",
        name: "Festivos",
      },
    },
  });
  side = await sidebar(page);
  await expect(side.getByText("Festivos", { exact: true })).toBeVisible();
  await side.getByRole("button", { name: "Options for Festivos" }).click();
  await page.getByRole("menuitem", { name: "Unsubscribe" }).click();
  const confirm = page.getByRole("alertdialog", {
    name: "Unsubscribe from “Festivos”?",
  });
  await confirm.getByRole("button", { name: "Unsubscribe" }).click();
  side = await sidebar(page);
  await expect(side.getByText("Festivos", { exact: true })).toHaveCount(0);
});

test("the trash bin: restore a calendar, delete an event for good", async ({
  page,
}) => {
  jmap.calendar.bridge = true;
  jmap.calendar.nextcloud = true;
  jmap.calendar.trash.push(
    { id: "c00000000000000aa", kind: "calendar", name: "Viajes" },
    {
      calendarId: CALENDAR_ID,
      id: "e00000000000000bb",
      kind: "event",
      name: "Dentista",
    }
  );
  await page.goto("/calendar");
  let side = await sidebar(page);
  await side.getByRole("button", { name: "More calendar options" }).click();
  await page.getByRole("menuitem", { name: "Trash" }).click();
  const dialog = page.getByRole("dialog", { name: "Calendar trash" });
  await expect(
    dialog.getByText("Items here are deleted for good after 30 days.")
  ).toBeVisible();
  await expect(dialog.getByText("Viajes")).toBeVisible();
  await expect(dialog.getByText("Dentista")).toBeVisible();
  await dialog.getByRole("button", { name: "Restore “Viajes”" }).click();
  await expect(dialog.getByText("Viajes")).toHaveCount(0);
  await dialog
    .getByRole("button", { name: "Delete “Dentista” forever" })
    .click();
  await expect(dialog.getByText("The trash is empty.")).toBeVisible();
  expect(jmap.calendar.trashSets).toStrictEqual([
    { accountId: "a", restore: ["c00000000000000aa"] },
    { accountId: "a", destroy: ["e00000000000000bb"] },
  ]);
  await page.keyboard.press("Escape");
  side = await sidebar(page);
  await expect(side.getByText("Viajes", { exact: true })).toBeVisible();
});
