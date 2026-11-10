// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The calendar at phone, tablet and desktop widths, with the
// fake JMAP server: views and keyboard, creating and moving events, the
// safety of event descriptions, and invitations in the mail reader.
// Screenshots go to /tmp/nebula-calendar (outside the repo).

import type { Locator, Page, TestInfo } from "@playwright/test";
import { expect, test } from "@playwright/test";

import {
  INVITATION_BLOB,
  INVITATION_UID,
  TASK_ID,
  TASK_TITLE,
  todayUtc,
} from "./fake-calendar";
import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

// Event times in the fake server are UTC.
test.use({ timezoneId: "UTC" });

const PHONE_MAX = 640;

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

/** Every visible button in `area` is at least 44 px high (touch). */
const expectTallButtons = async (area: Locator) => {
  const heights = await area.getByRole("button").evaluateAll((buttons) =>
    buttons
      .map((button) => ({
        height: button.getBoundingClientRect().height,
        label: button.getAttribute("aria-label") ?? button.textContent,
      }))
      .filter((box) => box.height > 0)
  );
  expect(heights.length).toBeGreaterThan(0);
  for (const box of heights) {
    expect.soft(box.height, `${box.label} height`).toBeGreaterThanOrEqual(44);
  }
};

const eventButton = (page: Page, title: string): Locator =>
  page.getByRole("button", { name: new RegExp(`^${title}, `, "u") }).first();

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

test("week view: today's events, all-day row, fits the screen", async ({
  page,
}, info) => {
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  await expect(eventButton(page, "Holiday")).toBeVisible();
  await expect(eventButton(page, "Design review")).toBeVisible();
  await shot(page, info, "week");
  await expectNoSidewaysScroll(page);
  if (isTouch(info)) {
    await expectTallButtons(page.getByRole("banner"));
  }
});

test("week view: day headers line up with their columns", async ({ page }) => {
  test.skip(isPhone(page), "the phone layout shows one day");
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  // Headless Chromium hides scrollbars, so the misalignment a real one
  // causes can't show here. What prevents it: the day headers scroll in the
  // same container as the columns, so a scrollbar narrows both.
  const layout = await page.evaluate(() => {
    // oxlint-disable-next-line unicorn/consistent-function-scoping -- page.evaluate runs this in the browser: it can't use functions from outside.
    const scroller = (element: Element | null | undefined) => {
      let node = element?.parentElement ?? null;
      while (node !== null && getComputedStyle(node).overflowY !== "auto") {
        node = node.parentElement;
      }
      return node;
    };
    const header = [...document.querySelectorAll("button[aria-label]")].find(
      (button) =>
        button.getAttribute("aria-label")?.startsWith("Open ") === true
    );
    const date = header?.getAttribute("aria-label")?.slice("Open ".length);
    const column = [...document.querySelectorAll("section[aria-label]")].find(
      (section) => section.getAttribute("aria-label") === date
    );
    const columnBox = column?.getBoundingClientRect();
    const headerBox = header?.parentElement?.getBoundingClientRect();
    return {
      aligned:
        columnBox !== undefined &&
        headerBox !== undefined &&
        Math.abs(columnBox.left - headerBox.left) <= 1 &&
        Math.abs(columnBox.right - headerBox.right) <= 1,
      sameScroller:
        header !== undefined && scroller(header) === scroller(column),
    };
  });
  expect(layout).toStrictEqual({ aligned: true, sameScroller: true });
});

test("a long calendar name ends in … inside the sidebar", async ({ page }) => {
  test.skip(isPhone(page), "the phone layout has no calendar sidebar");
  await page.goto("/calendar");
  const name = page.getByTitle("Stalwart Calendar (alice@nebulamail.test)");
  await expect(name).toBeVisible();
  const fit = await name.evaluate((element) => {
    const nav = element.closest("nav");
    const { right } = element.getBoundingClientRect();
    return {
      cut: element.scrollWidth > element.clientWidth,
      inside: nav !== null && right <= nav.getBoundingClientRect().right,
    };
  });
  expect(fit).toStrictEqual({ cut: true, inside: true });
});

test("a task in the calendar is not shown as an event", async ({ page }) => {
  // The fake calendar holds a task due today (a VTODO from another
  // client), returned by the same query as the events.
  // With "Show tasks" (on by default) it shows as a task, never an event.
  for (const view of ["week", "day", "month", "agenda"]) {
    // oxlint-disable-next-line no-await-in-loop -- one view after another.
    await page.goto(`/calendar?view=${view}`);
    // oxlint-disable-next-line no-await-in-loop -- same.
    await expect(eventButton(page, "Standup")).toBeVisible();
    // oxlint-disable-next-line no-await-in-loop -- same.
    await expect(eventButton(page, TASK_TITLE)).toHaveCount(0);
    // oxlint-disable-next-line no-await-in-loop -- same.
    await expect(
      page.getByRole("button", { name: `Task: ${TASK_TITLE}` })
      // Today's month cell is full of events: it is in "+N more".
    ).toHaveCount(view === "month" ? 0 : 1);
  }
  // Nor by its id in the URL.
  await page.goto(`/calendar?event=${TASK_ID}`);
  await expect(page.getByText("This event no longer exists.")).toBeVisible();
  await expect(eventButton(page, TASK_TITLE)).toHaveCount(0);
});

test("keyboard: views, next, today", async ({ page }) => {
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  await page.keyboard.press("m");
  await expect(page).toHaveURL(/view=month/u);
  await expect(eventButton(page, "Standup")).toBeVisible();
  await page.keyboard.press("a");
  await expect(page).toHaveURL(/view=agenda/u);
  await page.keyboard.press("j");
  await expect(page).toHaveURL(/date=/u);
  await page.keyboard.press("t");
  await expect(page).not.toHaveURL(/date=/u);
  await page.keyboard.press("w");
  // Week is the default view, so the URL drops it; phones show one day by
  // default, so there a week is written out.
  await (isPhone(page)
    ? expect(page).toHaveURL(/view=week/u)
    : expect(page).not.toHaveURL(/view=/u));
});

test("create an event with C and Ctrl/⌘ + Enter", async ({ page }, info) => {
  await page.goto("/calendar");
  await expect(eventButton(page, "Standup")).toBeVisible();
  await page.keyboard.press("c");
  const title = page.getByRole("textbox", { name: "Title" });
  await expect(title).toBeFocused();
  await title.fill("Lunch with Ana");
  await shot(page, info, "editor");
  await expectNoSidewaysScroll(page);
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      create: {
        event: {
          calendarIds: { cal1: true },
          duration: "PT1H",
          showWithoutTime: false,
          timeZone: "UTC",
          title: "Lunch with Ana",
        },
      },
      sendSchedulingMessages: false,
    });
  await expect(
    page.getByRole("heading", { name: "Lunch with Ana" })
  ).toBeVisible();
});

test("event details: HTML description sanitized, links confirmed", async ({
  page,
}, info) => {
  const loaded: string[] = [];
  page.on("request", (request) => {
    if (request.url().includes("tracker.test")) {
      loaded.push(request.url());
    }
  });
  let pwned = false;
  await page.exposeFunction("reportPwned", () => {
    pwned = true;
  });
  await page.addInitScript(() => {
    globalThis.addEventListener("message", (event) => {
      const report: unknown = Reflect.get(globalThis, "reportPwned");
      if (event.data === "pwned" && typeof report === "function") {
        Reflect.apply(report, globalThis, []);
      }
    });
  });
  await page.goto("/calendar");
  await eventButton(page, "Design review").click();
  const panel = page.getByRole("region", { name: "Event" });
  await expect(
    panel.getByRole("heading", { name: "Design review" })
  ).toBeVisible();
  // Bidi controls are stripped from untrusted text.
  await expect(panel.getByText("Room 1", { exact: true })).toBeVisible();
  const frame = panel.locator("iframe");
  await expect(frame).toHaveAttribute("sandbox", "allow-same-origin");
  const html = await frame.evaluate(
    (element: HTMLIFrameElement) =>
      element.contentDocument?.body.innerHTML ?? ""
  );
  expect(html).toContain("Agenda");
  expect(html).not.toContain("<script");
  expect(html).not.toContain("tracker.test");
  await shot(page, info, "details");
  await panel.getByRole("button", { name: "Join: Video" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("meet.example").first()).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel" }).click();
  expect(loaded).toStrictEqual([]);
  expect(pwned).toBeFalsy();
});

test("move an event with Alt + ↓ (no dragging needed)", async ({ page }) => {
  await page.goto("/calendar");
  const standup = eventButton(page, "Standup");
  await expect(standup).toBeVisible();
  await standup.focus();
  await page.keyboard.press("Alt+ArrowDown");
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({ update: { ev1: { start: `${todayUtc()}T08:15:00` } } });
});

test("invitation: the card answers and emails a REPLY, adding nothing before", async ({
  page,
}, info) => {
  jmap.emails.unshift({
    calendarBlob: INVITATION_BLOB,
    from: { email: "carol@partner.example", name: "Carol" },
    html: "<p>Carol invited you to the board meeting.</p>",
    id: "inv1",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: new Date().toISOString(),
    subject: "Invitation: Board meeting",
  });
  await page.goto("/mail/inbox");
  await page.getByText("Invitation: Board meeting").first().click();
  const card = page.getByRole("region", { name: "Calendar invitation" });
  await expect(
    card.getByRole("heading", { name: "Board meeting" })
  ).toBeVisible();
  // It overlaps "Design review" (13:00–14:00).
  await expect(card.getByText("Design review")).toBeVisible();
  await shot(page, info, "invitation");
  await expectNoSidewaysScroll(page);
  // Opening the email added nothing to the calendar.
  expect(jmap.calendar.sets).toStrictEqual([]);

  await card.getByRole("button", { name: "Yes" }).click();
  await expect
    .poll(() => jmap.calendar.sets.at(-1))
    .toMatchObject({
      create: {
        event: {
          calendarIds: { cal1: true },
          participants: { j: { participationStatus: "accepted" } },
          uid: INVITATION_UID,
        },
      },
      sendSchedulingMessages: false,
    });
  await expect.poll(() => jmap.calendar.submissions.length).toBe(1);
  const reply = jmap.calendar.emails.at(-1);
  expect(reply).toMatchObject({
    bodyStructure: {
      subParts: [
        { type: "text/plain" },
        { type: "text/calendar; method=REPLY" },
      ],
    },
    subject: "Accepted: Board meeting",
    to: [{ email: "carol@partner.example" }],
  });
  const ics = JSON.stringify(reply?.bodyValues);
  expect(ics).toContain("METHOD:REPLY");
  expect(ics).toContain("PARTSTAT=ACCEPTED");
  expect(ics).toContain(`UID:${INVITATION_UID}`);
  if (isPhone(page)) {
    await expectNoSidewaysScroll(page);
  }
});

test("deleting an event with more than 20 guests offers to skip the notice", async ({
  page,
}) => {
  const day = todayUtc();
  const guests = Object.fromEntries(
    Array.from({ length: 25 }, (_, index) => [
      `g${String(index)}`,
      {
        "@type": "Participant",
        calendarAddress: `mailto:guest${String(index)}@example.com`,
        participationStatus: "needs-action",
        roles: { attendee: true },
      },
    ])
  );
  jmap.calendar.events.push({
    "@type": "Event",
    calendarIds: { cal1: true },
    duration: "PT1H",
    id: "big1",
    organizerCalendarAddress: "mailto:[EMAIL]",
    participants: {
      ...guests,
      o: {
        "@type": "Participant",
        calendarAddress: "mailto:[EMAIL]",
        name: "Jane",
        participationStatus: "accepted",
        roles: { attendee: true, owner: true },
      },
    },
    start: `${day}T17:00:00`,
    timeZone: "Etc/UTC",
    title: "All hands",
    uid: "[EMAIL]",
    utcEnd: `${day}T18:00:00Z`,
    utcStart: `${day}T17:00:00Z`,
  });
  await page.goto("/calendar");
  await eventButton(page, "All hands").click();
  const panel = page.getByRole("region", { name: "Event" });
  await panel.getByRole("button", { name: "Delete event" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete event" })
    .click();
  // The bridge refuses to mail more than 20 cancellations: nothing is
  // deleted, and the app offers to delete without telling the guests.
  const notice = page.getByRole("region", { name: "Notifications" });
  await expect(notice.getByText("Too many guests to notify")).toBeVisible();
  expect(jmap.calendar.events.some((event) => event.id === "big1")).toBe(true);
  // The toast is aria-hidden for Playwright's role queries: found by text.
  await notice.getByText("Delete without notifying").click();
  await expect
    .poll(() => jmap.calendar.events.some((event) => event.id === "big1"))
    .toBe(false);
  expect(jmap.calendar.sets.at(-1)).toMatchObject({
    destroy: ["big1"],
    sendSchedulingMessages: false,
  });
  await expect(eventButton(page, "All hands")).toHaveCount(0);
});
