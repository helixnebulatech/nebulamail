// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Phase 2 in the browser: the contacts page at phone, tablet
// and desktop widths, the storage bar, and a folder shared by another
// account (its email ids repeat ours: the caches must keep them apart).
// Also an address book shared with us (read only, a card id that repeats
// ours, a Hebrew-calendar birthday), sharing ours, and birthdays without
// a year.
// Screenshots go to /tmp/nebula-mobile (outside the repo).

import { readFile } from "node:fs/promises";

import type { Page, TestInfo } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { FakeJmap } from "./fake-jmap";
import { SHARED_ACCOUNT, installFakeJmap } from "./fake-jmap";
import { chooseOption, expectValue } from "./select";
import { signIn } from "./session";

const PHONE_MAX = 640;
const NARROW_MAX = 1000;

const shot = async (page: Page, info: TestInfo, name: string) => {
  await page.screenshot({
    path: `/tmp/nebula-mobile/${info.project.name}/${name}.png`,
  });
};

const width = (page: Page): number => page.viewportSize()?.width ?? 0;

const expectNoSidewaysScroll = async (page: Page) => {
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth
  );
  expect(overflow).toBeLessThanOrEqual(0);
};

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

test("contacts: list, search and open a contact", async ({ page }, info) => {
  await page.goto("/contacts");
  const list = page.getByRole("list", { name: "Contacts list" });
  await expect(
    list.getByRole("button", { name: /Carla Gómez/u })
  ).toBeVisible();
  // Ours (Bob, Carla, Team) and Dana from Bob's shared address book.
  await expect(list.getByRole("button")).toHaveCount(4);
  await shot(page, info, "contacts");
  await expectNoSidewaysScroll(page);

  // Partial words match (Stalwart's own search wouldn't).
  await page.getByLabel("Search contacts").fill("pér");
  await expect(list.getByRole("button")).toHaveCount(1);
  await list.getByRole("button", { name: /Bob Pérez/u }).click();

  const detail = page.getByRole("region", { name: "Contact" });
  await expect(
    detail.getByRole("heading", { name: "Bob Pérez" })
  ).toBeVisible();
  // Notes keep their lines and are plain text.
  await expect(detail.getByText("Line one")).toBeVisible();
  await expect(detail.getByText("ACME")).toBeVisible();
  await shot(page, info, "contact-detail");
  await expectNoSidewaysScroll(page);
  if (width(page) <= NARROW_MAX) {
    await expect(list).toBeHidden();
    await detail.getByRole("button", { name: "Back to contacts" }).click();
    await expect(list).toBeVisible();
  }
});

test("contacts: a blob photo (Stalwart 1.0) shows and survives an edit", async ({
  page,
}) => {
  await page.goto("/contacts/k1");
  const detail = page.getByRole("region", { name: "Contact" });
  await expect(
    detail.getByRole("heading", { name: "Carla Gómez" })
  ).toBeVisible();
  // Downloaded with the session and redrawn: only a JPEG data: URL shows.
  await expect(
    detail.locator('img[src^="data:image/jpeg;base64,"]')
  ).toHaveCount(1);
  await detail.getByRole("button", { name: "Edit" }).click();
  const dialog = page.getByRole("dialog", { name: "Edit contact" });
  await expect(
    dialog.locator('img[src^="data:image/jpeg;base64,"]')
  ).toHaveCount(1);
  await dialog.getByLabel("Notes").fill("Met at the fair");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toBeHidden();
  const update = jmap.cardSets.at(-1)?.update;
  expect(update).toMatchObject({
    k1: { notes: { n1: { note: "Met at the fair" } } },
  });
  // The photo wasn't touched: its media entry stays as the server has it.
  expect(JSON.stringify(update)).not.toContain('"media"');
});

test("contacts: export includes a blob photo, after the server's 503", async ({
  page,
}) => {
  // Stalwart 1.0 out of memory: 503, and CORS hides its Retry-After.
  let refused = 0;
  await page.route("**/jmap/download/**", async (route) => {
    if (refused > 0) {
      await route.fallback();
      return;
    }
    refused += 1;
    await route.fulfill({
      headers: { "access-control-allow-origin": "*" },
      json: { detail: "Try again", status: 503, type: "about:blank" },
      status: 503,
    });
  });
  await page.goto("/contacts/k1");
  const detail = page.getByRole("region", { name: "Contact" });
  await expect(
    detail.locator('img[src^="data:image/jpeg;base64,"]')
  ).toHaveCount(1, { timeout: 15_000 });
  expect(refused).toBe(1);
  const saved = page.waitForEvent("download");
  await detail.getByRole("button", { name: "Export (.vcf)" }).click();
  const download = await saved;
  const content = await readFile(await download.path(), "utf-8");
  const text = content.replaceAll("\r\n ", "");
  expect(text).toContain("FN:Carla Gómez");
  expect(text).toMatch(/\r\nPHOTO:data:image\/jpeg;base64,[\w+/=]+\r\n/u);
});

test("contacts: a group shows its members", async ({ page }) => {
  await page.goto("/contacts");
  // In the sidebar; on phones (sidebar in a drawer) a switch in the list.
  await (
    width(page) <= PHONE_MAX
      ? page.getByRole("radio", { name: "Groups" })
      : page
          .getByRole("navigation", { name: "Contacts sidebar" })
          .getByRole("button", { name: /^Groups/u })
  ).click();
  await page.getByRole("button", { name: /^Team.*members/u }).click();
  const detail = page.getByRole("region", { name: "Contact" });
  await expect(
    detail.getByRole("link", { name: /Carla Gómez/u })
  ).toBeVisible();
  await expect(detail.getByRole("link", { name: /Bob Pérez/u })).toBeVisible();
});

test("contacts: create one with the editor", async ({ page }, info) => {
  await page.goto("/contacts");
  await expect(
    page.getByRole("button", { name: /Carla Gómez/u })
  ).toBeVisible();
  // The sidebar's button (with its N key) or, on phones, the floating one.
  await page
    .getByRole("button", { name: /^New contact/u })
    .filter({ visible: true })
    .first()
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("First name").fill("Dora");
  await dialog.getByLabel("Last name").fill("Díaz");
  await dialog.getByRole("button", { name: "Add email" }).click();
  await dialog.getByLabel("Email", { exact: true }).fill("dora@example.com");
  await shot(page, info, "contact-editor");
  await expectNoSidewaysScroll(page);
  await dialog.getByRole("button", { name: "Save" }).click();

  await expect(
    page.getByRole("region", { name: "Contact" }).getByRole("heading", {
      name: "Dora Díaz",
    })
  ).toBeVisible();
  const create = jmap.cardSets.at(-1)?.create;
  expect(Object.values(create ?? {})[0]).toMatchObject({
    addressBookIds: { book: true },
    emails: { e1: { address: "dora@example.com" } },
    name: { full: "Dora Díaz" },
    uid: expect.stringMatching(/^urn:uuid:/u),
  });
});

test("contacts: keyboard (j/k, Enter, e, #)", async ({ page }, info) => {
  test.skip(info.project.use.hasTouch === true, "Keyboard only");
  await page.goto("/contacts");
  await expect(page.getByRole("button", { name: /Bob Pérez/u })).toBeVisible();
  // Rows are sorted by name: Bob, Carla, Team.
  await page.keyboard.press("j");
  await page.keyboard.press("j");
  await page.keyboard.press("k");
  await page.keyboard.press("Enter");
  const detail = page.getByRole("region", { name: "Contact" });
  await expect(
    detail.getByRole("heading", { name: "Carla Gómez" })
  ).toBeVisible();
  await page.keyboard.press("e");
  const editor = page.getByRole("dialog", { name: "Edit contact" });
  await expect(editor).toBeVisible();
  await editor.getByLabel("Job title").fill("CTO");
  await editor.getByRole("button", { name: "Save" }).click();
  await expect(editor).toBeHidden();
  await expect(detail.getByText("CTO")).toBeVisible();
  await page.keyboard.press("#");
  const confirm = page.getByRole("alertdialog");
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "Delete" }).click();
  await expect.poll(() => jmap.cardSets.at(-1)?.destroy).toStrictEqual(["k1"]);
});

test("storage bar warns near the limit", async ({ page }) => {
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  if (width(page) <= PHONE_MAX) {
    await page.getByRole("button", { name: "Open menu" }).click();
  }
  const nav = page
    .getByRole("navigation", { name: "Mail folders" })
    .filter({ visible: true });
  await expect(nav.getByText("Your mailbox is almost full.")).toBeVisible();
  await expect(nav.getByRole("progressbar")).toBeVisible();
});

test("a shared folder uses its own account", async ({ page }, info) => {
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  await page.goto(`/mail/${SHARED_ACCOUNT}.team`);
  const row = page.getByText("Hello from the shared folder").first();
  await expect(row).toBeVisible();
  // Our own e1 ("Quarterly report…") must not leak into this list.
  await expect(page.getByText(/Quarterly report/u)).toHaveCount(0);
  await row.click();
  await expect(
    page.getByRole("heading", { name: "Hello from the shared folder" })
  ).toBeVisible();
  await shot(page, info, "shared-reader");
  expect(jmap.accountsUsed).toContain(SHARED_ACCOUNT);

  // Back in our inbox, e1 is ours again.
  await page.goto("/mail/inbox/e1");
  await expect(
    page.getByRole("heading", { name: /Quarterly report/u })
  ).toBeVisible();
});

test("the sidebar lists folders shared with us", async ({ page }) => {
  test.skip(
    (page.viewportSize()?.width ?? 0) <= PHONE_MAX,
    "On phones the sidebar is the drawer (covered above)"
  );
  await page.goto("/mail/inbox");
  const nav = page.getByRole("navigation", { name: "Mail folders" });
  await expect(nav.getByText("Shared with me")).toBeVisible();
  await nav.getByRole("link", { name: /Team inbox/u }).click();
  await expect(page).toHaveURL(
    new RegExp(`/mail/${SHARED_ACCOUNT}\\.team`, "u")
  );
});

test("contacts: a birthday without a year", async ({ page }) => {
  await page.goto("/contacts/k1");
  const detail = page.getByRole("region", { name: "Contact" });
  await expect(
    detail.getByRole("heading", { name: "Carla Gómez" })
  ).toBeVisible();
  await detail.getByRole("button", { name: "Edit" }).click();
  const editor = page.getByRole("dialog", { name: "Edit contact" });
  await chooseOption(editor.getByLabel("Day", { exact: true }), "24");
  // Half typed: refused with a message, nothing sent.
  await editor.getByRole("button", { name: "Save" }).click();
  await expect(editor.getByRole("alert")).toContainText("birthday");
  await chooseOption(editor.getByLabel("Month"), "12");
  await editor.getByRole("button", { name: "Save" }).click();
  await expect(editor).toBeHidden();
  // Only day and month: no year, no other calendar.
  expect(jmap.cardSets.at(-1)?.update).toStrictEqual({
    k1: expect.objectContaining({
      anniversaries: {
        b1: {
          date: { "@type": "PartialDate", day: 24, month: 12 },
          kind: "birth",
        },
      },
    }),
  });
  await expect(detail.getByText("December 24")).toBeVisible();

  // Editing again shows it without a year, and keeps it as it is.
  await detail.getByRole("button", { name: "Edit" }).click();
  await expectValue(editor.getByLabel("Day", { exact: true }), "24");
  await expectValue(editor.getByLabel("Month"), "12");
  await expect(editor.getByLabel("Year")).toHaveValue("");
});

test("contacts: a shared address book is read only", async ({ page }) => {
  await page.goto("/contacts");
  const list = page.getByRole("list", { name: "Contacts list" });
  await list.getByRole("button", { name: /Dana Levi/u }).click();
  await expect(page).toHaveURL(
    new RegExp(`/contacts/${SHARED_ACCOUNT}\\.k1$`, "u")
  );
  const detail = page.getByRole("region", { name: "Contact" });
  await expect(
    detail.getByRole("heading", { name: "Dana Levi" })
  ).toBeVisible();
  await expect(
    detail.getByText("In bob@example.com's shared address book (read only)")
  ).toBeVisible();
  // Its book's myRights: no edit or delete, not even by keyboard.
  await expect(detail.getByRole("button", { name: "Edit" })).toHaveCount(0);
  await expect(detail.getByRole("button", { name: "Delete" })).toHaveCount(0);
  await page.keyboard.press("e");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  // A Hebrew-calendar birthday, with its Gregorian day.
  await expect(
    detail.getByText("10 Adar II 5784 (March 20, 2024)")
  ).toBeVisible();
  expect(jmap.accountsUsed).toContain(SHARED_ACCOUNT);
  expect(jmap.cardSets).toHaveLength(0);

  // Our own k1 is still Carla.
  await page.goto("/contacts/k1");
  await expect(
    detail.getByRole("heading", { name: "Carla Gómez" })
  ).toBeVisible();
});

test("contacts: share our address book", async ({ page }) => {
  jmap.directory = true;
  await page.goto("/contacts");
  await expect(
    page.getByRole("button", { name: /Carla Gómez/u })
  ).toBeVisible();
  await page.getByRole("button", { name: "More contact actions" }).click();
  await page.getByRole("menuitem", { name: "Address books" }).click();
  const dialog = page.getByRole("dialog", { name: "Address books" });
  await expect(
    dialog.getByRole("list", { name: "Shared with you" })
  ).toContainText("Team contacts");
  await expect(
    dialog.getByRole("list", { name: "Shared with you" })
  ).toContainText("Can view");
  await dialog.getByRole("button", { name: "Share…" }).click();
  const share = page.getByRole("dialog", { name: "Share “Personal”" });
  await share.getByLabel("Search people by name or address").fill("eve");
  await share.getByRole("button", { exact: true, name: "Share" }).click();
  await expect
    .poll(() => jmap.bookSets.at(-1)?.update)
    .toStrictEqual({
      book: {
        "shareWith/pe": {
          mayDelete: false,
          mayRead: true,
          mayShare: false,
          mayWrite: false,
        },
      },
    });
  await expect(share.getByText("Eve Núñez")).toBeVisible();
  await chooseOption(share.getByLabel("Access for Eve Núñez"), {
    label: "Can view and edit",
  });
  await expect
    .poll(() => jmap.bookShares.pe)
    .toMatchObject({ mayDelete: false, mayWrite: true });
});
