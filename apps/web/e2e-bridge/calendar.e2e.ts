// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The calendar through the real bridge with Radicale
// the views, creating, editing, dragging and deleting events, a
// repeating series ("only this one" and deleting one occurrence, also one
// day of an all-day series, which the bridge keeps), reminders while the
// app is open, a calendar's zone and default reminders, and an invitation
// from alice to bob answered in the mail reader. Everything is checked on
// the CalDAV server itself.
//
// The invitation needs the test Postfix (`E2E_BRIDGE_SMTP=postfix`, as
// e2e-bridge/receipts.e2e.ts): Radicale doesn't schedule, so the bridge
// mails the invitation itself, and Postfix delivers it to bob's Dovecot
// (nothing through Stalwart). Without it, that test is skipped and
// nothing is sent. Everything this run makes has RUN in its title and is
// deleted at the end.

import type { JmapClient } from "@nebulamail/jmap-client";
import { CAPABILITIES } from "@nebulamail/jmap-client";
import type { Browser, Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { destroyMatching, reader, row, shot } from "../e2e-live/ui";
import { chooseOption, expectValue } from "../e2e/select";
import { davCalendarSettings, davText } from "./dav";
import { DOVECOT, SMTP } from "./env";
import { jmapFor, signIn } from "./session";

const ZONE = "Europe/Madrid";
test.use({ timezoneId: ZONE });
test.describe.configure({ mode: "serial", timeout: 180_000 });

/** In every title this run writes. */
const RUN = `cal${Date.now().toString(36)}`;
/** Mail through Postfix, LMTP and the bridge's sync of the inbox. */
const ROUND_TRIP = 90_000;
/** One hour in the time grid, in pixels (components/calendar/time-grid.tsx). */
const HOUR_PX = 48;

/** "YYYY-MM-DD" in Madrid, `days` from today. */
const day = (days: number): string =>
  new Intl.DateTimeFormat("en-CA", { timeZone: ZONE }).format(
    Date.now() + days * 86_400_000
  );
/** "20261028" for iCalendar. */
const compact = (date: string): string => date.replaceAll("-", "");

const signedIn = async (browser: Browser, user: string): Promise<Page> => {
  const context = await browser.newContext({ timezoneId: ZONE });
  const page = await context.newPage();
  await signIn(page, user, DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  return page;
};

const calendarText = async (user = "alice"): Promise<string> =>
  await davText(user, "calendar");

/** The event on the CalDAV server with this text in it ("" if none). */
const eventWith = async (user: string, needle: string): Promise<string> => {
  const text = await calendarText(user);
  return (
    text
      .split(/(?=BEGIN:VCALENDAR)/u)
      .find((calendar) => calendar.includes(needle)) ?? ""
  );
};

const eventButton = (page: Page, title: string): Locator =>
  page.getByRole("button", { name: new RegExp(`^${title}, `, "u") });

const panel = (page: Page): Locator =>
  page.getByRole("region", { name: "Event" });

/** Opens a page of the calendar and waits until it shows `title`. */
const openCalendar = async (
  page: Page,
  search: string,
  title: string
): Promise<void> => {
  await page.goto(`/calendar?${search}`);
  await expect(eventButton(page, title).first()).toBeVisible();
};

/**
 * The editor for a new event, from the URL (as drawing a range does),
 * with its title typed.
 */
const newEvent = async (
  page: Page,
  date: string,
  [start, end]: readonly [string, string],
  name: string
): Promise<void> => {
  await page.goto(
    `/calendar?view=day&date=${date}&new=${date}T${start}&end=${date}T${end}`
  );
  const title = page.getByRole("textbox", { name: "Title" });
  // The first load of the calendar can take a few seconds.
  await expect(title).toBeVisible({ timeout: 20_000 });
  await title.fill(name);
};

const save = async (page: Page): Promise<void> => {
  await page.keyboard.press("ControlOrMeta+Enter");
};

/** Our default calendar's id. */
const defaultCalendar = async (client: JmapClient): Promise<string> => {
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  const [calendars] = await client.request((b) => [
    b.call("Calendar/get", { accountId }),
  ]);
  const calendar =
    calendars.list.find((entry) => entry.isDefault === true) ??
    calendars.list[0];
  const id = calendar?.id;
  if (id === undefined) {
    throw new Error("No calendar (the bridge makes one)");
  }
  return id;
};

/** Deletes every event of the user with RUN in its title, quietly. */
const removeOurs = async (client: JmapClient): Promise<void> => {
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  const [query] = await client.request((b) => [
    b.call("CalendarEvent/query", { accountId }),
  ]);
  if (query.ids.length === 0) {
    return;
  }
  const [events] = await client.request((b) => [
    b.call("CalendarEvent/get", {
      accountId,
      ids: [...query.ids],
      properties: ["title"],
    }),
  ]);
  const ours = events.list
    .filter((event) => (event.title ?? "").includes(RUN))
    .map((event) => event.id);
  if (ours.length > 0) {
    await client.request((b) => [
      b.call("CalendarEvent/set", {
        accountId,
        destroy: ours,
        sendSchedulingMessages: false,
      }),
    ]);
  }
};

test.afterAll(async ({ browser }) => {
  for (const user of ["alice", "bob"]) {
    // oxlint-disable-next-line no-await-in-loop -- one user after the other.
    const page = await signedIn(browser, user);
    // oxlint-disable-next-line no-await-in-loop -- same.
    const client = await jmapFor(page);
    // oxlint-disable-next-line no-await-in-loop -- same.
    await removeOurs(client);
    if (SMTP.name === "postfix") {
      // oxlint-disable-next-line no-await-in-loop -- same.
      await destroyMatching(client, RUN);
    }
    // oxlint-disable-next-line no-await-in-loop -- same.
    await page.context().close();
  }
});

test("create, edit, drag and delete an event; week, month, agenda and day views", async ({
  browser,
}) => {
  const page = await signedIn(browser, "alice");
  const date = day(1);
  const title = `Lunch ${RUN}`;
  await newEvent(page, date, ["10:00", "11:00"], title);
  await save(page);
  await expect(panel(page).getByRole("heading", { name: title })).toBeVisible();
  // On Radicale, in Madrid time.
  await expect
    .poll(async () => await eventWith("alice", title))
    .toContain(`DTSTART;TZID=${ZONE}:${compact(date)}T100000`);

  await panel(page).getByRole("button", { name: "Edit event" }).click();
  const renamed = `${title} and Ana`;
  const titleField = page.getByRole("textbox", { name: "Title" });
  await titleField.fill(renamed);
  await save(page);
  await expect(
    panel(page).getByRole("heading", { name: renamed })
  ).toBeVisible();
  await expect
    .poll(async () => await eventWith("alice", title))
    .toContain(`SUMMARY:${renamed}`);

  // The views.
  await openCalendar(page, `date=${date}`, renamed);
  await page.screenshot(shot("bridge-calendar-week"));
  await page.keyboard.press("m");
  await expect(page).toHaveURL(/view=month/u);
  await expect(eventButton(page, renamed).first()).toBeVisible();
  await page.screenshot(shot("bridge-calendar-month"));
  await page.keyboard.press("a");
  await expect(page).toHaveURL(/view=agenda/u);
  await expect(eventButton(page, renamed).first()).toBeVisible();
  await page.screenshot(shot("bridge-calendar-agenda"));

  // Dragged an hour later in the day view.
  await openCalendar(page, `view=day&date=${date}`, renamed);
  const chip = eventButton(page, renamed).first();
  await chip.scrollIntoViewIfNeeded();
  const box = await chip.boundingBox();
  if (box === null) {
    throw new Error("The event isn't on screen");
  }
  const x = box.x + box.width / 2;
  const y = box.y + 12;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x, y + HOUR_PX / 2, { steps: 4 });
  await page.mouse.move(x, y + HOUR_PX, { steps: 4 });
  await page.mouse.up();
  await expect
    .poll(async () => await eventWith("alice", title))
    .toContain(`DTSTART;TZID=${ZONE}:${compact(date)}T110000`);
  await page.screenshot(shot("bridge-calendar-dragged"));

  await chip.click();
  await panel(page).getByRole("button", { name: "Delete event" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete event" })
    .click();
  await expect(eventButton(page, renamed)).toHaveCount(0);
  await expect.poll(async () => await eventWith("alice", title)).toBe("");
  await page.context().close();
});

test("a daily series: change only one occurrence, delete another", async ({
  browser,
}) => {
  const page = await signedIn(browser, "alice");
  const [first, second, third] = [day(1), day(2), day(3)];
  const title = `Standup ${RUN}`;
  await newEvent(page, first, ["09:00", "09:15"], title);
  await chooseOption(page.getByLabel("Repeat"), { label: "Every day" });
  await save(page);
  await expect(panel(page).getByRole("heading", { name: title })).toBeVisible();
  await expect
    .poll(async () => await eventWith("alice", title))
    .toContain("RRULE:FREQ=DAILY");

  // The second day only.
  await openCalendar(page, `view=day&date=${second}`, title);
  await eventButton(page, title).first().click();
  await panel(page).getByRole("button", { name: "Edit event" }).click();
  await page.getByRole("textbox", { name: "Title" }).fill(`${title} (room 4)`);
  await save(page);
  const scope = page.getByRole("dialog", { name: "Change a repeating event" });
  await scope.getByRole("button", { name: "This event" }).click();
  await expect(eventButton(page, `${title} \\(room 4\\)`)).toBeVisible();
  await expect
    .poll(async () => await eventWith("alice", title))
    .toContain(`RECURRENCE-ID;TZID=${ZONE}:${compact(second)}T090000`);
  // The others are still the series.
  await openCalendar(page, `view=day&date=${third}`, title);
  await expect(eventButton(page, `${title} \\(room 4\\)`)).toHaveCount(0);

  // The third day deleted.
  await eventButton(page, title).first().click();
  await panel(page).getByRole("button", { name: "Delete event" }).click();
  await page
    .getByRole("dialog", { name: "Delete a repeating event" })
    .getByRole("button", { name: "This event" })
    .click();
  await expect(eventButton(page, title)).toHaveCount(0);
  await expect
    .poll(async () => await eventWith("alice", title))
    .toMatch(new RegExp(`EXDATE;TZID=${ZONE}:${compact(third)}T090000`, "u"));

  // The week: the first day as it was, the second changed, no third.
  await openCalendar(page, `date=${first}`, title);
  await eventButton(page, title).first().scrollIntoViewIfNeeded();
  await page.screenshot(shot("bridge-calendar-series"));
  await page.context().close();
});

test("one day of an all-day series changes alone (the bridge keeps the series)", async ({
  browser,
}) => {
  const page = await signedIn(browser, "alice");
  const [first, second, third] = [day(1), day(2), day(3)];
  const title = `Offsite ${RUN}`;
  await page.goto(`/calendar?view=day&date=${first}&new=${first}`);
  const titleField = page.getByRole("textbox", { name: "Title" });
  await expect(titleField).toBeVisible();
  await titleField.fill(title);
  await expect(page.getByRole("switch", { name: "All day" })).toBeChecked();
  await chooseOption(page.getByLabel("Repeat"), { label: "Every day" });
  await save(page);
  await expect(panel(page).getByRole("heading", { name: title })).toBeVisible();

  await openCalendar(page, `view=day&date=${second}`, title);
  await eventButton(page, title).first().click();
  await panel(page).getByRole("button", { name: "Edit event" }).click();
  await page.getByRole("textbox", { name: "Title" }).fill(`${title} (Lyon)`);
  await save(page);
  const scope = page.getByRole("dialog", { name: "Change a repeating event" });
  // Offered with every server.
  const one = scope.getByRole("button", { name: "This event" });
  await expect(one).toBeVisible();
  await expect(scope.getByText(/can't be changed on its own/u)).toHaveCount(0);
  await page.screenshot(shot("bridge-calendar-all-day-scope"));
  await one.click();
  await expect(eventButton(page, `${title} \\(Lyon\\)`)).toBeVisible();
  const stored = async () => await eventWith("alice", title);
  await expect
    .poll(stored)
    .toContain(`RECURRENCE-ID;VALUE=DATE:${compact(second)}`);
  // The series is still there, and so are the other days.
  expect(await stored()).toContain("RRULE:FREQ=DAILY");
  await openCalendar(page, `view=day&date=${third}`, title);
  await expect(eventButton(page, `${title} \\(Lyon\\)`)).toHaveCount(0);
  await page.context().close();
});

/** "2026-10-27T19:05:00" in `zone` for an instant. */
const wallTime = (ms: number, zone: string): string => {
  const parts = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", {
      day: "2-digit",
      hour: "2-digit",
      hourCycle: "h23",
      minute: "2-digit",
      month: "2-digit",
      second: "2-digit",
      timeZone: zone,
      year: "numeric",
    })
      .formatToParts(ms)
      .map((part) => [part.type, part.value])
  );
  return `${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}:${parts.second}`;
};

test("reminders arrive while the app is open, at the occurrence's time", async ({
  browser,
}) => {
  const page = await signedIn(browser, "alice");
  const client = await jmapFor(page);
  const accountId = client.primaryAccountId(CAPABILITIES.calendars);
  const calendar = await defaultCalendar(client);
  // Both start on the next whole minute at least 75 s away: the alert
  // (a minute before) comes 15–75 s from now.
  const at = Math.ceil((Date.now() + 75_000) / 60_000) * 60_000;
  const alert = {
    a1: {
      "@type": "Alert",
      action: "display",
      trigger: { "@type": "OffsetTrigger", offset: "-PT1M" },
    },
  };
  const newYork = wallTime(at, "America/New_York");
  const [created] = await client.request((b) => [
    b.call("CalendarEvent/set", {
      accountId,
      create: {
        call: {
          "@type": "Event",
          alerts: alert,
          calendarIds: { [calendar]: true },
          duration: "PT30M",
          start: wallTime(at, ZONE),
          timeZone: ZONE,
          title: `Call ${RUN}`,
        },
        // A series in another zone than the browser's: the alert's
        // recurrenceId is a New York wall time (4.6).
        daily: {
          "@type": "Event",
          alerts: alert,
          calendarIds: { [calendar]: true },
          duration: "PT30M",
          recurrenceRule: { "@type": "RecurrenceRule", frequency: "daily" },
          start: wallTime(at - 2 * 86_400_000, "America/New_York").replace(
            /T.*/u,
            newYork.slice(10)
          ),
          timeZone: "America/New_York",
          title: `Daily ${RUN}`,
        },
      },
      sendSchedulingMessages: false,
    }),
  ]);
  expect(Object.keys(created.created)).toStrictEqual(["call", "daily"]);

  // Any page of the app shows them.
  await page.goto("/mail/inbox");
  const call = page.getByRole("region", { name: `Reminder: Call ${RUN}` });
  const daily = page.getByRole("region", { name: `Reminder: Daily ${RUN}` });
  await expect(call).toBeVisible({ timeout: 90_000 });
  await expect(daily).toBeVisible({ timeout: 30_000 });
  // "7:05" of "7:05 – 7:35 PM", in Madrid (the browser), for both.
  const madrid = new Intl.DateTimeFormat("en-US", {
    hour: "numeric",
    minute: "2-digit",
    timeZone: ZONE,
  })
    .format(at)
    .replace(/\s?[AP]M$/u, "");
  const shown = new RegExp(`(?:^|\\D)${madrid}\\D`, "u");
  await expect(call).toContainText(shown);
  await expect(daily).toContainText(shown);
  await page.screenshot(shot("bridge-calendar-reminders"));

  await call.getByRole("button", { name: "Dismiss" }).click();
  await expect(call).toHaveCount(0);
  // Stored in the event (every device stops ringing).
  await expect
    .poll(async () => await eventWith("alice", `Call ${RUN}`))
    .toContain("ACKNOWLEDGED:");
  await page.context().close();
});

/** Removes every default reminder in a settings dialog (a failed run's too). */
const clearReminders = async (dialog: Locator): Promise<void> => {
  const remove = dialog.getByRole("button", { name: "Remove reminder" });
  // One after the other: each click re-renders the list.
  // oxlint-disable-next-line no-await-in-loop -- see above.
  while ((await remove.count()) > 0) {
    // oxlint-disable-next-line no-await-in-loop -- see above.
    await remove.first().click();
  }
};

test("a calendar's zone and default reminders are stored in CalDAV", async ({
  browser,
}) => {
  const page = await signedIn(browser, "alice");
  await page.goto("/calendar");
  const side = page.getByRole("navigation", { name: "Calendar menu" });
  const options = side.getByRole("button", { name: /^Options for /u }).first();
  await expect(options).toBeVisible({ timeout: 20_000 });
  const openSettings = async () => {
    await options.click();
    await page.getByRole("menuitem", { name: "Calendar settings…" }).click();
    return page.getByRole("dialog", { name: "Calendar", exact: true });
  };

  let dialog = await openSettings();
  await clearReminders(dialog);
  await chooseOption(
    dialog.getByRole("combobox", { name: "Time zone" }),
    "America/New_York"
  );
  const defaults = dialog.getByRole("group", { name: "Default reminders" });
  await defaults
    .getByRole("button", { name: "Add a reminder" })
    .first()
    .click();
  await chooseOption(
    defaults.getByRole("combobox", { name: "Reminder" }).first(),
    "30"
  );
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toHaveCount(0);
  // On Radicale: the zone id and a VALARM 30 minutes before.
  await expect
    .poll(async () => await davCalendarSettings("alice"))
    .toMatch(/America\/New_York[\s\S]*TRIGGER[^\n]*-PT30M/u);

  // A new event starts with them (read back through the bridge).
  await side.getByRole("button", { name: /^New event/u }).click();
  const editor = page.getByRole("form", { name: "New event" });
  await expectValue(
    editor.getByRole("combobox", { name: "Time zone" }),
    "America/New_York"
  );
  await expectValue(editor.getByRole("combobox", { name: "Reminder" }), "30");
  await page.keyboard.press("Escape");
  await expect(editor).toHaveCount(0);

  // Back as it was: no zone, no default reminders.
  dialog = await openSettings();
  await chooseOption(dialog.getByRole("combobox", { name: "Time zone" }), "");
  await clearReminders(dialog);
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toHaveCount(0);
  await expect
    .poll(async () => await davCalendarSettings("alice"))
    .not.toMatch(/America\/New_York|-PT30M/u);
  await page.context().close();
});

test(
  "alice invites bob; bob accepts in the mail; alice sees his answer",
  { tag: "@postfix" },
  async ({ browser }) => {
    test.skip(
      SMTP.name !== "postfix",
      "needs the test Postfix: E2E_BRIDGE_SMTP=postfix (docker/README.md)"
    );
    const alice = await signedIn(browser, "alice");
    const date = day(1);
    const title = `Planning ${RUN}`;
    await newEvent(alice, date, ["12:00", "13:00"], title);
    const guests = alice.getByRole("textbox", { name: "Guests" });
    await guests.fill("bob@nebulamail.test");
    await guests.press("Enter");
    await expect(
      alice.getByRole("checkbox", { name: "Email invitations to the guests" })
    ).toBeChecked();
    await save(alice);
    await expect(
      panel(alice).getByRole("heading", { name: title })
    ).toBeVisible();
    // Radicale doesn't schedule: the bridge mails the invitation and marks
    // the guest so that no server sends another one.
    await expect
      .poll(async () => await eventWith("alice", title))
      .toMatch(
        /ATTENDEE;[^\n]*SCHEDULE-AGENT=CLIENT[^\n]*bob@nebulamail\.test/u
      );

    const bob = await signedIn(browser, "bob");
    const invitation = `Invitation: ${title}`;
    await expect(row(bob, invitation)).toBeVisible({ timeout: ROUND_TRIP });
    await row(bob, invitation).click();
    const card = reader(bob).getByRole("region", {
      name: "Calendar invitation",
    });
    await expect(card.getByRole("heading", { name: title })).toBeVisible();
    await bob.screenshot(shot("bridge-calendar-invitation"));
    await card.getByRole("button", { name: "Yes" }).click();
    await expect(bob.getByText("Answer sent to the organizer")).toBeVisible();
    // In bob's own calendar on Radicale, accepted.
    await expect
      .poll(async () => await eventWith("bob", title))
      .toMatch(/PARTSTAT=ACCEPTED[^\n]*bob@nebulamail\.test/u);

    // His answer reaches alice; her card updates her event.
    const answer = `Accepted: ${title}`;
    await alice.goto("/mail/inbox");
    await expect(row(alice, answer)).toBeVisible({ timeout: ROUND_TRIP });
    await row(alice, answer).click();
    const reply = reader(alice).getByRole("region", {
      name: "Calendar invitation",
    });
    await expect(reply).toContainText("going");
    await reply.getByRole("button", { name: "Update my event" }).click();
    await expect(alice.getByText("Your event was updated")).toBeVisible();
    await alice.screenshot(shot("bridge-calendar-reply"));
    await expect
      .poll(async () => await eventWith("alice", title))
      .toMatch(/PARTSTAT=ACCEPTED[^\n]*bob@nebulamail\.test/u);

    // And her calendar shows him going.
    await openCalendar(alice, `view=day&date=${date}`, title);
    await eventButton(alice, title).first().click();
    const people = panel(alice).getByRole("list", { name: "Guests" });
    await expect(
      people.getByRole("listitem").filter({ hasText: "bob" })
    ).toContainText("Going");
    await alice.screenshot(shot("bridge-calendar-accepted"));
    await alice.context().close();
    await bob.context().close();
  }
);
