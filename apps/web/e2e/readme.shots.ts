// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// README screenshots (playwright.shots.config.ts): a demo mailbox on the
// fake JMAP server, light and dark, desktop and phone.

import path from "node:path";

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { FakeEmail } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

const OUT = path.join(import.meta.dirname, "../../../docs/images");
const MINUTE = 60_000;

const ago = (minutes: number): string =>
  new Date(Date.now() - minutes * MINUTE).toISOString();

const LAUNCH_HTML = `
<p>Hi team,</p>
<p>The new onboarding flow is ready for a last look before we ship it on
Thursday. Highlights:</p>
<ul>
  <li><strong>Sign-up in two steps</strong> instead of five</li>
  <li>Workspace templates for design, engineering and support</li>
  <li>A checklist that disappears once you're done</li>
</ul>
<p>The staging build is up. Could you each try it with a fresh account and
leave notes in the doc by Wednesday noon?</p>
<p>Thanks!<br>Maya</p>`;

const demoEmails = (): FakeEmail[] => [
  {
    from: { email: "maya@northwind.example", name: "Maya Lindqvist" },
    html: LAUNCH_HTML,
    id: "d1",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: ago(12),
    subject: "Onboarding redesign: final review before Thursday",
  },
  {
    attachment: "Q4-budget.xlsx",
    from: { email: "daniel@northwind.example", name: "Daniel Okafor" },
    html: "<p>Here's the updated Q4 budget with the travel line split out.</p>",
    id: "d2",
    keywords: {},
    mailboxIds: { inbox: true },
    receivedAt: ago(48),
    subject: "Q4 budget, updated numbers",
  },
  {
    from: { email: "hello@fieldnotes.example", name: "Field Notes Weekly" },
    html: "<p>This week: quiet software, slow mornings and a reading list.</p>",
    id: "d3",
    keywords: { $seen: true },
    listId: "Field Notes <weekly.fieldnotes.example>",
    mailboxIds: { inbox: true },
    receivedAt: ago(95),
    subject: "Issue 112: The case for quiet software",
  },
  {
    from: { email: "lucia@studio-alba.example", name: "Lucía Ferrer" },
    html: "<p>Attaching the three logo directions we discussed. My favourite is B.</p>",
    id: "d4",
    keywords: { $flagged: true, $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: ago(180),
    subject: "Logo directions for the spring campaign",
  },
  {
    from: { email: "billing@cloudhost.example", name: "CloudHost Billing" },
    html: "<p>Your invoice for October is ready. Total: €42.00.</p>",
    id: "d5",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: ago(320),
    subject: "Your October invoice",
  },
  {
    from: { email: "tom@northwind.example", name: "Tom Becker" },
    html: "<p>Lunch on Friday? The new ramen place opened next door.</p>",
    id: "d6",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: ago(60 * 22),
    subject: "Friday lunch?",
  },
  {
    from: { email: "noreply@trains.example", name: "Rail Europe Tickets" },
    html: "<p>Your booking for Berlin → Copenhagen on 14 November is confirmed.</p>",
    id: "d7",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: ago(60 * 26),
    subject: "Booking confirmed: Berlin → Copenhagen",
  },
  {
    from: { email: "priya@northwind.example", name: "Priya Raman" },
    html: "<p>Notes from today's retro are in the shared doc.</p>",
    id: "d8",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: ago(60 * 30),
    subject: "Sprint 42 retro notes",
  },
  {
    from: { email: "events@devconf.example", name: "DevConf" },
    html: "<p>Your talk slot is confirmed: Hall B, 11:30.</p>",
    id: "d9",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: ago(60 * 50),
    subject: "Speaker info: your slot is confirmed",
  },
  {
    from: { email: "jonas@northwind.example", name: "Jonas Weber" },
    html: "<p>Can you approve the access request for the analytics project?</p>",
    id: "d10",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: ago(60 * 74),
    subject: "Access request: analytics project",
  },
];

/** "YYYY-MM-DD" of `offset` days from today, in UTC. */
const dayFromToday = (offset: number): string =>
  new Date(Date.now() + offset * 86_400_000).toISOString().slice(0, 10);

/** An instant as JMAP writes it, without milliseconds. */
const iso = (ms: number): string =>
  new Date(ms).toISOString().replace(".000", "");

/** A one-off event this week, in UTC. */
const event = (
  id: string,
  title: string,
  offset: number,
  time: string,
  minutes: number
) => {
  const day = dayFromToday(offset);
  const start = Date.parse(`${day}T${time}:00Z`);
  return {
    "@type": "Event",
    calendarIds: { cal1: true },
    duration:
      `PT${Math.floor(minutes / 60)}H${minutes % 60 === 0 ? "" : `${minutes % 60}M`}`.replace(
        "PT0H",
        "PT"
      ),
    id,
    start: `${day}T${time}:00`,
    timeZone: "Etc/UTC",
    title,
    uid: `${id}@demo.example`,
    utcEnd: iso(start + minutes * MINUTE),
    utcStart: iso(start),
  };
};

const demo = async (page: Page): Promise<void> => {
  const jmap = await installFakeJmap(page);
  jmap.emails.splice(0, jmap.emails.length, ...demoEmails());
  const [own] = jmap.calendar.calendars;
  if (own !== undefined) {
    own.name = "Personal";
  }
  jmap.calendar.events.push(
    event("x1", "Onboarding review", 1, "13:00", 90),
    event("x2", "1:1 with Daniel", -1, "09:30", 30),
    event("x3", "Focus time", -1, "11:00", 120),
    event("x4", "Customer call: Alba Studio", 2, "15:00", 60),
    event("x5", "Lunch with Tom", 3, "11:30", 60),
    event("x6", "Sprint planning", -1, "14:00", 60),
    event("x7", "Gym", 2, "17:00", 60),
    event("x8", "Train to Copenhagen", 4, "09:00", 270)
  );
};

const shot = async (page: Page, name: string): Promise<void> => {
  // Let fonts, avatars and transitions settle.
  await page.waitForTimeout(600);
  await page.screenshot({ path: path.join(OUT, `${name}.png`) });
};

test.describe("desktop", () => {
  test.use({ deviceScaleFactor: 2, viewport: { height: 820, width: 1360 } });

  test("inbox with the reader, light", async ({ context, page }) => {
    await signIn(context);
    await demo(page);
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/mail/inbox/d1");
    await expect(
      page.getByRole("heading", { name: /Onboarding redesign/u })
    ).toBeVisible();
    await shot(page, "inbox-light");
  });

  test("inbox with the reader, dark", async ({ context, page }) => {
    await signIn(context);
    await demo(page);
    await page.emulateMedia({ colorScheme: "dark" });
    await page.goto("/mail/inbox/d1");
    await expect(
      page.getByRole("heading", { name: /Onboarding redesign/u })
    ).toBeVisible();
    await shot(page, "inbox-dark");
  });

  test("command palette", async ({ context, page }) => {
    await signIn(context);
    await demo(page);
    await page.emulateMedia({ colorScheme: "dark" });
    await page.goto("/mail/inbox");
    await expect(page.getByText("Q4 budget, updated numbers")).toBeVisible();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(page.getByRole("dialog")).toBeVisible();
    await shot(page, "command-palette");
  });

  test("calendar", async ({ context, page }) => {
    await signIn(context);
    await demo(page);
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/calendar");
    await expect(page.getByRole("main")).toBeVisible();
    await shot(page, "calendar");
  });
});

test.describe("phone", () => {
  test.use({
    deviceScaleFactor: 3,
    hasTouch: true,
    isMobile: true,
    viewport: { height: 800, width: 390 },
  });

  test("inbox on a phone", async ({ context, page }) => {
    await signIn(context);
    await demo(page);
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/mail/inbox");
    await expect(page.getByText("Q4 budget, updated numbers")).toBeVisible();
    await shot(page, "phone");
  });
});
