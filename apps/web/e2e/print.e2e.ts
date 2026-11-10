// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Printing: the app prints from a hidden iframe with the
// sanitized email, a header made of DOM nodes and the print sandbox. The
// test replaces `print()` in that frame (before the app calls it) and
// records what would have been printed. Also in Firefox and WebKit
// (playwright.config.ts): Firefox's print() returns before printing, so
// the frame must stay until `afterprint`; and the printed document looks
// the same in each engine with print media.

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName === "chromium" &&
      !["desktop", "phone-412"].includes(info.project.name),
    "one desktop and one phone run are enough (and Firefox and WebKit)"
  );
});

const Printed = z.object({
  csp: z.string().nullable(),
  handlers: z.number(),
  header: z.string(),
  html: z.string(),
  sandbox: z.string().nullable(),
  scripts: z.number(),
  text: z.string(),
});

/**
 * Replaces print() of the print frame, before the app calls it, with one
 * that records what would be printed.
 */
const recordPrints = async (
  page: Page,
  mode: "blocking" | "later"
): Promise<void> => {
  // Runs in the app's page (the sandboxed frames run no scripts at all).
  await page.addInitScript((how) => {
    const printed: unknown[] = [];
    const messages: unknown[] = [];
    Reflect.set(globalThis, "nmPrinted", printed);
    Reflect.set(globalThis, "nmMessages", messages);
    globalThis.addEventListener("message", (event) => {
      messages.push(event.data);
    });
    // `load` doesn't bubble, but capturing listeners see it first: this
    // swaps print() before the app's own listener calls it.
    document.addEventListener(
      "load",
      (event) => {
        const frame = event.target;
        if (!(frame instanceof HTMLIFrameElement)) {
          return;
        }
        const win = frame.contentWindow;
        if (win === null) {
          return;
        }
        win.print = () => {
          const doc = frame.contentDocument;
          printed.push({
            csp:
              doc
                ?.querySelector('meta[http-equiv="Content-Security-Policy"]')
                ?.getAttribute("content") ?? null,
            handlers: [...(doc?.querySelectorAll("*") ?? [])].filter(
              (element) =>
                element
                  .getAttributeNames()
                  .some((name) => name.toLowerCase().startsWith("on"))
            ).length,
            header: doc?.querySelector("body > header")?.textContent ?? "",
            sandbox: frame.getAttribute("sandbox"),
            scripts: doc?.querySelectorAll("script").length ?? -1,
            text: doc?.body.textContent ?? "",
            // The printed document (sanitized, no scripts): drawn again
            // with print media below.
            html: doc?.documentElement.outerHTML ?? "",
          });
          // Chromium and Safari: the dialog has closed when print()
          // returns. Firefox: print() returns first, its preview comes
          // later (the test fires afterprint itself).
          if (how === "blocking") {
            win.dispatchEvent(new Event("beforeprint"));
            win.dispatchEvent(new Event("afterprint"));
          }
        };
      },
      true
    );
  }, mode);
};

const read = async (page: Page, name: string): Promise<unknown> =>
  await page.evaluate((key): unknown => {
    const value: unknown = Reflect.get(globalThis, key);
    return value;
  }, name);

const printFrom = async (page: Page, id: string, subject: string) => {
  await page.goto(`/mail/inbox/${id}`);
  const reader = page.getByRole("region", { name: "Message" });
  await expect(reader.getByRole("heading", { name: subject })).toBeVisible();
  await reader.getByRole("button", { name: "More actions" }).first().click();
  await page.getByRole("menuitem", { name: "Print" }).click();
  await expect.poll(async () => await read(page, "nmPrinted")).toHaveLength(1);
  const [printed] = z.array(Printed).parse(await read(page, "nmPrinted"));
  return printed;
};

const printFrame = (page: Page) =>
  page.locator('iframe[sandbox*="allow-modals"]');

test("print: sanitized email, header, print sandbox, no scripts", async ({
  context,
  page,
}) => {
  await signIn(context);
  await installFakeJmap(page);
  await recordPrints(page, "blocking");

  const printed = await printFrom(page, "e16", "Print test");
  expect(printed?.sandbox).toBe("allow-same-origin allow-modals");
  expect(printed?.scripts).toBe(0);
  expect(printed?.handlers).toBe(0);
  expect(printed?.csp).toMatch(/default-src 'none'/u);
  expect(printed?.text).toContain("Print me, please.");
  // The header: subject, sender, recipient and date.
  expect(printed?.header).toContain("Print test");
  expect(printed?.header).toContain("Pat Printer");
  expect(printed?.header).toContain("jane@example.com");
  expect(printed?.header).toMatch(/2026/u);
  // The frame goes away once printed; nothing ran inside it.
  await expect(printFrame(page)).toHaveCount(0);
  expect(await read(page, "nmMessages")).toEqual([]);
});

test("a print() that returns at once (Firefox): the frame waits for afterprint", async ({
  context,
  page,
}) => {
  await signIn(context);
  await installFakeJmap(page);
  await recordPrints(page, "later");
  await printFrom(page, "e16", "Print test");

  // print() returned, the preview hasn't come yet: the frame stays.
  await page.waitForTimeout(1000);
  await expect(printFrame(page)).toHaveCount(1);
  await printFrame(page).evaluate((frame) => {
    if (frame instanceof HTMLIFrameElement) {
      frame.contentWindow?.dispatchEvent(new Event("beforeprint"));
      frame.contentWindow?.dispatchEvent(new Event("afterprint"));
    }
  });
  await expect(printFrame(page)).toHaveCount(0);
});

/** Paper width (A4) in CSS pixels. */
const A4 = { height: 1123, width: 794 };

test("the printout with print media: header first, the whole email, no overflow", async ({
  browserName,
  context,
  page,
}, info) => {
  await signIn(context);
  const jmap = await installFakeJmap(page);
  const paragraphs = Array.from(
    { length: 60 },
    (_, index) => `<p>Paragraph ${index + 1} of a long email.</p>`
  ).join("");
  jmap.emails.unshift({
    from: { email: ["pat", "example.com"].join("@"), name: "Pat Printer" },
    html: `<div style="color:#222">${paragraphs}<p id="last">The end.</p></div>`,
    id: "p-long",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: "2026-11-03T09:00:00Z",
    subject: "Long printout",
  });
  await recordPrints(page, "blocking");
  const printed = await printFrom(page, "p-long", "Long printout");

  const paper = await context.newPage();
  await paper.setViewportSize(A4);
  await paper.emulateMedia({ media: "print" });
  await paper.setContent(printed?.html ?? "");
  const layout = await paper.evaluate(() => {
    const root = document.documentElement;
    return {
      background: getComputedStyle(document.body).backgroundColor,
      fits: root.scrollWidth <= root.clientWidth,
      header:
        document.querySelector("body > header")?.getBoundingClientRect().top ??
        -1,
      last: document.querySelector("#last")?.getBoundingClientRect().top ?? -1,
      page: innerHeight,
    };
  });
  // The header (subject, from, to, date) comes before the email, and the
  // whole email is laid out (past the first page), not cut to a frame.
  expect(layout.header).toBeGreaterThanOrEqual(0);
  expect(layout.last).toBeGreaterThan(layout.header);
  expect(layout.last).toBeGreaterThan(layout.page);
  expect(layout.fits).toBe(true);
  // Light paper: nothing makes it dark.
  expect(layout.background).toMatch(/^rgba?\((?:0, 0, 0, 0|255, 255, 255)/u);
  await info.attach(`printout-${browserName}.png`, {
    body: await paper.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  if (browserName === "chromium") {
    // Only Chromium makes PDFs.
    const pdf = await paper.pdf({ format: "A4" });
    expect(pdf.subarray(0, 5).toString()).toBe("%PDF-");
  }
});

test("the browser's own print() reaches the frame", async ({
  browserName,
  context,
  page,
}) => {
  await signIn(context);
  await installFakeJmap(page);
  // print() is left alone: only its events are recorded.
  await page.addInitScript(() => {
    const events: string[] = [];
    Reflect.set(globalThis, "nmEvents", events);
    document.addEventListener(
      "load",
      (event) => {
        const win =
          event.target instanceof HTMLIFrameElement
            ? event.target.contentWindow
            : null;
        for (const type of ["beforeprint", "afterprint"]) {
          win?.addEventListener(type, () => {
            events.push(type);
          });
        }
      },
      true
    );
  });
  await page.goto("/mail/inbox/e16");
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Print test" })
  ).toBeVisible();
  await reader.getByRole("button", { name: "More actions" }).first().click();
  await page.getByRole("menuitem", { name: "Print" }).click();

  // Firefox prints after print() returns: with the frame removed at once
  // (before 2026-11-03) these never came.
  await expect
    .poll(async () => await read(page, "nmEvents"))
    .toStrictEqual(["beforeprint", "afterprint"]);
  // Headless Firefox stops the page's timers after printing (a real one
  // resumes them when its dialog closes), so React can't remove the
  // frame there; the test above covers it.
  if (browserName !== "firefox") {
    await expect(printFrame(page)).toHaveCount(0);
  }
});

test("«Print» again while the first is still waiting (Firefox's preview open) prints again", async ({
  context,
  page,
}) => {
  await signIn(context);
  await installFakeJmap(page);
  await recordPrints(page, "later");
  await printFrom(page, "e16", "Print test");
  // No afterprint yet: the first frame still waits. Before 2026-11-04 a
  // second click changed nothing (same email, same state).
  await page.waitForTimeout(500);
  const reader = page.getByRole("region", { name: "Message" });
  await reader.getByRole("button", { name: "More actions" }).first().click();
  await page.getByRole("menuitem", { name: "Print" }).click();
  await expect.poll(async () => await read(page, "nmPrinted")).toHaveLength(2);
  // A new frame took the old one's place.
  await expect(printFrame(page)).toHaveCount(1);
  await printFrame(page).evaluate((frame) => {
    if (frame instanceof HTMLIFrameElement) {
      frame.contentWindow?.dispatchEvent(new Event("afterprint"));
    }
  });
  await expect(printFrame(page)).toHaveCount(0);
});

test("Ctrl/⌘+P with an email open prints it through the print frame", async ({
  context,
  page,
}) => {
  await signIn(context);
  await installFakeJmap(page);
  await recordPrints(page, "blocking");
  await page.goto("/mail/inbox/e16");
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Print test" })
  ).toBeVisible();
  // Its body has loaded (the shortcut waits for it).
  await expect(
    reader.getByRole("button", { name: "More actions" }).first()
  ).toBeEnabled();
  await page.waitForLoadState("networkidle");
  await page.keyboard.press("ControlOrMeta+p");
  await expect.poll(async () => await read(page, "nmPrinted")).toHaveLength(1);
  const [printed] = z.array(Printed).parse(await read(page, "nmPrinted"));
  expect(printed?.header).toContain("Print test");
  expect(printed?.text).toContain("Print me, please.");
  await expect(printFrame(page)).toHaveCount(0);
});

test("the app page with print media shows only the open email", async ({
  browserName,
  context,
  page,
}, info) => {
  await signIn(context);
  const jmap = await installFakeJmap(page);
  const paragraphs = Array.from(
    { length: 80 },
    (_, index) => `<p>Paragraph ${index + 1} of a long email.</p>`
  ).join("");
  jmap.emails.unshift({
    from: { email: ["pat", "example.com"].join("@"), name: "Pat Printer" },
    html: `<div>${paragraphs}<p id="last">The end.</p></div>`,
    id: "p-menu",
    keywords: { $seen: true },
    mailboxIds: { inbox: true },
    receivedAt: "2026-11-03T09:00:00Z",
    subject: "Printed from the menu",
  });
  await page.goto("/mail/inbox/p-menu");
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Printed from the menu" })
  ).toBeVisible();
  const body = reader.locator("iframe[title]").first();
  await expect
    .poll(async () => {
      const box = await body.boundingBox();
      return box?.height ?? 0;
    })
    .toBeGreaterThan(1000);
  await page.emulateMedia({ media: "print" });

  // The subject, the sender and the body stay; the app around them goes.
  await expect(
    reader.getByRole("heading", { name: "Printed from the menu" })
  ).toBeVisible();
  await expect(reader.getByText("Pat Printer").first()).toBeVisible();
  await expect(body).toBeVisible();
  const visible = await page.evaluate(() => {
    // oxlint-disable-next-line unicorn/consistent-function-scoping -- this function runs in the page: it can only use what is defined inside it.
    const shown = (element: Element): boolean =>
      element.checkVisibility() && element.getBoundingClientRect().height > 0;
    const email = document.querySelector("[data-print-email]");
    return {
      buttons: [...document.querySelectorAll("button")].filter(shown).length,
      navigation: [
        ...document.querySelectorAll("nav, [role=navigation]"),
      ].filter(shown).length,
      outside: [...document.body.querySelectorAll("*")].filter(
        (element) =>
          shown(element) &&
          email !== null &&
          !email.contains(element) &&
          !element.contains(email)
      ).length,
      // Not cut to the window: the whole conversation is laid out.
      tall: (email?.getBoundingClientRect().height ?? 0) > innerHeight,
    };
  });
  expect(visible).toStrictEqual({
    buttons: 0,
    navigation: 0,
    outside: 0,
    tall: true,
  });
  await info.attach(`app-print-${browserName}.png`, {
    body: await page.screenshot({ fullPage: true }),
    contentType: "image/png",
  });
  if (browserName === "chromium") {
    // The long body goes on to more than one page (not cut at the first).
    const printout = await page.pdf({ format: "A4" });
    const pdf = printout.toString("latin1");
    const pages = pdf.match(/\/Type\s*\/Page(?!s)/gu)?.length ?? 0;
    info.annotations.push({ description: String(pages), type: "pdf pages" });
    expect(pages).toBeGreaterThan(1);
  }
});
