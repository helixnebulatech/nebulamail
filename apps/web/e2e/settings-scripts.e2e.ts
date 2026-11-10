// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Settings, second part: the Sieve editor's
// colours, several scripts, the vacation response switching another app's
// script back on, its date pickers, preferences synced through FileNode,
// and Trusted Types (TipTap, React Aria and the email iframe with
// `require-trusted-types-for 'script'`).

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { FakeJmap } from "./fake-jmap";
import {
  addScript,
  installFakeJmap,
  putSyncedPreferences,
  syncedPreferences,
} from "./fake-jmap";
import { signIn } from "./session";

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

const activeOf = (name: string): boolean | null =>
  jmap.scripts.find((script) => script.name === name)?.isActive ?? null;

/** Opens the "More for …" menu of a script and picks an item. */
const scriptMenu = async (page: Page, script: string, item: string) => {
  await page.getByRole("button", { name: `More for ${script}` }).click();
  await page.getByRole("menuitem", { name: item }).click();
};

test("the Sieve editor colours the script as you type", async ({ page }) => {
  addScript(
    jmap,
    "my-rules",
    'require "fileinto";\n# sort\nif header :contains "subject" "x" {\n  fileinto "A";\n}\n',
    true
  );
  await page.goto("/settings/filters");
  const editor = page.getByLabel("Sieve script my-rules");
  await expect(editor).toHaveValue(/fileinto "A"/u);
  const layer = page.locator("[data-code-highlight]");
  await expect(layer).toHaveText(/require "fileinto";/u);
  const keyword = layer.locator("span", { hasText: /^require$/u });
  const comment = layer.locator("span", { hasText: /^# sort$/u });
  const tag = layer.locator("span", { hasText: /^:contains$/u });
  await expect(keyword).toHaveCount(1);
  const colour = async (locator: typeof keyword) =>
    await locator.evaluate((node) => getComputedStyle(node).color);
  const colours = new Set([
    await colour(keyword),
    await colour(comment),
    await colour(tag),
  ]);
  expect(colours.size).toBe(3);
  // The textarea's own text is transparent: the layer draws it.
  await expect(editor).toHaveCSS("color", "rgba(0, 0, 0, 0)");
  // The layer follows the text.
  await editor.press("Control+End");
  await editor.pressSequentially('stop; "open');
  await expect(layer.locator("span", { hasText: /^stop$/u })).toHaveCount(1);
  await expect(layer.locator("span", { hasText: /^"open/u })).toHaveCount(1);
  // The layer is decoration: not in the accessibility tree.
  await expect(layer.locator("xpath=..")).toHaveAttribute(
    "aria-hidden",
    "true"
  );
});

test("several scripts: create, edit, rename, activate, delete", async ({
  page,
}) => {
  await page.goto("/settings/filters");
  await page.getByRole("button", { name: "New Sieve script" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Name").fill("vacation");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog.getByText("That name is reserved")).toBeVisible();
  await dialog.getByLabel("Name").fill("Work");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("“Work” created")).toBeVisible();
  await expect.poll(() => activeOf("Work")).toBeFalsy();

  // Edit it as Sieve; the server checks it before saving.
  await page.getByRole("button", { name: "Edit Work" }).click();
  const editor = page.getByLabel("Sieve script Work");
  await expect(editor).toHaveValue(/require \["fileinto"\];/u);
  await editor.fill("keep;\n");
  await page.getByRole("button", { name: "Check and save" }).click();
  await expect(page.getByText(/^Saved\./u)).toBeVisible();
  const work = () => jmap.scripts.find((script) => script.name === "Work");
  // Uploaded with CRLF line ends (RFC 5228 §8.1).
  expect(jmap.blobs.get(work()?.blobId ?? "")).toBe("keep;\r\n");
  await page.getByRole("button", { name: "Back to filters" }).click();
  await expect(page.getByRole("button", { name: "New filter" })).toBeVisible();

  await scriptMenu(page, "Work", "Rename");
  await page.getByRole("dialog").getByLabel("Name").fill("Office");
  await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
  await expect.poll(() => work()?.name ?? "gone").toBe("gone");
  await expect.poll(() => activeOf("Office")).toBeFalsy();

  await scriptMenu(page, "Office", "Activate");
  await expect.poll(() => activeOf("Office")).toBeTruthy();
  // The active script isn't our rules: it shows as Sieve.
  await expect(page.getByLabel("Sieve script Office")).toBeVisible();
  await expect(
    page.getByRole("list", { name: "Sieve scripts" }).getByText("Active")
  ).toBeVisible();

  await scriptMenu(page, "Office", "Delete");
  await expect(page.getByRole("alertdialog")).toContainText("active script");
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete" })
    .click();
  await expect.poll(() => activeOf("Office")).toBeNull();
  await expect(page.getByRole("button", { name: "New filter" })).toBeVisible();
});

const saveVacation = async (page: Page) => {
  await page.getByRole("button", { name: "Save", exact: true }).click();
};

test("turning the vacation response off switches the paused script back on", async ({
  page,
}) => {
  jmap.fileNode = true;
  addScript(jmap, "my-rules", "keep;\n", true);
  await page.goto("/settings/vacation");
  const toggle = page.getByRole("switch", { name: "Vacation response on" });
  await toggle.click();
  await expect(page.getByText(/“my-rules”/u).first()).toBeVisible();
  await saveVacation(page);
  await expect.poll(() => jmap.vacation.isEnabled).toBeTruthy();
  expect(activeOf("my-rules")).toBeFalsy();
  // Noted in the synced preferences (any device can switch it back on).
  await expect
    .poll(() => JSON.stringify(syncedPreferences(jmap)))
    .toContain('"vacationPausedScript":{"at"');

  // Another device: nothing on this one.
  await page.evaluate(() => {
    localStorage.clear();
  });
  await page.reload();
  await expect(
    page.getByText("When you turn it off, “my-rules” will be switched back on.")
  ).toBeVisible();
  await toggle.click();
  await saveVacation(page);
  await expect.poll(() => activeOf("my-rules")).toBeTruthy();
  await expect(page.getByText("“my-rules” is active again.")).toBeVisible();
  expect(jmap.vacation.isEnabled).toBeFalsy();
});

test("vacation dates with the date picker", async ({ page }) => {
  await page.goto("/settings/vacation");
  const first = page.getByRole("group", { name: "First day" });
  await expect(first).toBeVisible();
  await page.getByRole("button", { name: "Choose a date" }).first().click();
  const calendar = page.getByRole("dialog", { name: "Choose a date" });
  await expect(calendar.getByRole("grid")).toBeVisible();
  // Any day of the shown month that can be picked.
  await calendar
    .getByRole("gridcell")
    .filter({ hasNot: page.locator("[aria-disabled='true']") })
    .getByRole("button")
    .nth(14)
    .click();
  await expect(calendar).toBeHidden();
  await expect(
    page.getByRole("button", { name: "Clear First day" })
  ).toBeVisible();
  await page.getByRole("switch", { name: "Vacation response on" }).click();
  await saveVacation(page);
  await expect
    .poll(() => jmap.vacationUpdates.at(-1)?.fromDate ?? null)
    .toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/u);
  // Clearing it saves no date.
  await page.getByRole("button", { name: "Clear First day" }).click();
  await saveVacation(page);
  await expect.poll(() => jmap.vacationUpdates.length).toBe(2);
  expect(jmap.vacationUpdates.at(-1)?.fromDate).toBeNull();
});

test("preferences follow the account (FileNode)", async ({ context, page }) => {
  jmap.fileNode = true;
  // Another device chose the dark theme a moment ago.
  putSyncedPreferences(jmap, {
    format: "nebula-mail-preferences",
    values: { theme: { at: Date.now(), value: "dark" } },
    version: 1,
  });
  await page.goto("/settings/general");
  await expect
    .poll(async () => {
      const cookies = await context.cookies();
      return cookies.find((cookie) => cookie.name === "wm-prefs")?.value ?? "";
    })
    .toMatch(/^dark\./u);
  await expect(page.getByText(/saved in your account/u)).toBeVisible();
  // A change here goes to the server.
  await page.getByRole("radio", { name: "Comfortable" }).click();
  await expect
    .poll(() => JSON.stringify(syncedPreferences(jmap)?.values ?? {}))
    .toContain('"value":"comfortable"');
  expect(JSON.stringify(syncedPreferences(jmap))).toContain('"value":"dark"');
});

test("without FileNode the preferences stay on the device", async ({
  page,
}) => {
  await page.goto("/settings/general");
  await expect(page.getByText(/stay on this device/u)).toBeVisible();
  await expect(
    page.getByRole("switch", { name: "Save settings in a mail folder" })
  ).toBeVisible();
  expect(jmap.files).toHaveLength(0);
});

test("Trusted Types: the editor, the date picker and an email work", async ({
  browserName,
  page,
}) => {
  test.skip(browserName !== "chromium", "Trusted Types are Chromium's");
  const problems: string[] = [];
  page.on("console", (message) => {
    if (/trusted ?type|TrustedHTML|TrustedScript/iu.test(message.text())) {
      problems.push(message.text());
    }
  });
  page.on("pageerror", (error) => {
    problems.push(error.message);
  });
  jmap.vacation = {
    ...jmap.vacation,
    htmlBody: "<p>Back <b>Monday</b>.</p>",
    textBody: "Back Monday.",
  };
  const response = await page.goto("/settings/vacation");
  expect(response?.headers()["content-security-policy"]).toContain(
    "require-trusted-types-for 'script'"
  );
  expect(
    await page.evaluate(() => {
      const factory: unknown = Reflect.get(globalThis, "trustedTypes");
      const policy: unknown =
        typeof factory === "object" && factory !== null
          ? Reflect.get(factory, "defaultPolicy")
          : null;
      return typeof policy === "object" && policy !== null
        ? String(Reflect.get(policy, "name"))
        : null;
    })
  ).toBe("default");
  // TipTap loads its content (DOMParser, through the default policy).
  const message = page.getByRole("textbox", { name: "Message" });
  await expect(message.locator("strong, b")).toHaveText("Monday");
  // Pasting HTML (ProseMirror's own policy, then our sanitizer).
  await message.click();
  await message.press("Control+End");
  await message.evaluate((node) => {
    const data = new DataTransfer();
    data.setData(
      "text/html",
      '<p>Pasted <em>here</em><img src=x onerror="window.pwned=1"></p>'
    );
    data.setData("text/plain", "Pasted here");
    node.dispatchEvent(
      new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData: data,
      })
    );
  });
  await expect(message.locator("em")).toHaveText("here");
  expect(
    await page.evaluate(() => typeof Reflect.get(globalThis, "pwned"))
  ).toBe("undefined");
  // React Aria's date picker.
  await page.getByRole("button", { name: "Choose a date" }).first().click();
  await expect(
    page.getByRole("dialog", { name: "Choose a date" }).getByRole("grid")
  ).toBeVisible();
  await page.keyboard.press("Escape");
  // An email in its iframe (srcdoc through our policy).
  await page.goto("/mail/inbox");
  await page.getByText("Weekly digest").first().click();
  await expect(
    page.locator("iframe[title]").first().contentFrame().locator("body")
  ).not.toBeEmpty();
  expect(problems).toStrictEqual([]);
});
