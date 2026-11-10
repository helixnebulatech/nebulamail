// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tasks in the browser against the fake JMAP server, whose
// tasks behave like Stalwart's (fake-calendar.ts: no filter by
// type, the series' due in every occurrence, overrides of repeating
// tasks). Views, quick add, editing, completing and undo, subtasks with
// Tab / Shift+Tab (and by dropping one task on another), a repeating
// task's occurrence, a task's reminder, lists (also an existing calendar,
// and taking one out of Tasks),
// the phone layout, tasks in the calendar, a task from an email, the
// keyboard and the rights of a shared list. Screenshots go to
// /tmp/nebula-tasks (outside the repo).

import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, test } from "@playwright/test";

import {
  ERRANDS_ID,
  SHARED_CALENDAR_ACCOUNT,
  SHARED_CALENDAR_ID,
  TASK_ASSIGNMENT_BLOB,
  TASK_ASSIGNMENT_UID,
  TASK_COUNTER_BLOB,
  TASK_SERIES_BLOB,
  TASK_SERIES_UID,
  TASK_ID,
  TASK_TITLE,
  addTaskFixtures,
  assignedSeries,
  assignedTask,
  myAssignedTask,
  todayUtc,
  tomorrowUtc,
} from "./fake-calendar";
import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { chooseOption, expectValue, optionNames } from "./select";
import { signIn } from "./session";

// Due times in the fake server are UTC.
test.use({ timezoneId: "UTC" });

const PHONE_MAX = 640;
const NARROW_MAX = 1000;

const shot = async (page: Page, info: TestInfo, name: string) => {
  await page.screenshot({
    path: `/tmp/nebula-tasks/${info.project.name}/${name}.png`,
  });
};

const width = (page: Page): number => page.viewportSize()?.width ?? 0;
const isPhone = (page: Page): boolean => width(page) <= PHONE_MAX;

const expectNoSidewaysScroll = async (page: Page) => {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth
  );
  expect(overflow).toBeLessThanOrEqual(0);
};

/** The task list (the main panel). */
const list = (page: Page): Locator =>
  page.getByRole("region", { name: "Tasks" });

const row = (page: Page, title: string): Locator =>
  list(page).getByRole("button", { name: new RegExp(title, "u") });

const panel = (page: Page): Locator =>
  page.getByRole("region", { name: "Task", exact: true });

/** A view or list in the sidebar (in the drawer on phones). */
const goTo = async (page: Page, name: RegExp) => {
  if (isPhone(page)) {
    await page.getByRole("button", { name: "Tasks sidebar" }).click();
  }
  await page
    .getByRole("navigation", { name: "Tasks sidebar" })
    .getByRole("button", { name })
    .click();
};

/** Where an element is on screen (it must be there). */
const boxOf = async (locator: Locator) => {
  const found = await locator.boundingBox();
  if (found === null) {
    throw new Error("not on screen");
  }
  return found;
};

const lastSet = (jmap: FakeJmap) => jmap.calendar.sets.at(-1);

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
  addTaskFixtures(jmap.calendar);
});

test("views: today, upcoming, all, completed and a list", async ({
  page,
}, info) => {
  await page.goto("/tasks");
  await expect(
    page.getByRole("heading", { level: 1, name: "Today" })
  ).toBeVisible();
  // Overdue first, then today (a repeating task once).
  await expect(
    list(page).getByRole("heading", { name: /Overdue/u })
  ).toBeVisible();
  await expect(row(page, "Buy milk")).toBeVisible();
  await expect(row(page, "Weekly report")).toHaveCount(1);
  await expect(row(page, "Pay the invoice")).toBeVisible();
  await expect(row(page, "Read a book")).toHaveCount(0);
  // Accessible names: the tick says what it does; marks have words.
  await expect(
    list(page).getByRole("checkbox", { name: "Done: Buy milk" })
  ).toBeVisible();
  await expect(row(page, "Buy milk")).toHaveAccessibleName(
    /Has notes.*Overdue.*Priority: High.*In Errands/u
  );
  await shot(page, info, "today");
  await expectNoSidewaysScroll(page);

  await goTo(page, /^Upcoming/u);
  await expect(page).toHaveURL(/view=upcoming/u);
  await expect(
    list(page).getByRole("heading", { name: /Tomorrow/u })
  ).toBeVisible();
  // Each occurrence of the weekly task, on its day.
  await expect(row(page, "Weekly report")).toHaveCount(4);
  await shot(page, info, "upcoming");

  await goTo(page, /^All/u);
  await expect(row(page, "Read a book")).toBeVisible();
  // The subtask sits under its parent, with "1/2" done on the parent.
  await expect(row(page, "Plan the trip")).toHaveAccessibleName(
    /1 of 2 subtasks done/u
  );
  const parentLeft = await row(page, "Plan the trip").evaluate(
    (element) => element.getBoundingClientRect().left
  );
  const childLeft = await row(page, "Book flights").evaluate(
    (element) => element.getBoundingClientRect().left
  );
  expect(childLeft).toBeGreaterThan(parentLeft);
  await shot(page, info, "all");

  await goTo(page, /^Completed/u);
  await expect(row(page, "Call mom")).toBeVisible();
  await expect(row(page, "Pack the bags")).toBeVisible();
  // Done tasks are struck through.
  await expect(
    row(page, "Call mom").locator("span", { has: page.locator("bdi") })
  ).toHaveCSS("text-decoration-line", "line-through");
  await shot(page, info, "completed");

  await goTo(page, /^Errands/u);
  await expect(
    page.getByRole("heading", { level: 1, name: "Errands" })
  ).toBeVisible();
  await expect(row(page, "Buy milk")).toBeVisible();
  await expect(row(page, "Read a book")).toBeVisible();
  await expect(row(page, "Pay the invoice")).toHaveCount(0);
  await list(page).getByRole("button", { name: "Show 1 completed" }).click();
  await expect(row(page, "Call mom")).toBeVisible();
  await shot(page, info, "list");
  await expectNoSidewaysScroll(page);
});

test("quick add: Enter adds to today, Tab makes the next one a subtask", async ({
  page,
}) => {
  await page.goto("/tasks");
  const add = page.getByRole("textbox", { name: "New task" });
  await add.fill("Water the plants");
  await add.press("Enter");
  await expect
    .poll(() => lastSet(jmap)?.create)
    .toMatchObject({
      task: {
        "@type": "Task",
        calendarIds: { cal1: true },
        due: `${todayUtc()}T00:00:00`,
        progress: "needs-action",
        showWithoutTime: true,
        title: "Water the plants",
      },
    });
  await expect(row(page, "Water the plants")).toBeVisible();
  // The input stays; Tab: the next one hangs from the task just added.
  await expect(add).toHaveValue("");
  await add.press("Tab");
  const sub = page.getByRole("textbox", {
    name: "New subtask of Water the plants",
  });
  await expect(sub).toBeFocused();
  await sub.fill("Buy fertilizer");
  await sub.press("Enter");
  const created = jmap.calendar.events.find(
    (event) => event.title === "Water the plants"
  );
  await expect
    .poll(() => lastSet(jmap)?.create)
    .toMatchObject({
      task: {
        relatedTo: {
          [String(created?.uid)]: { relation: { parent: true } },
        },
        title: "Buy fertilizer",
      },
    });
  // Shift+Tab: back to the top.
  await sub.press("Shift+Tab");
  await expect(page.getByRole("textbox", { name: "New task" })).toBeFocused();
});

test("the panel saves every field by itself", async ({ page }, info) => {
  await page.goto("/tasks?view=all");
  await row(page, "Read a book").click();
  const detail = panel(page);
  await expect(detail.getByRole("textbox", { name: "Title" })).toHaveValue(
    "Read a book"
  );
  if (width(page) <= NARROW_MAX) {
    // The task takes the list's place.
    await expect(list(page)).toBeHidden();
  }
  // Title and notes: a moment after typing stops.
  await detail.getByRole("textbox", { name: "Title" }).fill("Read two books");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ readbook: { title: "Read two books" } });
  await detail.getByRole("textbox", { name: "Notes" }).fill("Fiction first");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      readbook: { description: "Fiction first", descriptionContentType: null },
    });
  // Priority, due date, reminders and repetition: at once.
  await detail.getByRole("radio", { name: "High" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ readbook: { priority: 1 } });
  // Estimated time (RFC 8984 §5.2.3): a Duration once focus leaves the
  // boxes, shown in the row; ✕ removes it with a null patch.
  await detail.getByRole("spinbutton", { name: "Hours" }).fill("1");
  await detail.getByRole("spinbutton", { name: "Minutes" }).fill("30");
  await detail.getByRole("spinbutton", { name: "Minutes" }).press("Enter");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ readbook: { estimatedDuration: "PT1H30M" } });
  if (width(page) > NARROW_MAX) {
    await expect(row(page, "Read two books")).toContainText("1 h 30 min");
  }
  await detail.getByRole("button", { name: "Remove estimated time" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ readbook: { estimatedDuration: null } });
  await detail.getByRole("button", { name: "Tomorrow" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({
      readbook: {
        due: expect.stringMatching(/T00:00:00$/u),
        showWithoutTime: true,
      },
    });
  await expect(
    detail.getByRole("button", { name: "Add a time" })
  ).toBeVisible();
  await detail.getByRole("button", { name: "Add a reminder" }).click();
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update))
    .toContain("AbsoluteTrigger");
  await chooseOption(
    detail.getByRole("combobox", { name: "Repeat" }),
    "weekly"
  );
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({
      readbook: {
        recurrenceRule: { frequency: "weekly" },
        start: expect.stringMatching(/T00:00:00$/u),
      },
    });
  // Another list (a calendar of the same account).
  await chooseOption(detail.getByRole("combobox", { name: "List" }), {
    label: "Stalwart Calendar (alice@nebulamail.test)",
  });
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ readbook: { calendarIds: { cal1: true } } });
  await shot(page, info, "panel");
  await expectNoSidewaysScroll(page);
  // Esc closes the panel.
  await detail.getByRole("textbox", { name: "Notes" }).press("Escape");
  await expect(detail).toHaveCount(0);
  await expect(list(page)).toBeVisible();
});

test("ticking a task completes it, with Undo", async ({ page }) => {
  await page.goto("/tasks");
  await list(page).getByRole("checkbox", { name: "Done: Buy milk" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({
      buymilk: {
        iCalendar: {
          name: "vtodo",
          properties: [
            [
              "completed",
              {},
              "date-time",
              expect.stringMatching(/^\d{8}T\d{6}Z$/u),
            ],
          ],
        },
        progress: "completed",
        progressUpdated: expect.stringMatching(
          /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/u
        ),
      },
    });
  // It stays (struck through) until the view changes.
  await expect(row(page, "Buy milk")).toBeVisible();
  await page.getByRole("button", { name: "Undo" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ buymilk: { iCalendar: null, progress: "needs-action" } });
});

test("completing one occurrence of a repeating task", async ({ page }) => {
  await page.goto("/tasks");
  await list(page)
    .getByRole("checkbox", { name: "Done: Weekly report" })
    .click();
  // Only today's occurrence (the series had no overrides: the whole map),
  // with its start (support.stalw.art/t/2007).
  const today = `${todayUtc()}T09:00:00`;
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      report: {
        recurrenceOverrides: {
          [today]: {
            progress: "completed",
            // RFC 8984 §5.2.6, in the occurrence's own override.
            progressUpdated: expect.stringMatching(
              /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/u
            ),
            start: today,
          },
        },
      },
    });
  await goTo(page, /^Upcoming/u);
  // The next ones are still to do.
  await expect(row(page, "Weekly report")).toHaveCount(4);
});

test("Tab and Shift+Tab make a task a subtask and back", async ({ page }) => {
  await page.goto(`/tasks?list=${ERRANDS_ID}`);
  await expect(row(page, "Read a book")).toBeVisible();
  await row(page, "Read a book").focus();
  await page.keyboard.press("Tab");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      readbook: {
        relatedTo: {
          "buy-milk@example.com": {
            "@type": "Relation",
            relation: { parent: true },
          },
        },
      },
    });
  // It moved under "Buy milk" and keeps the keyboard.
  await expect(row(page, "Read a book")).toBeFocused();
  await expect(row(page, "Buy milk")).toHaveAccessibleName(
    /0 of 1 subtasks done/u
  );
  await page.keyboard.press("Shift+Tab");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      readbook: { "relatedTo/buy-milk@example.com": null },
    });
  // At the top, Shift+Tab leaves the list as usual.
  await expect(row(page, "Buy milk")).not.toHaveAccessibleName(/subtasks/u);
});

test("a task's reminder: due time, Mark done", async ({ page }, info) => {
  await page.goto("/tasks");
  await expect(row(page, "Buy milk")).toBeVisible();
  jmap.calendar.alerts.push({
    "@type": "CalendarAlert",
    accountId: "a",
    alertId: "a1",
    calendarEventId: "report",
    recurrenceId: `${todayUtc()}T09:00:00`,
    uid: "weekly-report@example.com",
  });
  const reminder = page.getByRole("region", {
    name: "Task reminder: Weekly report",
  });
  await expect(reminder).toBeVisible({ timeout: 15_000 });
  await expect(reminder.getByText(/^Due /u)).toBeVisible();
  await shot(page, info, "reminder");
  await reminder.getByRole("button", { name: "Mark done" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({
      report: {
        "alerts/a1/acknowledged": expect.stringMatching(/Z$/u),
        recurrenceOverrides: {
          [`${todayUtc()}T09:00:00`]: { progress: "completed" },
        },
      },
    });
  await expect(reminder).toHaveCount(0);
  await expect(page.getByText("Done: Weekly report")).toBeVisible();
});

test("lists: create, rename and delete", async ({ page }, info) => {
  await page.goto("/tasks");
  if (isPhone(page)) {
    await page.getByRole("button", { name: "Tasks sidebar" }).click();
  }
  await page
    .getByRole("navigation", { name: "Tasks sidebar" })
    .getByRole("button", { name: "Add a list" })
    .click();
  await page.getByRole("menuitem", { name: "New list" }).click();
  const dialog = page.getByRole("dialog", { name: "New list" });
  await dialog.getByRole("textbox", { name: "Name" }).fill("Groceries");
  await shot(page, info, "new-list");
  await dialog.getByRole("button", { name: "Create list" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.create)
    .toMatchObject({ calendar: { name: "Groceries" } });
  // The new (empty) list opens, and stays a list (synced preferences).
  await expect(
    page.getByRole("heading", { level: 1, name: "Groceries" })
  ).toBeVisible();
  await expect(page.getByText("This list is empty")).toBeVisible();
  const add = page.getByRole("textbox", { name: "New task" });
  await add.fill("Apples");
  await add.press("Enter");
  await expect
    .poll(() => lastSet(jmap)?.create)
    .toMatchObject({ task: { calendarIds: { cal3: true }, title: "Apples" } });

  if (isPhone(page)) {
    await page.getByRole("button", { name: "Tasks sidebar" }).click();
  }
  const nav = page.getByRole("navigation", { name: "Tasks sidebar" });
  await nav.getByRole("button", { name: "Options for Groceries" }).click();
  await page.getByRole("menuitem", { name: "Name and colour…" }).click();
  const edit = page.getByRole("dialog", { name: "List name and colour" });
  await edit.getByRole("textbox", { name: "Name" }).fill("Shopping");
  await edit.getByRole("button", { name: "Save" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.update)
    .toMatchObject({ cal3: { name: "Shopping" } });

  if (isPhone(page)) {
    await page.getByRole("button", { name: "Tasks sidebar" }).click();
  }
  await nav.getByRole("button", { name: "Options for Shopping" }).click();
  await page.getByRole("menuitem", { name: "Delete list…" }).click();
  await page
    .getByRole("alertdialog", { name: "Delete “Shopping”?" })
    .getByRole("button", { name: "Delete list" })
    .click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.destroy)
    .toStrictEqual(["cal3"]);
  await expect(
    page.getByRole("heading", { level: 1, name: "Today" })
  ).toBeVisible();
});

test("lists: add an existing calendar as a list", async ({ page }, info) => {
  const [own] = jmap.calendar.calendars;
  if (own === undefined) {
    throw new Error("no calendar");
  }
  jmap.calendar.calendars.push({
    ...own,
    color: "#B42318",
    id: "birthdays",
    isDefault: false,
    name: "Birthdays",
  });
  await page.goto("/tasks");
  await expect(row(page, "Buy milk")).toBeVisible();
  if (isPhone(page)) {
    await page.getByRole("button", { name: "Tasks sidebar" }).click();
  }
  const nav = page.getByRole("navigation", { name: "Tasks sidebar" });
  // Only calendars with tasks are lists so far.
  await expect(nav.getByRole("button", { name: /^Birthdays/u })).toHaveCount(0);
  await nav.getByRole("button", { name: "Add a list" }).click();
  await page.getByRole("menuitem", { name: "Add a calendar…" }).click();
  const dialog = page.getByRole("dialog", {
    name: "Add a calendar as a list",
  });
  // Errands and the default calendar are lists already.
  await expect(dialog.getByRole("button", { name: /Errands/u })).toHaveCount(0);
  await shot(page, info, "add-calendar");
  await dialog.getByRole("button", { name: /^Birthdays/u }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByText("“Birthdays” added to Tasks")).toBeVisible();
  await expect(
    page.getByRole("heading", { level: 1, name: "Birthdays" })
  ).toBeVisible();
  await expect(page.getByText("This list is empty")).toBeVisible();
  // It stays a list (synced preferences) and takes new tasks.
  await page.reload();
  await goTo(page, /^Birthdays/u);
  const add = page.getByRole("textbox", { name: "New task" });
  await add.fill("Buy a present");
  await add.press("Enter");
  await expect
    .poll(() => lastSet(jmap)?.create)
    .toMatchObject({
      task: { calendarIds: { birthdays: true }, title: "Buy a present" },
    });
  // ⌘K offers it too: what's left is the calendar Bob shares.
  await page.keyboard.press("Control+k");
  await page.getByRole("option", { name: "Add a calendar…" }).click();
  await expect(dialog.getByRole("button", { name: /^(?!Close$)/u })).toHaveText(
    [/Team calendar.*Shared by bob@example\.com/u]
  );
});

test("lists: remove one from Tasks without deleting its calendar", async ({
  page,
}, info) => {
  await page.goto("/tasks");
  await goTo(page, /^Errands/u);
  await expect(row(page, "Buy milk")).toBeVisible();
  const sets = jmap.calendar.calendarSets.length;
  const nav = page.getByRole("navigation", { name: "Tasks sidebar" });
  const options = async () => {
    if (isPhone(page)) {
      await page.getByRole("button", { name: "Tasks sidebar" }).click();
    }
    await nav.getByRole("button", { name: "Options for Errands" }).click();
    await shot(page, info, "list-menu");
    await page.getByRole("menuitem", { name: "Remove from Tasks" }).click();
  };
  await options();
  const toast = page.getByText(
    "“Errands” removed from Tasks. The calendar stays."
  );
  await expect(toast).toBeVisible();
  // Back to Today, without the list nor its tasks (Buy milk is overdue).
  await expect(
    page.getByRole("heading", { level: 1, name: "Today" })
  ).toBeVisible();
  await expect(row(page, "Buy milk")).toHaveCount(0);
  await expect(row(page, "Pay the invoice")).toBeVisible();
  await expect(nav.getByRole("button", { name: /^Errands/u })).toHaveCount(0);
  // Undo brings it back.
  await page.getByRole("button", { name: "Undo" }).click();
  await expect(row(page, "Buy milk")).toBeVisible();

  await options();
  await expect(toast).toBeVisible();
  await expect(row(page, "Buy milk")).toHaveCount(0);
  // It stays out after reloading, though it has tasks (synced preferences).
  await page.reload();
  await expect(row(page, "Pay the invoice")).toBeVisible();
  await expect(row(page, "Buy milk")).toHaveCount(0);
  await goTo(page, /^All/u);
  await expect(row(page, "Read a book")).toHaveCount(0);
  // The calendar wasn't touched and is still in the calendar app.
  expect(jmap.calendar.calendarSets).toHaveLength(sets);
  expect(
    jmap.calendar.calendars.some((calendar) => calendar.id === ERRANDS_ID)
  ).toBeTruthy();
  // "Add a calendar…" offers it again.
  if (isPhone(page)) {
    await page.getByRole("button", { name: "Tasks sidebar" }).click();
  }
  await nav.getByRole("button", { name: "Add a list" }).click();
  await page.getByRole("menuitem", { name: "Add a calendar…" }).click();
  await page
    .getByRole("dialog", { name: "Add a calendar as a list" })
    .getByRole("button", { name: /^Errands/u })
    .click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Errands" })
  ).toBeVisible();
  await expect(row(page, "Buy milk")).toBeVisible();
});

test("lists: share one (no free/busy level), also from ⌘K", async ({
  page,
}) => {
  jmap.directory = true;
  await page.goto("/tasks");
  await goTo(page, /^Errands/u);
  await expect(
    page.getByRole("heading", { level: 1, name: "Errands" })
  ).toBeVisible();
  if (isPhone(page)) {
    await page.getByRole("button", { name: "Tasks sidebar" }).click();
  }
  const nav = page.getByRole("navigation", { name: "Tasks sidebar" });
  await nav.getByRole("button", { name: "Options for Errands" }).click();
  await page.getByRole("menuitem", { name: "Share…" }).click();
  const dialog = page.getByRole("dialog", { name: "Share “Errands”" });
  await expect(dialog.getByText("Only you can see this list.")).toBeVisible();
  expect(
    await optionNames(dialog.getByLabel("Access for people you add"))
  ).toStrictEqual([
    "Can view tasks",
    "Can view and edit",
    "Can edit and share",
  ]);
  await dialog.getByLabel("Search people by name or address").fill("eve");
  await dialog.getByRole("button", { exact: true, name: "Share" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.update)
    .toMatchObject({
      [ERRANDS_ID]: {
        "shareWith/pe": { mayReadItems: true, mayWriteAll: false },
      },
    });
  await expectValue(dialog.getByLabel("Access for Eve Núñez"), "read");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  // ⌘K: the open list's "Share…".
  await page.keyboard.press("Control+k");
  await page.getByRole("option", { name: /Share list “Errands”/u }).click();
  await expect(dialog.getByText("Eve Núñez")).toBeVisible();
});

test("phone: the drawer, the New task button and the task instead of the list", async ({
  page,
}, info) => {
  test.skip(!isPhone(page), "phone layout");
  await page.goto("/tasks");
  await expect(
    page.getByRole("navigation", { name: "Tasks sidebar" })
  ).toBeHidden();
  await page.getByRole("button", { name: "Tasks sidebar" }).click();
  await shot(page, info, "drawer");
  await page
    .getByRole("navigation", { name: "Tasks sidebar" })
    .getByRole("button", { name: /^All/u })
    .click();
  await expect(page).toHaveURL(/view=all/u);
  await page.getByRole("button", { name: "New task", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "New task" })).toBeFocused();
  await row(page, "Plan the trip").click();
  await expect(list(page)).toBeHidden();
  await expect(
    page.getByRole("button", { name: "New task", exact: true })
  ).toBeHidden();
  await shot(page, info, "phone-panel");
  await expectNoSidewaysScroll(page);
  await panel(page).getByRole("button", { name: "Back to the list" }).click();
  await expect(list(page)).toBeVisible();
});

test("tasks in the calendar, with their tick and a switch", async ({
  page,
}, info) => {
  await page.goto("/calendar?view=agenda");
  const open = page.getByRole("button", { name: "Task: Pay the invoice" });
  await expect(open).toBeVisible();
  // Not as an event.
  await expect(
    page.getByRole("button", { name: /^Pay the invoice/u })
  ).toHaveCount(0);
  await page.getByRole("checkbox", { name: "Done: Pay the invoice" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({ task1: { progress: "completed" } });
  await shot(page, info, "calendar-agenda");
  if (!isPhone(page)) {
    // On the task's date: tomorrow may already be in the next week or month.
    const due = tomorrowUtc();
    await page.goto(`/calendar?view=week&date=${due}`);
    await expect(
      page.getByRole("button", { name: "Task: Plan the trip" })
    ).toBeVisible();
    await shot(page, info, "calendar-week");
    await page.goto(`/calendar?view=month&date=${due}`);
    await expect(
      page.getByRole("button", { name: "Task: Plan the trip" })
    ).toBeVisible();
    await shot(page, info, "calendar-month");
    // "Show tasks" is per device, in the preferences cookie (the server
    // renders the calendar as chosen, lib/use-layout.ts).
    await page.getByRole("switch", { name: "Show tasks" }).click();
    await expect(
      page.getByRole("button", { name: "Task: Plan the trip" })
    ).toHaveCount(0);
    const cookies = await page.context().cookies();
    const prefs = cookies.find((cookie) => cookie.name === "wm-prefs");
    expect(prefs?.value).toMatch(/\.hidden$/u);
    await page.reload();
    await expect(
      page.getByRole("switch", { name: "Show tasks" })
    ).not.toBeChecked();
    await page.getByRole("switch", { name: "Show tasks" }).click();
  }
  await page.goto("/calendar?view=agenda");
  await page.getByRole("button", { name: "Task: Plan the trip" }).click();
  await expect(page).toHaveURL(/\/tasks\?task=plantrip/u);
  await expect(panel(page).getByRole("textbox", { name: "Title" })).toHaveValue(
    "Plan the trip"
  );
});

test("a task from an email links back to it", async ({ page }, info) => {
  await page.goto("/mail/inbox/e1");
  await expect(
    page.getByRole("heading", { name: /Quarterly report review/u })
  ).toBeVisible();
  // A key pressed before the page has hydrated does nothing: press until
  // the task is created.
  await expect(async () => {
    await page.keyboard.press("Shift+T");
    await expect
      .poll(() => lastSet(jmap)?.create, { timeout: 1000 })
      .toBeDefined();
  }).toPass();
  await expect
    .poll(() => lastSet(jmap)?.create)
    .toMatchObject({
      task: {
        description: expect.stringMatching(
          /^From: Carla Gómez <carla@example\.com> · /u
        ),
        links: {
          l1: { href: "mid:e1@example.com" },
          l2: { href: expect.stringMatching(/\/mail\/inbox\/e1$/u) },
        },
        title:
          "Quarterly report review before the Friday meeting with the board",
      },
    });
  await page.getByRole("button", { name: "Open", exact: true }).click();
  await expect(page).toHaveURL(/\/tasks\?task=/u);
  const detail = panel(page);
  await expect(detail.getByText("Message-ID: e1@example.com")).toBeVisible();
  await shot(page, info, "from-email");
  await detail.getByRole("button", { name: /Open the email/u }).click();
  await expect(page).toHaveURL(/\/mail\/inbox\/e1$/u);
  // Also in the reader's menu and ⌘K.
  await page.getByRole("button", { name: "More actions" }).first().click();
  await expect(
    page.getByRole("menuitem", { name: /Create task/u })
  ).toBeVisible();
});

test("keyboard: move, tick, priority, due, list, delete, search", async ({
  page,
}) => {
  test.skip(
    isPhone(page),
    "a keyboard on a phone is rare; the desktop covers it"
  );
  await page.goto("/tasks?view=all");
  await expect(row(page, "Buy milk")).toBeVisible();
  await page.keyboard.press("j");
  await expect(row(page, "Buy milk")).toBeFocused();
  await page.keyboard.press("j");
  await expect(row(page, "Weekly report")).toBeFocused();
  await page.keyboard.press("k");
  await expect(row(page, "Buy milk")).toBeFocused();
  // ! then 0–3: priority.
  await page.keyboard.press("!");
  await page.keyboard.press("2");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ buymilk: { priority: 5 } });
  // d: due date.
  await page.keyboard.press("d");
  await page
    .getByRole("dialog", { name: "Due date" })
    .getByRole("button", { name: "No date" })
    .click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ buymilk: { due: null } });
  await expect(page.getByRole("dialog", { name: "Due date" })).toHaveCount(0);
  // v: move to another list.
  await row(page, "Buy milk").focus();
  await page.keyboard.press("v");
  await page
    .getByRole("dialog", { name: "Move to list" })
    .getByRole("button", { name: /Stalwart Calendar/u })
    .click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ buymilk: { calendarIds: { cal1: true } } });
  await expect(page.getByRole("dialog", { name: "Move to list" })).toHaveCount(
    0
  );
  // x: done (Space too).
  await row(page, "Pay the invoice").focus();
  await page.keyboard.press("x");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({ task1: { progress: "completed" } });
  // e: edit the title in the panel.
  await row(page, "Read a book").focus();
  await page.keyboard.press("e");
  await expect(
    panel(page).getByRole("textbox", { name: "Title" })
  ).toBeFocused();
  await page.keyboard.press("Escape");
  // #: delete, with Undo.
  await row(page, "Read a book").focus();
  await page.keyboard.press("#");
  await expect.poll(() => lastSet(jmap)?.destroy).toStrictEqual(["readbook"]);
  await expect(row(page, "Read a book")).toHaveCount(0);
  await expect(page.getByText("Deleted: Read a book")).toBeVisible();
  // This toast's Undo (the tick's toast may still be there, stacked).
  await page
    .getByRole("dialog")
    .filter({
      has: page.getByRole("heading", { name: "Deleted: Read a book" }),
    })
    .getByRole("button", { name: "Undo" })
    .click();
  await expect
    .poll(() => JSON.stringify(jmap.calendar.sets.slice(-2)))
    .toContain("undo-readbook");
  await expect(page.getByText("Deleted: Read a book")).toHaveCount(0);
  // /: search; c: new task; ?: the help.
  await page.keyboard.press("/");
  await page.keyboard.type("trip");
  await expect(row(page, "Plan the trip")).toBeVisible();
  await expect(row(page, "Weekly report")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await page.keyboard.press("c");
  await expect(page.getByRole("textbox", { name: "New task" })).toBeFocused();
  await page.keyboard.press("Escape");
  await page.keyboard.press("?");
  const help = page.getByRole("dialog", { name: "Tasks shortcuts" });
  await expect(help.getByText("Make it a subtask")).toBeVisible();
  await page.keyboard.press("Escape");
  // ⌘K lists the views and lists.
  await page.keyboard.press("Control+k");
  await expect(page.getByRole("option", { name: /Errands/u })).toBeVisible();
  await page.keyboard.press("Escape");
  // g k goes to Tasks from the mail.
  await page.goto("/mail/inbox");
  await expect(
    page.getByText(/Quarterly report review/u).first()
  ).toBeVisible();
  await page.keyboard.press("g");
  await page.keyboard.press("k");
  await expect(page).toHaveURL(/\/tasks$/u);
});

test("a list shared read-only: its tasks can't be changed", async ({
  page,
}) => {
  const [shared] = jmap.calendar.sharedCalendars;
  if (shared !== undefined) {
    Object.assign(shared.myRights, { mayWriteAll: false, mayWriteOwn: false });
  }
  jmap.calendar.sharedEvents.push({
    "@type": "Task",
    calendarIds: { [SHARED_CALENDAR_ID]: true },
    id: "bobtask",
    progress: "needs-action",
    title: "Bob's review",
    uid: "bob-review-task@example.com",
  });
  await page.goto("/tasks?view=all");
  await expect(row(page, "Bob's review")).toBeVisible();
  await expect(
    list(page).getByRole("checkbox", { name: "Done: Bob's review" })
  ).toBeDisabled();
  await row(page, "Bob's review").click();
  await expect(
    panel(page).getByText(/shares with you \(read only\)/u)
  ).toBeVisible();
  await expect(
    panel(page).getByRole("textbox", { name: "Title" })
  ).not.toBeEditable();
  expect(SHARED_CALENDAR_ACCOUNT).toBe("b");
});

test("a day task's reminders ring at a time of day", async ({ page }, info) => {
  const tomorrow = tomorrowUtc();
  await page.goto("/tasks?task=plantrip");
  const detail = panel(page);
  await expect(detail.getByRole("textbox", { name: "Title" })).toHaveValue(
    "Plan the trip"
  );
  // "Add a reminder": on the day, at 9:00 (an absolute time: every client
  // rings it).
  await detail.getByRole("button", { name: "Add a reminder" }).click();
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update))
    .toContain(`"when":"${tomorrow}T09:00:00Z"`);
  const reminder = detail.getByRole("group", { name: /On the day at 9:00/u });
  await expect(reminder).toBeVisible();
  // The day before, at 6:30 (the next reminders start from that time).
  await chooseOption(reminder.getByRole("combobox", { name: "Reminder day" }), {
    label: "1 day before",
  });
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update))
    .toContain(`"when":"${todayUtc()}T09:00:00Z"`);
  await expect(
    detail.getByRole("group", { name: /1 day before at 9:00/u })
  ).toBeVisible();
  const hour = detail.getByRole("spinbutton", { name: /hour/iu }).last();
  await hour.click();
  // (12-hour clock in en-US: 6 stays AM.)
  await hour.pressSequentially("6");
  const minute = detail.getByRole("spinbutton", { name: /minute/iu }).last();
  await minute.click();
  await minute.pressSequentially("30");
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update))
    .toContain(`"when":"${todayUtc()}T06:30:00Z"`);
  await expect(
    detail.getByRole("group", { name: /1 day before at 6:30/u })
  ).toBeVisible();
  await detail.getByRole("button", { name: "Add a reminder" }).click();
  await expect(
    detail.getByRole("group", { name: /On the day at 6:30/u })
  ).toBeVisible();
  await shot(page, info, "day-reminders");
  await expectNoSidewaysScroll(page);
});

test("a day task's reminder card says when it was set for", async ({
  page,
}) => {
  const task = jmap.calendar.events.find((event) => event.id === "buymilk");
  if (task !== undefined) {
    task.alerts = {
      a9: {
        "@type": "Alert",
        action: "display",
        trigger: {
          "@type": "AbsoluteTrigger",
          when: `${todayUtc()}T09:00:00Z`,
        },
      },
    };
  }
  await page.goto("/tasks");
  await expect(row(page, "Buy milk")).toBeVisible();
  jmap.calendar.alerts.push({
    "@type": "CalendarAlert",
    accountId: "a",
    alertId: "a9",
    calendarEventId: "buymilk",
    uid: "buy-milk@example.com",
  });
  const card = page.getByRole("region", { name: "Task reminder: Buy milk" });
  await expect(card).toBeVisible({ timeout: 15_000 });
  await expect(card.getByText(/^Due .*reminder at 9:00/u)).toBeVisible();
});

test("manual order: Alt+↑/↓ and dragging, with Undo", async ({
  page,
}, info) => {
  await page.goto(`/tasks?list=${ERRANDS_ID}`);
  const rows = list(page).locator("[data-task-id]");
  await expect(rows).toHaveCount(2);
  await expect(rows.first()).toContainText("Buy milk");
  // Alt+↑ on the second: no order yet, so the level is numbered.
  await row(page, "Read a book").focus();
  await page.keyboard.press("Alt+ArrowUp");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      buymilk: {
        iCalendar: {
          name: "vtodo",
          properties: [["x-apple-sort-order", {}, "integer", 2048]],
        },
      },
      readbook: {
        iCalendar: {
          name: "vtodo",
          properties: [["x-apple-sort-order", {}, "integer", 1024]],
        },
      },
    });
  await expect(rows.first()).toContainText("Read a book");
  await expect(row(page, "Read a book")).toBeFocused();
  // Undo puts it back.
  await page.getByRole("button", { name: "Undo" }).first().click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      buymilk: { iCalendar: null },
      readbook: { iCalendar: null },
    });
  await expect(rows.first()).toContainText("Buy milk");
  if (isPhone(page) || page.viewportSize() === null) {
    return;
  }
  test.skip(
    info.project.name !== "desktop",
    "dragging with a mouse (a finger rests first: same code)"
  );
  // Drag "Buy milk" below "Read a book".
  const from = await row(page, "Buy milk").boundingBox();
  const to = await row(page, "Read a book").boundingBox();
  if (from === null || to === null) {
    throw new Error("rows not on screen");
  }
  await page.mouse.move(from.x + 40, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + 40, to.y + to.height - 2, { steps: 8 });
  await shot(page, info, "drag-reorder");
  await page.mouse.up();
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update))
    .toContain("x-apple-sort-order");
  await expect(rows.first()).toContainText("Read a book");
  // The drop isn't a click: no task opened.
  await expect(panel(page)).toHaveCount(0);
});

test("⌘K makes the focused task a subtask of the one above and back", async ({
  page,
}) => {
  await page.goto(`/tasks?list=${ERRANDS_ID}`);
  await row(page, "Read a book").focus();
  await page.keyboard.press("Control+k");
  await page
    .getByRole("option", { name: "Make it a subtask of “Buy milk”" })
    .click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      readbook: {
        relatedTo: {
          "buy-milk@example.com": {
            "@type": "Relation",
            relation: { parent: true },
          },
        },
      },
    });
  await expect(row(page, "Buy milk")).toHaveAccessibleName(
    /0 of 1 subtasks done/u
  );
  await row(page, "Read a book").focus();
  await page.keyboard.press("Control+k");
  await page
    .getByRole("option", { name: "Move “Read a book” up a level" })
    .click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      readbook: { "relatedTo/buy-milk@example.com": null },
    });
  await expect(row(page, "Buy milk")).not.toHaveAccessibleName(/subtasks/u);
});

test("dropping a task on another makes it a subtask; dragging it out undoes it", async ({
  page,
}, info) => {
  test.skip(
    info.project.name !== "desktop",
    "dragging with a mouse (a finger rests first: same code)"
  );
  await page.goto(`/tasks?list=${ERRANDS_ID}`);
  const milk = list(page).locator('[data-task-id="buymilk"]');
  await expect(milk).toBeVisible();
  // "Read a book" onto the middle of "Buy milk".
  const from = await boxOf(row(page, "Read a book"));
  const onto = await boxOf(milk);
  await page.mouse.move(from.x + 40, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + 40, onto.y + onto.height / 2, { steps: 8 });
  await expect(page.getByText("Subtask of Buy milk")).toBeVisible();
  await shot(page, info, "drag-nest");
  await page.mouse.up();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      readbook: {
        relatedTo: {
          "buy-milk@example.com": {
            "@type": "Relation",
            relation: { parent: true },
          },
        },
      },
    });
  await expect(row(page, "Buy milk")).toHaveAccessibleName(
    /0 of 1 subtasks done/u
  );
  await expect(
    page.getByText("“Read a book” is now a subtask of “Buy milk”")
  ).toBeVisible();
  // Undo takes it out again.
  await page.getByRole("button", { name: "Undo" }).first().click();
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update))
    .toBe(JSON.stringify({ readbook: { relatedTo: null } }));
  await expect(row(page, "Buy milk")).not.toHaveAccessibleName(/subtasks/u);

  // A subtask dragged onto the top edge of a top task leaves its parent
  // and goes before it.
  await row(page, "Read a book").focus();
  await page.keyboard.press("Tab");
  await expect(row(page, "Buy milk")).toHaveAccessibleName(
    /0 of 1 subtasks done/u
  );
  const child = await boxOf(row(page, "Read a book"));
  const top = await boxOf(milk);
  await page.mouse.move(child.x + 60, child.y + child.height / 2);
  await page.mouse.down();
  await page.mouse.move(child.x + 60, top.y + 3, { steps: 8 });
  await page.mouse.up();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({
      buymilk: {
        iCalendar: {
          name: "vtodo",
          properties: [["x-apple-sort-order", {}, "integer", 2048]],
        },
      },
      readbook: {
        iCalendar: {
          name: "vtodo",
          properties: [["x-apple-sort-order", {}, "integer", 1024]],
        },
        "relatedTo/buy-milk@example.com": null,
      },
    });
  const rows = list(page).locator("[data-task-id]");
  await expect(rows.first()).toContainText("Read a book");
  await expect(row(page, "Buy milk")).not.toHaveAccessibleName(/subtasks/u);
});

test("dragging a task onto a list of the sidebar moves it there", async ({
  page,
}, info) => {
  test.skip(info.project.name !== "desktop", "the sidebar beside the list");
  await page.goto(`/tasks?list=${ERRANDS_ID}`);
  const from = await row(page, "Read a book").boundingBox();
  const target = await page
    .getByRole("navigation", { name: "Tasks sidebar" })
    .getByRole("button", { name: /^Stalwart Calendar/u })
    .boundingBox();
  if (from === null || target === null) {
    throw new Error("not on screen");
  }
  await page.mouse.move(from.x + 40, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(target.x + 30, target.y + target.height / 2, {
    steps: 10,
  });
  await expect(page.getByText(/Drop on Stalwart Calendar/u)).toBeVisible();
  await shot(page, info, "drag-to-list");
  await page.mouse.up();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toStrictEqual({ readbook: { calendarIds: { cal1: true } } });
  await expect(
    page.getByText(/Moved “Read a book” to Stalwart/u)
  ).toBeVisible();
  await expect(row(page, "Read a book")).toHaveCount(0);
});

test("moving a task in the calendar: dragging and Alt + arrows", async ({
  page,
}, info) => {
  test.skip(isPhone(page), "the day view's all-day row on a phone");
  const tomorrow = tomorrowUtc();
  const after = new Date(Date.now() + 2 * 86_400_000)
    .toISOString()
    .slice(0, 10);
  await page.goto(`/calendar?view=day&date=${tomorrow}`);
  const chip = page.getByRole("button", { name: "Task: Plan the trip" });
  await expect(chip).toBeVisible();
  // Alt + → moves the due a day (the keyboard way).
  await chip.focus();
  await page.keyboard.press("Alt+ArrowRight");
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({ plantrip: { due: `${after}T00:00:00` } });
  await expect(page.getByText(/“Plan the trip”: due/u)).toBeVisible();
  await page.getByRole("button", { name: "Undo" }).first().click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({ plantrip: { due: `${tomorrow}T00:00:00` } });
  test.skip(info.project.name !== "desktop", "dragging with a mouse");
  // Drag it from the all-day row to 10:00.
  await expect(chip).toBeVisible();
  const column = page.locator(
    `[data-drop-kind="time"][data-drop-date="${tomorrow}"]`
  );
  const box = await column.boundingBox();
  const start = await chip.boundingBox();
  if (box === null || start === null) {
    throw new Error("not on screen");
  }
  await column.evaluate((element) => {
    element.scrollIntoView({ block: "start" });
  });
  await page.mouse.move(start.x + 30, start.y + start.height / 2);
  await page.mouse.down();
  const columnNow = await column.boundingBox();
  if (columnNow === null) {
    throw new Error("no column");
  }
  // 10:00 is 10/24 of the day's height (plus where the chip was held).
  const y = columnNow.y + (columnNow.height * 10) / 24 + start.height / 2;
  await page.mouse.move(columnNow.x + 40, y, { steps: 10 });
  await shot(page, info, "calendar-drag");
  await page.mouse.up();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({
      plantrip: {
        due: `${tomorrow}T10:00:00`,
        showWithoutTime: false,
      },
    });
});

// ---- Assigned tasks -------------------

/** The text of the last email the app sent (an iTIP REPLY). */
const lastEmailText = (): string =>
  JSON.stringify(jmap.calendar.emails.at(-1)?.bodyValues ?? {});

test("assign a task: people with their status, sent with scheduling", async ({
  page,
}, info) => {
  await page.goto(`/tasks?view=all&task=${TASK_ID}`);
  const field = panel(page).getByLabel("Assign to");
  await field.fill("bob@example.com");
  await field.press("Enter");
  await expect
    .poll(() => lastSet(jmap))
    .toMatchObject({
      sendSchedulingMessages: true,
      update: {
        [TASK_ID]: { organizerCalendarAddress: "mailto:jane@example.com" },
      },
    });
  expect(JSON.stringify(lastSet(jmap))).toContain("mailto:bob@example.com");
  const people = panel(page).getByRole("list", { name: "Assigned to" });
  await expect(people.getByText("No answer yet")).toBeVisible();
  // A typo is kept in the field and said.
  await field.fill("not-an-address");
  await field.press("Enter");
  await expect(panel(page).getByRole("alert")).toContainText(
    "not an email address"
  );
  await shot(page, info, "assign");
  // Taking it back: no participants, still with scheduling (a CANCEL).
  await people.getByRole("button", { name: "Remove bob@example.com" }).click();
  await expect
    .poll(() => lastSet(jmap))
    .toMatchObject({
      sendSchedulingMessages: true,
      update: { [TASK_ID]: { participants: null } },
    });
});

test("a task assigned to me: completing it mails a REPLY", async ({ page }) => {
  jmap.calendar.events.push(assignedTask());
  await page.goto("/tasks?view=all&task=assigned1");
  await expect(panel(page).getByText("Assigned to you by Carol")).toBeVisible();
  // Nobody can be added by an assignee.
  await expect(panel(page).getByLabel("Assign to")).toHaveCount(0);
  await panel(page)
    .getByRole("checkbox", { name: "Done: Review the contract" })
    .click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({
      assigned1: {
        "participants/j": { progress: "completed" },
        progress: "completed",
      },
    });
  // The app mails the answer itself, not the server.
  expect(lastSet(jmap)).not.toHaveProperty("sendSchedulingMessages");
  await expect.poll(lastEmailText).toContain("PARTSTAT=COMPLETED");
  expect(lastEmailText()).toContain("BEGIN:VTODO");
  expect(jmap.calendar.emails.at(-1)).toMatchObject({
    subject: "Task done: Review the contract",
    to: [{ email: "carol@partner.example" }],
  });
  // Declining.
  await panel(page).getByRole("button", { name: "Decline" }).click();
  await expect.poll(lastEmailText).toContain("PARTSTAT=DECLINED");
});

test("a reminder of a task assigned to me: Mark done mails a REPLY", async ({
  page,
}) => {
  jmap.calendar.events.push({
    ...assignedTask(),
    alerts: {
      a1: {
        "@type": "Alert",
        action: "display",
        trigger: { "@type": "OffsetTrigger", offset: "-PT15M" },
      },
    },
  });
  await page.goto("/tasks?view=all");
  await expect(row(page, "Review the contract")).toBeVisible();
  jmap.calendar.alerts.push({
    "@type": "CalendarAlert",
    accountId: "a",
    alertId: "a1",
    calendarEventId: "assigned1",
    recurrenceId: null,
    uid: assignedTask().uid,
  });
  const reminder = page.getByRole("region", {
    name: "Task reminder: Review the contract",
  });
  await expect(reminder).toBeVisible({ timeout: 15_000 });
  await reminder.getByRole("button", { name: "Mark done" }).click();
  await expect
    .poll(() => lastSet(jmap)?.update)
    .toMatchObject({
      assigned1: {
        "participants/j": { progress: "completed" },
        progress: "completed",
      },
    });
  expect(lastSet(jmap)).not.toHaveProperty("sendSchedulingMessages");
  await expect.poll(lastEmailText).toContain("PARTSTAT=COMPLETED");
  expect(jmap.calendar.emails.at(-1)).toMatchObject({
    to: [{ email: "carol@partner.example" }],
  });
});

test("a task I assigned shows who has it in the list", async ({ page }) => {
  await page.goto(`/tasks?view=all&task=${TASK_ID}`);
  const field = panel(page).getByLabel("Assign to");
  await field.fill("dana@example.com");
  await field.press("Enter");
  const people = panel(page).getByRole("list", { name: "Assigned to" });
  await expect(people.getByText("No answer yet")).toBeVisible();
  // On a phone the panel covers the list.
  await page.goto("/tasks?view=all");
  await expect(row(page, TASK_TITLE)).toContainText("dana@example.com");
  await expect(row(page, TASK_TITLE)).toHaveAccessibleName(
    /Assigned to dana@example\.com: No answer yet/u
  );
});

test("an assigned task in an email: nothing is added until accepted", async ({
  page,
}, info) => {
  jmap.emails.unshift({
    calendarBlob: TASK_ASSIGNMENT_BLOB,
    from: { email: "carol@partner.example", name: "Carol" },
    html: "<p>Carol assigned you a task.</p>",
    id: "tsk1",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: new Date().toISOString(),
    subject: "Task assigned: Translate the brochure",
  });
  await page.goto("/mail/inbox");
  await page.getByText("Task assigned: Translate the brochure").first().click();
  const card = page.getByRole("region", { name: "Assigned task" });
  await expect(
    card.getByRole("heading", { name: "Translate the brochure" })
  ).toBeVisible();
  await expect(card.getByText("Carol assigned you a task")).toBeVisible();
  await expect(card.getByText(/^Due: /u)).toBeVisible();
  // Not shown as an event invitation.
  await expect(
    page.getByRole("region", { name: "Calendar invitation" })
  ).toHaveCount(0);
  await shot(page, info, "assignment-card");
  const before = jmap.calendar.sets.length;
  expect(
    jmap.calendar.sets
      .slice(0, before)
      .some((set) => JSON.stringify(set).includes(TASK_ASSIGNMENT_UID))
  ).toBe(false);

  await card.getByRole("button", { name: "Accept" }).click();
  await expect
    .poll(() => lastSet(jmap))
    .toMatchObject({
      create: {
        task: { "@type": "Task", uid: TASK_ASSIGNMENT_UID },
      },
    });
  // Only the task's data: none of the sender's alerts.
  expect(JSON.stringify(lastSet(jmap))).toContain(
    '"participationStatus":"accepted"'
  );
  await expect.poll(lastEmailText).toContain("PARTSTAT=ACCEPTED");
  expect(lastEmailText()).toContain("BEGIN:VTODO");
  await expect(page.getByText("Task added to your tasks")).toBeVisible();
});

test("a task assigned to me: proposing another due mails a COUNTER", async ({
  page,
}, info) => {
  jmap.calendar.events.push(assignedTask());
  await page.goto("/tasks?view=all&task=assigned1");
  const sets = jmap.calendar.sets.length;
  await panel(page)
    .getByRole("button", { name: "Propose another due date or pass it on" })
    .click();
  await panel(page)
    .getByLabel("Note for Carol")
    .fill("Can it wait until Monday?");
  await shot(page, info, "assigned-more");
  await panel(page).getByRole("button", { name: "Send proposal" }).click();
  await expect(page.getByText("Proposal sent to Carol")).toBeVisible();
  const counter = jmap.calendar.emails.at(-1);
  const ics = JSON.stringify(counter?.bodyValues ?? {});
  expect(ics).toContain("METHOD:COUNTER");
  expect(ics).toContain("BEGIN:VTODO");
  // No due yet: today is proposed, as a day.
  expect(ics).toContain(`DUE;VALUE=DATE:${todayUtc().replaceAll("-", "")}`);
  expect(ics).toContain("COMMENT:Can it wait until Monday?");
  expect(counter).toMatchObject({
    subject: "New due date proposed: Review the contract",
    to: [{ email: "carol@partner.example" }],
  });
  // The task itself doesn't change: the organizer decides.
  expect(jmap.calendar.sets).toHaveLength(sets);
});

test("a task assigned to me: passing it on mails the organizer and the delegate", async ({
  page,
}) => {
  jmap.calendar.events.push(assignedTask());
  await page.goto("/tasks?view=all&task=assigned1");
  await panel(page)
    .getByRole("button", { name: "Propose another due date or pass it on" })
    .click();
  const field = panel(page).getByLabel("Pass it on to");
  await field.fill("dana@example.com");
  await field.press("Enter");
  await panel(page).getByRole("button", { name: "Pass on" }).click();
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update ?? {}))
    .toContain('"participationStatus":"delegated"');
  // The user's copy, without scheduling: the app mails both messages.
  expect(lastSet(jmap)).not.toHaveProperty("sendSchedulingMessages");
  expect(JSON.stringify(lastSet(jmap))).toContain(
    '"delegatedFrom":{"mailto:jane@example.com":true}'
  );
  await expect(
    page.getByText("Task passed on to dana@example.com")
  ).toBeVisible();
  const [reply, request] = jmap.calendar.emails.slice(-2);
  const replyIcs = JSON.stringify(reply?.bodyValues ?? {});
  expect(reply).toMatchObject({
    subject: "Task passed on: Review the contract",
    to: [{ email: "carol@partner.example" }],
  });
  expect(replyIcs).toContain("METHOD:REPLY");
  expect(replyIcs.replaceAll("\\r\\n ", "")).toContain(
    'PARTSTAT=DELEGATED;DELEGATED-TO=\\"mailto:dana@example.com\\"'
  );
  const requestIcs = JSON.stringify(request?.bodyValues ?? {});
  expect(request).toMatchObject({
    subject: "Task assigned: Review the contract",
    to: [{ email: "dana@example.com" }],
  });
  expect(requestIcs).toContain("METHOD:REQUEST");
  expect(requestIcs).toContain("BEGIN:VTODO");
  expect(requestIcs.replaceAll("\\r\\n ", "")).toContain(
    'DELEGATED-FROM=\\"mailto:jane@example.com\\"'
  );
  // Passed on: no more answers from here.
  await expect(
    panel(page).getByText("You passed it on to dana@example.com")
  ).toBeVisible();
  await expect(panel(page).getByRole("button", { name: "Accept" })).toHaveCount(
    0
  );
});

test("a repeating task assigned to me: passing it on sends the whole series", async ({
  page,
}, info) => {
  jmap.calendar.events.push(assignedSeries());
  await page.goto("/tasks?view=all");
  await row(page, "Weekly figures").first().click();
  await panel(page)
    .getByRole("button", { name: "Propose another due date or pass it on" })
    .click();
  // Said before passing it on: the whole series goes, not the reminders.
  await expect(
    panel(page).getByText(/^It repeats: they get the whole series/u)
  ).toBeVisible();
  await shot(page, info, "delegate-series");
  const field = panel(page).getByLabel("Pass it on to");
  await field.fill("dana@example.com");
  await field.press("Enter");
  await panel(page).getByRole("button", { name: "Pass on" }).click();
  // The series itself (not an occurrence), without scheduling.
  await expect
    .poll(() => JSON.stringify(lastSet(jmap)?.update ?? {}))
    .toContain('"participationStatus":"delegated"');
  expect(Object.keys(lastSet(jmap)?.update ?? {})).toStrictEqual(["assigned2"]);
  expect(lastSet(jmap)).not.toHaveProperty("sendSchedulingMessages");
  await expect(
    page.getByText("Task passed on to dana@example.com")
  ).toBeVisible();
  const [reply, request] = jmap.calendar.emails.slice(-2);
  const replyIcs = JSON.stringify(reply?.bodyValues ?? {}).replaceAll(
    "\\r\\n ",
    ""
  );
  // The REPLY is about the whole series: no RECURRENCE-ID.
  expect(replyIcs).toContain("PARTSTAT=DELEGATED");
  expect(replyIcs).not.toContain("RECURRENCE-ID");
  const ics = JSON.stringify(request?.bodyValues ?? {}).replaceAll(
    "\\r\\n ",
    ""
  );
  expect(request).toMatchObject({ to: [{ email: "dana@example.com" }] });
  // The rule, its start and due in Madrid time with Madrid's VTIMEZONE,
  // and the Friday taken out.
  expect(ics).toContain("BEGIN:VTIMEZONE\\r\\nTZID:Europe/Madrid");
  expect(ics).toContain("DTSTART;TZID=Europe/Madrid:20310307T090000");
  expect(ics).toContain("DUE;TZID=Europe/Madrid:20310307T090000");
  expect(ics).toContain("RRULE:FREQ=WEEKLY;BYDAY=FR");
  expect(ics).toContain("EXDATE;TZID=Europe/Madrid:20310314T090000");
  // Summer time from the last Sunday of March 2031 (02:00 → 03:00).
  expect(ics).toContain("BEGIN:DAYLIGHT\\r\\nDTSTART:20310330T020000");
  expect(ics).not.toContain("VALARM");
});

test("a repeating task passed on to me: Accept adds the series", async ({
  page,
}, info) => {
  jmap.emails.unshift({
    calendarBlob: TASK_SERIES_BLOB,
    from: { email: "bob@example.com", name: "Bob" },
    html: "<p>Bob passed you a task.</p>",
    id: "tsk3",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: new Date().toISOString(),
    subject: "Task assigned: Weekly figures",
  });
  await page.goto("/mail/inbox");
  await page.getByText("Task assigned: Weekly figures").first().click();
  const card = page.getByRole("region", { name: "Assigned task" });
  await expect(
    card.getByRole("heading", { name: "Weekly figures" })
  ).toBeVisible();
  await expect(card.getByText(/^Due: .*2031/u)).toBeVisible();
  await expect(card.getByText(/^Repeats: /u)).toBeVisible();
  await shot(page, info, "series-card");
  await card.getByRole("button", { name: "Accept" }).click();
  await expect
    .poll(() => lastSet(jmap))
    .toMatchObject({
      create: {
        task: {
          "@type": "Task",
          due: "2031-03-07T09:00:00",
          recurrenceOverrides: {
            "2031-03-14T09:00:00": { excluded: true },
            "2031-03-21T09:00:00": {
              due: "2031-03-22T12:00:00",
              start: "2031-03-22T12:00:00",
            },
          },
          recurrenceRule: { byDay: [{ day: "fr" }], frequency: "weekly" },
          start: "2031-03-07T09:00:00",
          timeZone: "Europe/Madrid",
          uid: TASK_SERIES_UID,
        },
      },
    });
  // None of the sender's alerts, in the series or an occurrence.
  expect(JSON.stringify(lastSet(jmap))).not.toContain("alerts");
  await expect.poll(lastEmailText).toContain("PARTSTAT=ACCEPTED");
  expect(jmap.calendar.emails.at(-1)).toMatchObject({
    to: [{ email: "carol@partner.example" }],
  });
  await expect(page.getByText("Task added to your tasks")).toBeVisible();
});

test("a proposed due in an email: keep it or accept it", async ({
  page,
}, info) => {
  jmap.calendar.events.push(myAssignedTask());
  jmap.emails.unshift({
    calendarBlob: TASK_COUNTER_BLOB,
    from: { email: "carol@partner.example", name: "Carol" },
    html: "<p>Carol proposes a new due date.</p>",
    id: "tsk2",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: new Date().toISOString(),
    subject: "New due date proposed: Print the brochure",
  });
  await page.goto("/mail/inbox");
  await page
    .getByText("New due date proposed: Print the brochure")
    .first()
    .click();
  const card = page.getByRole("region", { name: "Assigned task" });
  await expect(card.getByText("Carol proposes another due date")).toBeVisible();
  await expect(card.getByText(/^Proposed: .*2031/u)).toBeVisible();
  await expect(card.getByText(/^Now: /u)).toBeVisible();
  await expect(card.getByLabel("Note from the sender")).toContainText(
    "The printer is busy until then."
  );
  await shot(page, info, "task-counter-card");

  await card.getByRole("button", { name: "Keep current date" }).click();
  await expect(page.getByText("Carol was told the date stays")).toBeVisible();
  const decline = jmap.calendar.emails.at(-1);
  expect(JSON.stringify(decline?.bodyValues ?? {})).toContain(
    "METHOD:DECLINECOUNTER"
  );
  expect(decline).toMatchObject({
    subject: "Proposal declined: Print the brochure",
    to: [{ email: "carol@partner.example" }],
  });

  await card.getByRole("button", { name: "Accept new date" }).click();
  await expect
    .poll(() => lastSet(jmap))
    .toMatchObject({
      sendSchedulingMessages: true,
      update: { mine1: { due: "2031-03-14T00:00:00", showWithoutTime: true } },
    });
});
