// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A calendar's time zone and default reminders
// with the fake JMAP server: set them in the calendar's settings, then a
// new event starts with them, follows the calendar and "all day" until
// the reminders are edited by hand, and is saved with explicit alerts
// (never `useDefaultAlerts`). Screenshots go to /tmp/nebula-calendar.

import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import { CALENDAR_ID } from "./fake-calendar";
import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { chooseOption, expectValue } from "./select";
import { signIn } from "./session";

test.use({ timezoneId: "UTC" });

const PHONE_MAX = 640;
const OWN = "Stalwart Calendar (alice@nebulamail.test)";

const shot = async (page: Page, info: TestInfo, name: string) => {
  await page.screenshot({
    path: `/tmp/nebula-calendar/${info.project.name}/${name}.png`,
  });
};

const isPhone = (page: Page): boolean =>
  (page.viewportSize()?.width ?? 0) <= PHONE_MAX;

/** The sidebar: in place, or in the drawer on phones (opened if closed). */
const sidebar = async (page: Page): Promise<Locator> => {
  if (isPhone(page)) {
    const drawer = page.getByRole("dialog", { name: "Calendar menu" });
    if (!(await drawer.isVisible())) {
      await page.getByRole("button", { name: "Calendar menu" }).click();
    }
    return drawer;
  }
  return page.getByRole("navigation", { name: "Calendar menu" });
};

const offsetAlert = (offset: string) => ({
  "@type": "Alert",
  action: "display",
  trigger: { "@type": "OffsetTrigger", offset, relativeTo: "start" },
});

/**
 * Milliseconds from pressing the select to its list in the page with its
 * options (measured in the page, without Playwright's own waits).
 */
const timeToOpen = async (page: Page, select: Locator): Promise<number> => {
  const opening = page.evaluate(async () => {
    const { promise, resolve } = Promise.withResolvers<number>();
    let pressed = 0;
    document.addEventListener(
      "pointerdown",
      () => {
        pressed = performance.now();
      },
      { capture: true, once: true }
    );
    const observer = new MutationObserver(() => {
      if (
        pressed > 0 &&
        document.querySelector('[role="listbox"] [role="option"]') !== null
      ) {
        observer.disconnect();
        resolve(performance.now() - pressed);
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });
    return await promise;
  });
  await select.click();
  await expect(select.page().getByRole("listbox")).toBeVisible();
  return await opening;
};

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
  // A second calendar of ours with its own defaults.
  const [own] = jmap.calendar.calendars;
  jmap.calendar.calendars.push({
    color: "#0E7A4E",
    defaultAlertsWithTime: { w: offsetAlert("-PT5M") },
    defaultAlertsWithoutTime: {},
    id: "work",
    isDefault: false,
    isVisible: true,
    myRights: own?.myRights ?? {},
    name: "Work",
  });
});

test("set a calendar's zone and default reminders, then use them", async ({
  page,
}, info) => {
  await page.goto("/calendar");
  let side = await sidebar(page);
  await side.getByRole("button", { name: `Options for ${OWN}` }).click();
  await page.getByRole("menuitem", { name: "Calendar settings…" }).click();
  let dialog = page.getByRole("dialog", { name: "Calendar", exact: true });
  const zone = dialog.getByRole("combobox", { name: "Time zone" });
  await expectValue(zone, "");
  await chooseOption(zone, "America/New_York");
  const defaults = dialog.getByRole("group", { name: "Default reminders" });
  // The fake has none yet ({} like Stalwart).
  await expect(
    defaults.getByRole("combobox", { name: "Reminder" })
  ).toHaveCount(0);
  const add = defaults.getByRole("button", { name: "Add a reminder" });
  await add.first().click();
  await chooseOption(
    defaults.getByRole("combobox", { name: "Reminder" }).first(),
    "30"
  );
  await add.last().click();
  await chooseOption(
    defaults.getByRole("combobox", { name: "Reminder" }).last(),
    "1440"
  );
  await shot(page, info, "calendar-settings");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1))
    .toMatchObject({
      update: {
        [CALENDAR_ID]: { timeZone: "America/New_York" },
      },
    });
  // Each alert under its own new id (Stalwart 1.0 refuses an id used
  // twice in the account; the fake too).
  const Alerts = z.record(z.string(), z.unknown());
  const saved = z
    .object({
      update: z.record(
        z.string(),
        z.object({
          defaultAlertsWithTime: Alerts,
          defaultAlertsWithoutTime: Alerts,
        })
      ),
    })
    .parse(jmap.calendar.calendarSets.at(-1)).update[CALENDAR_ID];
  expect(Object.values(saved?.defaultAlertsWithTime ?? {})).toStrictEqual([
    offsetAlert("-PT30M"),
  ]);
  expect(Object.values(saved?.defaultAlertsWithoutTime ?? {})).toStrictEqual([
    offsetAlert("-P1D"),
  ]);
  expect(
    new Set([
      ...Object.keys(saved?.defaultAlertsWithTime ?? {}),
      ...Object.keys(saved?.defaultAlertsWithoutTime ?? {}),
    ]).size
  ).toBe(2);

  // Stored: the dialog shows them again.
  side = await sidebar(page);
  await side.getByRole("button", { name: `Options for ${OWN}` }).click();
  await page.getByRole("menuitem", { name: "Calendar settings…" }).click();
  dialog = page.getByRole("dialog", { name: "Calendar", exact: true });
  await expectValue(
    dialog.getByRole("combobox", { name: "Time zone" }),
    "America/New_York"
  );
  const reminders = dialog.getByRole("combobox", { name: "Reminder" });
  await expect(reminders).toHaveCount(2);
  await expectValue(reminders.first(), "30");
  await expectValue(reminders.last(), "1440");
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toHaveCount(0);

  // A new event: in the calendar's zone, with its reminders.
  side = await sidebar(page);
  await side.getByRole("button", { name: /^New event/u }).click();
  const editor = page.getByRole("form", { name: "New event" });
  await expect(editor.getByRole("textbox", { name: "Title" })).toBeFocused();
  await expectValue(
    editor.getByRole("combobox", { name: "Time zone" }),
    "America/New_York"
  );
  const alerts = editor.getByRole("combobox", { name: "Reminder" });
  await expect(alerts).toHaveCount(1);
  await expectValue(alerts, "30");
  await shot(page, info, "new-event-defaults");

  // All day: the all-day defaults; and back.
  await editor.getByRole("switch", { name: "All day" }).click();
  await expectValue(alerts, "1440");
  await editor.getByRole("switch", { name: "All day" }).click();
  await expectValue(alerts, "30");

  // Another calendar: its defaults.
  const calendar = editor.getByRole("combobox", { name: "Calendar" });
  await chooseOption(calendar, { label: "Work" });
  await expectValue(alerts, "5");

  // Edited by hand: they stay when the calendar changes again.
  await chooseOption(alerts, "60");
  await chooseOption(calendar, { label: OWN });
  await expectValue(alerts, "60");

  await editor.getByRole("textbox", { name: "Title" }).fill("Call New York");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      create: {
        event: {
          alerts: { a1: offsetAlert("-PT1H") },
          calendarIds: { [CALENDAR_ID]: true },
          timeZone: "America/New_York",
          title: "Call New York",
        },
      },
    });
  const created = JSON.stringify(jmap.calendar.sets.at(-1));
  expect(created).not.toContain("useDefaultAlerts");
});

test("no defaults: a new event has no reminders; others' calendars hide them", async ({
  page,
}) => {
  await page.goto("/calendar");
  await expect(
    page.getByRole("button", { name: /^Standup, /u }).first()
  ).toBeVisible();
  await page.keyboard.press("c");
  const editor = page.getByRole("form", { name: "New event" });
  await expect(editor.getByRole("textbox", { name: "Title" })).toBeFocused();
  await expectValue(editor.getByRole("combobox", { name: "Time zone" }), "UTC");
  await expect(editor.getByRole("combobox", { name: "Reminder" })).toHaveCount(
    0
  );
  await page.keyboard.press("Escape");
  await expect(editor).toHaveCount(0);

  const side = await sidebar(page);
  await side.getByRole("button", { name: "Options for Team calendar" }).click();
  await page.getByRole("menuitem", { name: "Colour…" }).click();
  const dialog = page.getByRole("dialog", { name: "Calendar", exact: true });
  await expect(dialog.getByRole("button", { name: "Colour 1" })).toBeVisible();
  await expect(dialog.getByRole("combobox", { name: "Time zone" })).toHaveCount(
    0
  );
  await expect(
    dialog.getByRole("group", { name: "Default reminders" })
  ).toHaveCount(0);
});

test("a time zone the server refuses: says so", async ({ page }) => {
  await page.goto("/calendar");
  const side = await sidebar(page);
  await side.getByRole("button", { name: "Options for Work" }).click();
  await page.getByRole("menuitem", { name: "Calendar settings…" }).click();
  const dialog = page.getByRole("dialog", { name: "Calendar", exact: true });
  // The browser knows it; this server doesn't (like an old one).
  jmap.calendar.refuseZones = true;
  await chooseOption(
    dialog.getByRole("combobox", { name: "Time zone" }),
    "Europe/Madrid"
  );
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(
    page.getByText("Your server doesn't accept that time zone.")
  ).toBeVisible();
});

test("the time zone select: a long list, typing to jump, Esc, fits the screen", async ({
  page,
}, info) => {
  await page.goto("/calendar");
  const side = await sidebar(page);
  await side.getByRole("button", { name: `Options for ${OWN}` }).click();
  await page.getByRole("menuitem", { name: "Calendar settings…" }).click();
  const dialog = page.getByRole("dialog", { name: "Calendar", exact: true });
  const zone = dialog.getByRole("combobox", { name: "Time zone" });
  await expectValue(zone, "");

  // A short select first, to compare: the reminders (about ten).
  await dialog
    .getByRole("group", { name: "Default reminders" })
    .getByRole("button", { name: "Add a reminder" })
    .first()
    .click();
  const reminder = dialog.getByRole("combobox", { name: "Reminder" });
  const shortIn = await timeToOpen(page, reminder);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("listbox")).toBeHidden();

  // Every zone the browser knows (hundreds), grouped by region.
  const openedIn = await timeToOpen(page, zone);
  const listbox = page.getByRole("listbox");
  info.annotations.push({
    description: `${String(Math.round(openedIn))} ms (reminders: ${String(Math.round(shortIn))} ms)`,
    type: "time zone list opened in",
  });
  expect(await listbox.getByRole("option").count()).toBeGreaterThan(300);
  await expect(listbox.getByRole("group", { name: "Europe" })).toBeVisible();

  // It fits the screen, and on touch screens every option is 44 px tall.
  const viewport = page.viewportSize();
  const box = await listbox.boundingBox();
  expect(box).not.toBeNull();
  if (box !== null && viewport !== null) {
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(viewport.width);
    expect(box.y + box.height).toBeLessThanOrEqual(viewport.height);
  }
  if (info.project.name !== "desktop") {
    const option = await listbox
      .getByRole("option", { name: /^Madrid/u })
      .boundingBox();
    expect(option?.height ?? 0).toBeGreaterThanOrEqual(44);
  }

  // Typing jumps to the first zone that starts with it; Enter chooses it.
  await page.keyboard.type("Madr");
  await expect(
    listbox.getByRole("option", { name: /^Madrid/u })
  ).toHaveAttribute("data-highlighted", "");
  await page.keyboard.press("Enter");
  await expect(listbox).toBeHidden();
  await expectValue(zone, "Europe/Madrid");
  await expect(zone).toBeFocused();

  // Esc closes without changing anything.
  await zone.press("ArrowDown");
  await expect(listbox).toBeVisible();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Escape");
  await expect(listbox).toBeHidden();
  await expectValue(zone, "Europe/Madrid");
  await expect(dialog).toBeVisible();
  await shot(page, info, "time-zone-select");
});
