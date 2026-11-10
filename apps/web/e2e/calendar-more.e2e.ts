// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The calendar's second round, with the fake
// JMAP server: calendars others share (their own account), creating,
// recolouring and deleting calendars, drawing a range to create an event,
// reminders (CalendarAlert on the event source, dismiss and snooze), an
// invitation to one occurrence of a series, a note with the answer,
// proposing another time (COUNTER) and answering a proposal.
// Screenshots go to /tmp/nebula-calendar (outside the repo).

import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, test } from "@playwright/test";

import {
  COUNTER_BLOB,
  INVITATION_BLOB,
  OCCURRENCE_BLOB,
  OCCURRENCE_ID,
  PLANNING_ID,
  PLANNING_UID,
  SHARED_CALENDAR_ACCOUNT,
  SHARED_CALENDAR_ID,
  WEEKLY_UID,
  todayUtc,
  tomorrowUtc,
} from "./fake-calendar";
import type { FakeEmail, FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { chooseOption, expectValue, optionNames } from "./select";
import { signIn } from "./session";

// Event times in the fake server are UTC.
test.use({ timezoneId: "UTC" });

const PHONE_MAX = 640;
const HOUR_PX = 48;

const shot = async (page: Page, info: TestInfo, name: string) => {
  await page.screenshot({
    path: `/tmp/nebula-calendar/${info.project.name}/${name}.png`,
  });
};

const isPhone = (page: Page): boolean =>
  (page.viewportSize()?.width ?? 0) <= PHONE_MAX;

const isTouch = (info: TestInfo): boolean => info.project.use.hasTouch === true;

const expectNoSidewaysScroll = async (page: Page) => {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth
  );
  expect(overflow).toBeLessThanOrEqual(0);
};

const eventButton = (page: Page, title: string): Locator =>
  page.getByRole("button", { name: new RegExp(`^${title}, `, "u") }).first();

/** The sidebar: in place, or in the drawer on phones. */
const sidebar = async (page: Page): Promise<Locator> => {
  if (isPhone(page)) {
    await page.getByRole("button", { name: "Calendar menu" }).click();
    return page.getByRole("dialog", { name: "Calendar menu" });
  }
  return page.getByRole("navigation", { name: "Calendar menu" });
};

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

const invitationEmail = (
  id: string,
  calendarBlob: string,
  subject: string
): FakeEmail => ({
  calendarBlob,
  from: { email: "carol@partner.example", name: "Carol" },
  html: `<p>${subject}</p>`,
  id,
  keywords: {},
  mailboxIds: { inbox: true },
  receivedAt: new Date().toISOString(),
  subject,
});

const openCard = async (page: Page, email: FakeEmail): Promise<Locator> => {
  jmap.emails.unshift(email);
  // Straight to the email (the list itself is tested elsewhere).
  await page.goto(`/mail/inbox/${email.id}`);
  const card = page.getByRole("region", { name: "Calendar invitation" });
  await expect(card).toBeVisible();
  return card;
};

const lastIcs = (): string =>
  JSON.stringify(jmap.calendar.emails.at(-1)?.bodyValues ?? {});

test("a calendar someone shares: its events, in its own account", async ({
  page,
}, info) => {
  await page.goto("/calendar");
  // Bob's event has the same id as our Standup: both must show.
  await expect(eventButton(page, "Standup")).toBeVisible();
  await expect(eventButton(page, "Team review")).toBeVisible();
  expect(jmap.accountsUsed).toContain(SHARED_CALENDAR_ACCOUNT);
  await eventButton(page, "Team review").click();
  const panel = page.getByRole("region", { name: "Event" });
  await expect(
    panel.getByRole("heading", { name: "Team review" })
  ).toBeVisible();
  await expect(page).toHaveURL(/account=b/u);
  await page.reload();
  await expect(
    page.getByRole("region", { name: "Event" }).getByRole("heading", {
      name: "Team review",
    })
  ).toBeVisible();
  const side = await sidebar(page);
  await expect(
    side.getByRole("region", { name: "Shared by bob@example.com" })
  ).toBeVisible();
  await expect(side.getByText("Team calendar")).toBeVisible();
  await shot(page, info, "shared");
  await expectNoSidewaysScroll(page);
});

test("create, recolour and delete a calendar", async ({ page }, info) => {
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  const side = await sidebar(page);
  await side.getByRole("button", { name: "New calendar" }).click();
  const dialog = page.getByRole("dialog", { name: "New calendar" });
  await dialog.getByRole("textbox", { name: "Name" }).fill("Gym");
  await dialog.getByRole("button", { name: "Colour 3" }).click();
  await expect(
    dialog.getByRole("button", { name: "Colour 3" })
  ).toHaveAttribute("aria-pressed", "true");
  await shot(page, info, "new-calendar");
  await dialog.getByRole("button", { name: "Create" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1))
    .toMatchObject({
      accountId: "a",
      create: { calendar: { color: "#0E7A4E", name: "Gym" } },
    });
  await expect(side.getByText("Gym", { exact: true })).toBeVisible();

  // Someone else's calendar: only its colour (on our side).
  await side.getByRole("button", { name: "Options for Team calendar" }).click();
  await page.getByRole("menuitem", { name: "Colour…" }).click();
  const colour = page.getByRole("dialog", { name: "Calendar" });
  await expect(colour.getByRole("textbox", { name: "Name" })).toHaveCount(0);
  await colour.getByRole("button", { name: "Colour 7" }).click();
  await colour.getByRole("button", { name: "Save" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1))
    .toMatchObject({
      accountId: SHARED_CALENDAR_ACCOUNT,
      update: { [SHARED_CALENDAR_ID]: { color: "#DB2777" } },
    });

  // A task and a journal entry of another app (VJOURNAL: a `Group` from
  // Stalwart and the bridge) live in it too: the dialog says so.
  jmap.calendar.events.push(
    {
      "@type": "Task",
      calendarIds: { cal2: true },
      id: "gym-task",
      title: "Buy shoes",
    },
    {
      "@type": "Group",
      calendarIds: { cal2: true },
      id: "gym-journal",
    }
  );
  await side.getByRole("button", { name: "Options for Gym" }).click();
  await page.getByRole("menuitem", { name: "Delete calendar…" }).click();
  const confirm = page.getByRole("alertdialog");
  await expect(confirm.getByText("Delete “Gym”?")).toBeVisible();
  await expect(confirm).toContainText(
    "It also has 1 task, which will be deleted too."
  );
  await expect(confirm).toContainText(
    "It also has 1 item from another app that Nebula Mail doesn’t show"
  );
  await confirm.getByRole("button", { name: "Delete" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1))
    .toMatchObject({ destroy: ["cal2"], onDestroyRemoveEvents: true });
  // Our default calendar can't be deleted from here.
  await side
    .getByRole("button", {
      name: "Options for Stalwart Calendar (alice@nebulamail.test)",
    })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Delete calendar…" })
  ).toHaveCount(0);
});

/**
 * Screen points of two wall times (minutes) in today's column, after
 * scrolling the hours so both are in view.
 */
const columnPoints = async (
  page: Page,
  from: number,
  to: number
): Promise<readonly [{ x: number; y: number }, { x: number; y: number }]> => {
  const header = page.locator('[aria-current="date"]').first();
  const label = (await header.getAttribute("aria-label")) ?? "";
  const day = page.locator(
    `section[aria-label="${label.replace(/^Open /u, "")}"]`
  );
  await day.evaluate(
    (element, top) => {
      let node = element.parentElement;
      while (node !== null && getComputedStyle(node).overflowY !== "auto") {
        node = node.parentElement;
      }
      if (node !== null) {
        node.scrollTop = Math.max(0, top - 150);
      }
    },
    (from * HOUR_PX) / 60
  );
  const box = await day.boundingBox();
  if (box === null) {
    throw new Error("No day column");
  }
  const x = box.x + box.width / 2;
  return [
    { x, y: box.y + (from * HOUR_PX) / 60 },
    { x, y: box.y + (to * HOUR_PX) / 60 },
  ] as const;
};

test("drag over empty time to create an event that long", async ({
  page,
}, info) => {
  test.skip(isTouch(info), "mouse; the finger version is below");
  await page.goto("/calendar?view=day");
  await expect(eventButton(page, "Standup")).toBeVisible();
  const [from, to] = await columnPoints(page, 16 * 60 + 35, 17 * 60 + 40);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(to.x, (from.y + to.y) / 2, { steps: 4 });
  await page.mouse.move(to.x, to.y, { steps: 4 });
  await expect(page.getByText(/^New event, /u)).toBeVisible();
  await shot(page, info, "drawing");
  await page.mouse.up();
  const day = todayUtc();
  await expect(page).toHaveURL(
    new RegExp(
      `new=${day}T16%3A30&end=${day}T17%3A45|end=${day}T17%3A45.*new=${day}T16%3A30`,
      "u"
    )
  );
  await page.getByRole("textbox", { name: "Title" }).fill("Focus time");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      create: {
        event: {
          duration: "PT1H15M",
          start: `${day}T16:30:00`,
          title: "Focus time",
        },
      },
    });
});

test("with a finger: rest, then drag (a swipe still scrolls)", async ({
  page,
}, info) => {
  test.skip(!isTouch(info), "touch screens");
  await page.goto("/calendar?view=day");
  await expect(eventButton(page, "Standup")).toBeVisible();
  const [from, to] = await columnPoints(page, 16 * 60 + 35, 17 * 60 + 40);
  const cdp = await page.context().newCDPSession(page);
  const touch = async (
    type: "touchStart" | "touchMove" | "touchEnd",
    x: number,
    y: number
  ) => {
    await cdp.send("Input.dispatchTouchEvent", {
      touchPoints: type === "touchEnd" ? [] : [{ x, y }],
      type,
    });
  };
  await touch("touchStart", from.x, from.y);
  // oxlint-disable-next-line playwright/no-wait-for-timeout -- the long press itself is a wait.
  await page.waitForTimeout(700);
  for (let step = 1; step <= 6; step += 1) {
    // oxlint-disable-next-line no-await-in-loop -- a finger moves in order.
    await touch("touchMove", from.x, from.y + ((to.y - from.y) * step) / 6);
  }
  await touch("touchEnd", to.x, to.y);
  const day = todayUtc();
  await expect(page).toHaveURL(new RegExp(`end=${day}T17%3A45`, "u"));
  await expect(page.getByRole("textbox", { name: "Title" })).toBeVisible();
  await shot(page, info, "drawn-by-finger");
});

test("a reminder: shown in the app, dismissed on the server", async ({
  page,
}, info) => {
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  jmap.calendar.alerts.push({
    "@type": "CalendarAlert",
    accountId: "a",
    alertId: "a1",
    calendarEventId: PLANNING_ID,
    recurrenceId: null,
    uid: PLANNING_UID,
  });
  const reminders = page.getByRole("region", { name: "Reminders" });
  const reminder = reminders.getByRole("region", {
    name: "Reminder: Planning",
  });
  await expect(reminder).toBeVisible({ timeout: 15_000 });
  await shot(page, info, "reminder");
  await expectNoSidewaysScroll(page);
  await reminder.getByRole("button", { name: "Dismiss" }).click();
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      sendSchedulingMessages: false,
      update: {
        [PLANNING_ID]: {
          "alerts/a1/acknowledged": expect.stringMatching(/Z$/u),
        },
      },
    });
  await expect(reminder).toHaveCount(0);
});

test.describe("in another time zone than the event", () => {
  test.use({ timezoneId: "America/New_York" });

  test("a reminder of one occurrence shows its real time", async ({ page }) => {
    // A daily series in Madrid; the occurrence two hours from now.
    const at = new Date(
      Math.ceil(Date.now() / 3_600_000) * 3_600_000 + 7_200_000
    );
    const wall = (zone: string): string => {
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
          .formatToParts(at)
          .map((part) => [part.type, part.value])
      );
      return `${parts.year}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}:${parts.second}`;
    };
    const madrid = wall("Europe/Madrid");
    jmap.calendar.events.push({
      "@type": "Event",
      alerts: {
        a1: {
          "@type": "Alert",
          action: "display",
          trigger: { "@type": "OffsetTrigger", offset: "-PT10M" },
        },
      },
      calendarIds: { cal1: true },
      duration: "PT1H",
      id: "daily1",
      recurrenceRule: { "@type": "RecurrenceRule", frequency: "daily" },
      // The series started a week ago, same wall time.
      start: `${new Date(at.getTime() - 7 * 86_400_000).toISOString().slice(0, 10)}${madrid.slice(10)}`,
      timeZone: "Europe/Madrid",
      title: "Madrid daily",
      uid: "daily-1@example.com",
    });
    await page.goto("/mail/inbox");
    // Like Stalwart: the occurrence's start in the event's zone, no `Z`.
    jmap.calendar.alerts.push({
      "@type": "CalendarAlert",
      accountId: "a",
      alertId: "a1",
      calendarEventId: "daily1",
      recurrenceId: madrid,
      uid: "daily-1@example.com",
    });
    const reminder = page.getByRole("region", {
      name: "Reminder: Madrid daily",
    });
    await expect(reminder).toBeVisible({ timeout: 15_000 });
    // "4:00" (of "4:00 – 5:00 PM"): the occurrence in the browser's zone.
    const newYork = new Intl.DateTimeFormat("en-US", {
      hour: "numeric",
      minute: "2-digit",
      timeZone: "America/New_York",
    })
      .format(at)
      .replace(/\s?[AP]M$/u, "");
    await expect(reminder).toContainText(
      new RegExp(`(?:^|\\D)${newYork}\\D`, "u")
    );
  });
});

test("a reminder snoozed for 10 minutes", async ({ page }) => {
  await page.goto("/mail/inbox");
  jmap.calendar.alerts.push({
    "@type": "CalendarAlert",
    accountId: "a",
    alertId: "a1",
    calendarEventId: PLANNING_ID,
    recurrenceId: null,
    uid: PLANNING_UID,
  });
  // Reminders show on every screen of the app, not only the calendar.
  const reminder = page.getByRole("region", { name: "Reminder: Planning" });
  await expect(reminder).toBeVisible({ timeout: 15_000 });
  await reminder.getByRole("button", { name: "Snooze" }).click();
  await page.getByRole("menuitem", { name: "For 10 minutes" }).click();
  await expect.poll(() => jmap.calendar.sets.length).toBeGreaterThan(0);
  const update = JSON.stringify(jmap.calendar.sets.at(-1));
  expect(update).toContain("alerts/a1/acknowledged");
  expect(update).toContain('"alerts/snz');
  expect(update).toContain('"@type":"AbsoluteTrigger"');
  expect(update).toContain('"relatedTo":{"a1"');
  await expect(reminder).toHaveCount(0);
});

test("an invitation to one occurrence answers only that occurrence", async ({
  page,
}, info) => {
  const card = await openCard(
    page,
    invitationEmail("occ1", OCCURRENCE_BLOB, "Updated invitation: Weekly sync")
  );
  await expect(
    card.getByRole("heading", { name: "Weekly sync" })
  ).toBeVisible();
  await expect(
    card.getByText("In your calendar", { exact: true })
  ).toBeVisible();
  await shot(page, info, "occurrence-invitation");
  expect(jmap.calendar.sets).toStrictEqual([]);
  await card.getByRole("button", { name: "Yes" }).click();
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      sendSchedulingMessages: false,
      update: {
        [OCCURRENCE_ID]: { "participants/j/participationStatus": "accepted" },
      },
    });
  // The series itself isn't touched.
  expect(JSON.stringify(jmap.calendar.sets)).not.toContain('"ser1":');
  await expect.poll(() => jmap.calendar.submissions.length).toBe(1);
  const ics = lastIcs();
  expect(ics).toContain("METHOD:REPLY");
  expect(ics).toContain(`UID:${WEEKLY_UID}`);
  expect(ics).toContain(
    `RECURRENCE-ID:${todayUtc().replaceAll("-", "")}T100000Z`
  );
});

test("answer with a note for the organizer", async ({ page }) => {
  const card = await openCard(
    page,
    invitationEmail("inv1", INVITATION_BLOB, "Invitation: Board meeting")
  );
  await card
    .getByRole("button", { name: "Add a note or propose a time" })
    .click();
  await page
    .getByRole("textbox", { name: "Note for the organizer" })
    .fill("Running 5 minutes late, sorry");
  await card.getByRole("button", { name: "Maybe" }).click();
  await expect.poll(() => jmap.calendar.submissions.length).toBe(1);
  const ics = lastIcs();
  expect(ics).toContain("PARTSTAT=TENTATIVE");
  expect(ics).toContain("COMMENT:Running 5 minutes late\\\\, sorry");
  expect(ics).toContain("Running 5 minutes late, sorry");
});

test("propose another time (COUNTER), nothing changes here", async ({
  page,
}, info) => {
  const card = await openCard(
    page,
    invitationEmail("inv1", INVITATION_BLOB, "Invitation: Board meeting")
  );
  await card
    .getByRole("button", { name: "Add a note or propose a time" })
    .click();
  await shot(page, info, "propose");
  await expectNoSidewaysScroll(page);
  await page
    .getByRole("textbox", { name: "Note for the organizer" })
    .fill("Later?");
  await page.getByRole("button", { name: "Send proposal" }).click();
  await expect.poll(() => jmap.calendar.submissions.length).toBe(1);
  const email = jmap.calendar.emails.at(-1);
  expect(email).toMatchObject({
    bodyStructure: {
      subParts: [
        { type: "text/plain" },
        { type: "text/calendar; method=COUNTER" },
      ],
    },
    subject: "New time proposed: Board meeting",
    to: [{ email: "carol@partner.example" }],
  });
  const ics = lastIcs();
  expect(ics).toContain("METHOD:COUNTER");
  expect(ics).toContain(`DTSTART:${todayUtc().replaceAll("-", "")}T133000Z`);
  expect(ics).toContain("COMMENT:Later?");
  // Proposing doesn't add or change anything in our calendar.
  expect(jmap.calendar.sets).toStrictEqual([]);
});

test("a guest's proposal: accept moves the event and tells everyone", async ({
  page,
}, info) => {
  const card = await openCard(
    page,
    invitationEmail("ctr1", COUNTER_BLOB, "New time proposed: Planning")
  );
  await expect(card.getByText("Carol proposes a new time")).toBeVisible();
  await expect(card.getByText("Can we start at 10?")).toBeVisible();
  await expect(card.getByText(/^Now: /u)).toBeVisible();
  await shot(page, info, "counter");
  await card.getByRole("button", { name: "Accept new time" }).click();
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      sendSchedulingMessages: true,
      update: {
        [PLANNING_ID]: {
          duration: "PT1H",
          start: `${tomorrowUtc()}T10:00:00`,
        },
      },
    });
});

test("a guest's proposal: keep the time (DECLINECOUNTER)", async ({ page }) => {
  const card = await openCard(
    page,
    invitationEmail("ctr1", COUNTER_BLOB, "New time proposed: Planning")
  );
  await card.getByRole("button", { name: "Keep current time" }).click();
  await expect.poll(() => jmap.calendar.submissions.length).toBe(1);
  expect(jmap.calendar.emails.at(-1)).toMatchObject({
    subject: "Proposal declined: Planning",
    to: [{ email: "carol@partner.example" }],
  });
  const ics = lastIcs();
  expect(ics).toContain("METHOD:DECLINECOUNTER");
  expect(ics).toContain(`UID:${PLANNING_UID}`);
  expect(jmap.calendar.sets).toStrictEqual([]);
});

/** A weekly series of ours with Carol invited, and today's occurrence. */
const ourSeries = (
  id: string,
  title: string,
  timing: { readonly allDay: boolean; readonly time: string }
): void => {
  const start = `${todayUtc()}T${timing.time}:00`;
  const duration = timing.allDay ? "P1D" : "PT1H";
  const carol = {
    "@type": "Participant",
    calendarAddress: "mailto:carol@partner.example",
    expectReply: true,
    name: "Carol",
    participationStatus: "needs-action",
    roles: { attendee: true },
  };
  const jane = {
    "@type": "Participant",
    calendarAddress: "mailto:jane@example.com",
    name: "Jane",
    participationStatus: "accepted",
    roles: { attendee: true, owner: true },
  };
  const utcStart = `${start}Z`;
  const utcEnd = new Date(
    Date.parse(utcStart) + (timing.allDay ? 1440 : 60) * 60_000
  )
    .toISOString()
    .replace(".000", "");
  jmap.calendar.events.push({
    "@type": "Event",
    calendarIds: { cal1: true },
    duration,
    id,
    organizerCalendarAddress: "mailto:jane@example.com",
    participants: { c: carol, o: jane },
    recurrenceRule: { "@type": "RecurrenceRule", frequency: "weekly" },
    sequence: 2,
    ...(timing.allDay ? { showWithoutTime: true } : { timeZone: "Etc/UTC" }),
    start,
    title,
    uid: `${id}@example.com`,
  });
  // Like Stalwart: occurrences lose showWithoutTime (read from the series).
  jmap.calendar.occurrences.push({
    baseEventId: id,
    calendarIds: { cal1: true },
    duration,
    id: `${id}-occ`,
    // She answered for this occurrence only: its override says so.
    participants: { c: { ...carol, participationStatus: "accepted" }, o: jane },
    recurrenceId: start,
    start,
    title,
    uid: `${id}@example.com`,
    utcEnd,
    utcStart,
    ...(timing.allDay ? {} : { timeZone: "Etc/UTC" }),
  });
};

test("moving one occurrence keeps its guests in the override", async ({
  page,
}) => {
  ourSeries("ser3", "Team lunch", { allDay: false, time: "12:00" });
  await page.goto("/calendar");
  const lunch = eventButton(page, "Team lunch");
  await expect(lunch).toBeVisible();
  await lunch.focus();
  await page.keyboard.press("Alt+ArrowDown");
  await page.getByRole("button", { name: "This event" }).click();
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      sendSchedulingMessages: true,
      update: {
        "ser3-occ": {
          // Stalwart only applies later replies to this occurrence if
          // its override has them; her answer stays.
          participants: {
            c: {
              calendarAddress: "mailto:carol@partner.example",
              participationStatus: "accepted",
            },
            o: { calendarAddress: "mailto:jane@example.com" },
          },
          start: `${todayUtc()}T12:15:00`,
        },
      },
    });
  expect(JSON.stringify(jmap.calendar.sets)).not.toContain('"ser3":');
  // The server tells the guests (Stalwart 1.0.1 sends the REQUEST for one
  // occurrence itself, with SEQUENCE raised): the app sends
  // no second, stale one. Give a stray email time to show up: none does.
  await page.waitForTimeout(500);
  expect(jmap.calendar.submissions).toStrictEqual([]);
});

test("one day of an all-day series changes alone", async ({ page }) => {
  // Stalwart keeps the series when one of its days changes.
  ourSeries("ser2", "Offsite", { allDay: true, time: "00:00" });
  await page.goto("/calendar?event=ser2-occ&series=ser2");
  await page.getByRole("button", { name: "Edit event" }).click();
  const title = page.getByRole("textbox", { name: "Title" });
  await expect(title).toHaveValue("Offsite");
  await title.fill("Offsite room 4");
  await page.keyboard.press("ControlOrMeta+Enter");
  const dialog = page.getByRole("dialog", {
    name: "Change a repeating event",
  });
  await expect(dialog.getByText(/can't be changed on its own/u)).toHaveCount(0);
  await dialog.getByRole("button", { name: "This event" }).click();
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      update: { "ser2-occ": { title: "Offsite room 4" } },
    });
  expect(JSON.stringify(jmap.calendar.sets)).not.toContain('"ser2":');
  // Stalwart sends the REQUEST for that day itself.
  await page.waitForTimeout(500);
  expect(jmap.calendar.submissions).toStrictEqual([]);
});

// With the bridge the bridge sends the
// proposals and one occurrence's REQUEST itself: the app sends no email.

test("with the bridge: a proposal goes by CalendarEvent/counter", async ({
  page,
}) => {
  jmap.calendar.bridge = true;
  const card = await openCard(
    page,
    invitationEmail("inv1", INVITATION_BLOB, "Invitation: Board meeting")
  );
  await card
    .getByRole("button", { name: "Add a note or propose a time" })
    .click();
  await page
    .getByRole("textbox", { name: "Note for the organizer" })
    .fill("Later?");
  await page.getByRole("button", { name: "Send proposal" }).click();
  await expect.poll(() => jmap.calendar.proposals.length).toBe(1);
  const [name, args] = jmap.calendar.proposals[0] ?? [];
  expect(name).toBe("CalendarEvent/counter");
  expect(args).toMatchObject({
    blobId: INVITATION_BLOB,
    comment: "Later?",
    utcStart: `${todayUtc()}T13:30:00Z`,
  });
  expect(args).not.toHaveProperty("eventId");
  expect(jmap.calendar.submissions).toStrictEqual([]);
  expect(jmap.calendar.sets).toStrictEqual([]);
});

test("with the bridge: keeping the time goes by CalendarEvent/declineCounter", async ({
  page,
}) => {
  jmap.calendar.bridge = true;
  const card = await openCard(
    page,
    invitationEmail("ctr1", COUNTER_BLOB, "New time proposed: Planning")
  );
  await card.getByRole("button", { name: "Keep current time" }).click();
  await expect.poll(() => jmap.calendar.proposals.length).toBe(1);
  expect(jmap.calendar.proposals[0]).toStrictEqual([
    "CalendarEvent/declineCounter",
    {
      accountId: expect.any(String),
      blobId: COUNTER_BLOB,
      eventId: PLANNING_ID,
    },
  ]);
  expect(jmap.calendar.submissions).toStrictEqual([]);
  expect(jmap.calendar.sets).toStrictEqual([]);
});

test("with the bridge: moving one occurrence, the bridge tells the guests", async ({
  page,
}) => {
  jmap.calendar.bridge = true;
  ourSeries("ser3", "Team lunch", { allDay: false, time: "12:00" });
  await page.goto("/calendar");
  const lunch = eventButton(page, "Team lunch");
  await expect(lunch).toBeVisible();
  await lunch.focus();
  await page.keyboard.press("Alt+ArrowDown");
  await page.getByRole("button", { name: "This event" }).click();
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      sendSchedulingMessages: true,
      update: { "ser3-occ": { start: `${todayUtc()}T12:15:00` } },
    });
  // Give a stray email time to show up: none does.
  await page.waitForTimeout(500);
  expect(jmap.calendar.submissions).toStrictEqual([]);
  expect(jmap.calendar.emails).toStrictEqual([]);
});

test("share a calendar: add someone, change their access, remove them", async ({
  page,
}, info) => {
  jmap.directory = true;
  const [own] = jmap.calendar.calendars;
  if (own !== undefined) {
    // Set by another client: shown as is ("Custom"), never rewritten.
    own.shareWith = { pb: { mayReadItems: true, mayWriteOwn: true } };
  }
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  const side = await sidebar(page);
  // Bob's calendar doesn't let us share it (`mayShare` false).
  await side.getByRole("button", { name: "Options for Team calendar" }).click();
  await expect(page.getByRole("menuitem", { name: "Colour…" })).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Share…" })).toHaveCount(0);
  await page.keyboard.press("Escape");

  await side
    .getByRole("button", {
      name: "Options for Stalwart Calendar (alice@nebulamail.test)",
    })
    .click();
  await page.getByRole("menuitem", { name: "Share…" }).click();
  const dialog = page.getByRole("dialog", {
    name: "Share “Stalwart Calendar (alice@nebulamail.test)”",
  });
  await expectValue(dialog.getByLabel("Access for Bob Pérez"), "custom");
  const fresh = dialog.getByLabel("Access for people you add");
  await expectValue(fresh, "read");
  expect(await optionNames(fresh)).toStrictEqual([
    "Can see free/busy only",
    "Can view events",
    "Can view and edit",
    "Can edit and share",
  ]);
  await dialog.getByLabel("Search people by name or address").fill("eve");
  await dialog.getByRole("button", { exact: true, name: "Share" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.update)
    .toStrictEqual({
      cal1: {
        "shareWith/pe": {
          mayDelete: false,
          mayReadFreeBusy: true,
          mayReadItems: true,
          mayRSVP: false,
          mayShare: false,
          mayUpdatePrivate: false,
          mayWriteAll: false,
          mayWriteOwn: false,
        },
      },
    });
  await expectValue(dialog.getByLabel("Access for Eve Núñez"), "read");
  await shot(page, info, "share-calendar");
  await expectNoSidewaysScroll(page);

  await chooseOption(dialog.getByLabel("Access for Eve Núñez"), {
    label: "Can view and edit",
  });
  await expect
    .poll(() => own?.shareWith?.pe)
    .toMatchObject({ mayShare: false, mayWriteAll: true, mayWriteOwn: true });
  await expectValue(dialog.getByLabel("Access for Eve Núñez"), "write");
  // Bob's custom access stays untouched.
  expect(own?.shareWith?.pb).toStrictEqual({
    mayReadItems: true,
    mayWriteOwn: true,
  });

  await dialog
    .getByRole("button", { name: "Stop sharing with Eve Núñez" })
    .click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.update)
    .toStrictEqual({ cal1: { "shareWith/pe": null } });
  await expect(dialog.getByText("Eve Núñez")).toHaveCount(0);
  await expect(dialog.getByText("Bob Pérez")).toBeVisible();
});

test("share a calendar with a group: found by name, shown as a group", async ({
  page,
}) => {
  jmap.directory = true;
  const [own] = jmap.calendar.calendars;
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  const side = await sidebar(page);
  await side
    .getByRole("button", {
      name: "Options for Stalwart Calendar (alice@nebulamail.test)",
    })
    .click();
  await page.getByRole("menuitem", { name: "Share…" }).click();
  const dialog = page.getByRole("dialog", {
    name: "Share “Stalwart Calendar (alice@nebulamail.test)”",
  });
  // A group (a Nextcloud one through the bridge): no address, only the
  // server's search finds it, and it says it is a group.
  await dialog.getByLabel("Search people by name or address").fill("staf");
  const offered = dialog.getByRole("listitem").filter({ hasText: "Staff" });
  await expect(offered).toContainText("Group");
  await offered.getByRole("button", { exact: true, name: "Share" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.update)
    .toMatchObject({
      cal1: { "shareWith/pg": { mayReadItems: true, mayWriteAll: false } },
    });
  // Read back: not in the directory, so described by the server, still a
  // group, with the group's access.
  await expect.poll(() => own?.shareWith?.pg).toBeDefined();
  const row = dialog.getByRole("listitem").filter({ hasText: "Staff" });
  await expect(row).toContainText("Group");
  await expectValue(dialog.getByLabel("Access for Staff"), "read");
  await dialog.getByRole("button", { name: "Stop sharing with Staff" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.update)
    .toStrictEqual({ cal1: { "shareWith/pg": null } });
  await expect(dialog.getByText("Staff")).toHaveCount(0);
});

test("without directory queries the share dialog keeps the shares and says why nobody can be added", async ({
  page,
}) => {
  // Stalwart 1.0 by default: `Principal/query` is forbidden to users.
  jmap.directoryClosed = true;
  const [own] = jmap.calendar.calendars;
  if (own !== undefined) {
    own.shareWith = { pb: { mayReadItems: true, mayWriteOwn: true } };
  }
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  const side = await sidebar(page);
  await side
    .getByRole("button", {
      name: "Options for Stalwart Calendar (alice@nebulamail.test)",
    })
    .click();
  await page.getByRole("menuitem", { name: "Share…" }).click();
  const dialog = page.getByRole("dialog", {
    name: "Share “Stalwart Calendar (alice@nebulamail.test)”",
  });
  await expectValue(dialog.getByLabel("Access for Bob Pérez"), "custom");
  await expect(
    dialog.getByText("This server doesn't let you look people up.", {
      exact: false,
    })
  ).toBeVisible();
  await expect(
    dialog.getByLabel("Search people by name or address")
  ).toBeDisabled();
  // What is shared can still be taken back.
  await dialog
    .getByRole("button", { name: "Stop sharing with Bob Pérez" })
    .click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1)?.update)
    .toStrictEqual({ cal1: { "shareWith/pb": null } });
});

test("a new calendar shared from the start, also with a group only the server finds", async ({
  page,
}) => {
  jmap.directory = true;
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  const side = await sidebar(page);
  await side.getByRole("button", { name: "New calendar" }).click();
  const dialog = page.getByRole("dialog", { name: "New calendar" });
  await dialog.getByRole("textbox", { name: "Name" }).fill("Equipo");
  await expect(dialog.getByText("Only you will see it.")).toBeVisible();
  const search = dialog.getByLabel("Search people by name or address");
  await search.fill("eve");
  await dialog.getByRole("button", { exact: true, name: "Share" }).click();
  await expectValue(dialog.getByLabel("Access for Eve Núñez"), "read");
  // Not in the directory: found by the server (like a Nextcloud group).
  await search.fill("staf");
  await expect(dialog.getByText("Staff")).toBeVisible();
  await chooseOption(dialog.getByLabel("Access for people you add"), {
    label: "Can view and edit",
  });
  await dialog.getByRole("button", { exact: true, name: "Share" }).click();
  await expectValue(dialog.getByLabel("Access for Staff"), "write");
  // Nothing goes to the server until the calendar is created.
  expect(
    jmap.calendar.calendarSets.filter((set) => set.update !== undefined)
  ).toHaveLength(0);
  await dialog.getByRole("button", { name: "Create" }).click();
  await expect
    .poll(() => jmap.calendar.calendarSets.at(-1))
    .toMatchObject({
      create: {
        calendar: {
          name: "Equipo",
          shareWith: {
            pe: { mayReadItems: true, mayWriteAll: false },
            pg: { mayReadItems: true, mayWriteAll: true },
          },
        },
      },
    });
  await expect(side.getByText("Equipo", { exact: true })).toBeVisible();
});

test("share a calendar from ⌘K; hidden when the server doesn't allow it", async ({
  page,
}) => {
  jmap.directory = true;
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  await page.keyboard.press("Control+k");
  await page
    .getByRole("option", {
      name: /Share calendar “Stalwart Calendar \(alice@nebulamail\.test\)”/u,
    })
    .click();
  const dialog = page.getByRole("dialog", {
    name: "Share “Stalwart Calendar (alice@nebulamail.test)”",
  });
  await expect(
    dialog.getByText("Only you can see this calendar.")
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);

  // A server without sharing (Radicale through the bridge): no mayShare.
  const [own] = jmap.calendar.calendars;
  if (own !== undefined) {
    own.myRights = { ...own.myRights, mayShare: false };
  }
  await page.reload();
  await expect(eventButton(page, "Standup")).toBeVisible();
  await page.keyboard.press("Control+k");
  await expect(page.getByRole("option", { name: /Today/u })).toBeVisible();
  await expect(
    page.getByRole("option", { name: /Share calendar/u })
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  const side = await sidebar(page);
  await side
    .getByRole("button", {
      name: "Options for Stalwart Calendar (alice@nebulamail.test)",
    })
    .click();
  await expect(
    page.getByRole("menuitem", { name: "Calendar settings…" })
  ).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Share…" })).toHaveCount(0);
});
