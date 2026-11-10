// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The rest of the compose toolbar: font and
// size, paragraph direction, emoji, an image in the body (inline cid:
// part) and plain-text mode, and the default format from Settings.

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { chooseOption } from "./select";
import { signIn } from "./session";

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName !== "chromium" || info.project.name !== "desktop",
    "the toolbar is the same everywhere: one desktop run is enough"
  );
});

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

const openComposer = async (page: Page) => {
  await page.goto("/mail/inbox");
  // A click before the page has hydrated does nothing.
  await page.waitForLoadState("networkidle");
  await page
    .getByRole("button", { name: /^(?:New message|Compose)/u })
    .filter({ visible: true })
    .first()
    .click();
  const composer = page.locator("section[data-composer]");
  await expect(composer).toBeVisible();
  await composer.getByRole("button", { name: /^Save draft/u }).waitFor();
  return composer;
};

const Created = z.object({
  bodyStructure: z.unknown().optional(),
  bodyValues: z.object({
    html: z.object({ value: z.string() }).optional(),
    text: z.object({ value: z.string() }).optional(),
  }),
  htmlBody: z.unknown().optional(),
});

/** Saves now (⌘S) and returns the draft the server got. */
const saveNow = async (page: Page) => {
  const before = jmap.created.length;
  await page
    .locator("section[data-composer]")
    .getByRole("textbox", { name: "Subject" })
    .press("ControlOrMeta+s");
  await expect.poll(() => jmap.created.length).toBeGreaterThan(before);
  return Created.parse(jmap.created.at(-1));
};

// A 1×1 PNG.
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=",
  "base64"
);

test("font, size, direction and emoji", async ({ page }) => {
  const composer = await openComposer(page);
  const editor = composer.getByRole("textbox", { name: "Message" });
  const toolbar = composer.getByRole("toolbar", { name: "Formatting" });
  const choose = async (menu: string, item: string) => {
    await toolbar.getByRole("button", { name: menu }).click();
    await page.getByRole("menuitemradio", { name: item }).click();
  };

  // Text style, font, colours, alignment… are behind "Aa".
  await toolbar.getByRole("button", { name: "Formatting options" }).click();
  await editor.click();
  await editor.press("ControlOrMeta+Home");
  await editor.pressSequentially("Hola");
  await editor.press("Shift+Home");
  await choose("Font", "Georgia");
  await choose("Text size", "24 px");
  await expect(editor.locator("span[style]").first()).toHaveCSS(
    "font-size",
    "24px"
  );
  await choose("More formatting", "Right to left");
  await expect(editor.locator('p[dir="rtl"]')).toHaveText("Hola");

  await editor.press("End");
  await toolbar.getByRole("button", { name: "Emoji" }).click();
  const grid = page.getByRole("group", { name: "Emoji" });
  // Keyboard: the arrows move in the grid, Enter inserts.
  await expect(grid.getByRole("button").first()).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  await expect(editor.locator('p[dir="rtl"]')).toHaveText("Hola😃");
  await expect(editor).toBeFocused();

  const { bodyValues } = await saveNow(page);
  const html = bodyValues.html?.value ?? "";
  // The default font and size (Settings) wrap the message.
  expect(html).toMatch(
    /^<div style="font-family: Arial, sans-serif; font-size: 14px">/u
  );
  expect(html).toMatch(/<p dir="rtl">/u);
  expect(html).toMatch(/font-family: Georgia, serif; font-size: 24px/u);
  expect(bodyValues.text?.value).toContain("Hola😃");
});

test("an image from this device goes in the text as an inline part", async ({
  page,
}) => {
  const composer = await openComposer(page);
  const editor = composer.getByRole("textbox", { name: "Message" });
  const toolbar = composer.getByRole("toolbar", { name: "Formatting" });
  await editor.click();
  await editor.press("ControlOrMeta+Home");
  await editor.pressSequentially("Mira: ");

  // A file the body can't show is refused.
  await toolbar.getByRole("button", { name: "More formatting" }).click();
  const refused = page.waitForEvent("filechooser");
  await page.getByRole("menuitem", { name: "Insert image" }).click();
  const refusedFile = await refused;
  await refusedFile.setFiles({
    buffer: Buffer.from("x"),
    mimeType: "image/svg+xml",
    name: "a.svg",
  });
  await expect(
    page
      .getByRole("region", { name: "Notifications" })
      .getByText(/a\.svg: only PNG, JPEG, GIF or WebP/u)
  ).toBeVisible();

  await toolbar.getByRole("button", { name: "More formatting" }).click();
  const chooser = page.waitForEvent("filechooser");
  await page.getByRole("menuitem", { name: "Insert image" }).click();
  const chooserFile = await chooser;
  await chooserFile.setFiles({
    buffer: PNG,
    mimeType: "image/png",
    name: "punto.png",
  });

  // Shown from the local copy (data:), never from its cid: URL.
  const image = editor.locator(".nm-inline-image img");
  await expect(image).toHaveAttribute("src", /^data:image\/png;base64,/u);
  // Part of the body, not of the attachment list.
  await expect(composer.getByText("punto.png")).toHaveCount(0);

  const draft = await saveNow(page);
  expect(draft.bodyValues.html?.value).toMatch(
    /Mira: <img alt="punto\.png" src="cid:[\w-]+@nebula-mail">/u
  );
  expect(draft.bodyStructure).toMatchObject({
    subParts: [
      { type: "multipart/alternative" },
      {
        cid: expect.stringMatching(/^[\w-]+@nebula-mail$/u),
        disposition: "inline",
        name: "punto.png",
        type: "image/png",
      },
    ],
    type: "multipart/related",
  });
});

test("plain text: warns, sends only text, and back to HTML", async ({
  page,
}) => {
  const composer = await openComposer(page);
  const editor = composer.getByRole("textbox", { name: "Message" });
  await editor.click();
  await editor.press("ControlOrMeta+Home");
  await editor.pressSequentially("Hola");
  await composer
    .getByRole("toolbar", { name: "Formatting" })
    .getByRole("button", { name: "Bold" })
    .click();

  await composer.getByRole("button", { name: "More options" }).click();
  await page.getByRole("menuitemcheckbox", { name: "Plain text" }).click();
  const dialog = page.getByRole("alertdialog", {
    name: "Switch to plain text?",
  });
  await expect(dialog).toContainText("Formatting will be lost");
  await dialog.getByRole("button", { name: "Switch to plain text" }).click();

  const area = composer.locator("textarea[aria-label='Message']");
  await expect(area).toHaveValue(/^Hola/u);
  await expect(
    composer.getByRole("toolbar", { name: "Formatting" })
  ).toHaveCount(0);
  await area.press("End");
  await area.pressSequentially(" <b>no es HTML</b>");

  const plain = await saveNow(page);
  expect(plain.htmlBody).toBeUndefined();
  expect(plain.bodyValues.html).toBeUndefined();
  expect(plain.bodyValues.text?.value).toMatch(/^Hola <b>no es HTML<\/b>/u);

  // Back to HTML: nothing to lose, no question; the text stays text.
  await composer.getByRole("button", { name: "More options" }).click();
  await page.getByRole("menuitemcheckbox", { name: "Plain text" }).click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await expect(editor).toContainText("Hola <b>no es HTML</b>");
  const html = await saveNow(page);
  expect(html.bodyValues.html?.value).toContain(
    "Hola &lt;b&gt;no es HTML&lt;/b&gt;"
  );
});

test("Settings: new messages in plain text, default font and size", async ({
  page,
}) => {
  await page.goto("/settings/general");
  await page.waitForLoadState("networkidle");
  const plainRadio = page
    .getByRole("radiogroup", { name: "Format for new messages" })
    .getByRole("radio", { name: "Plain text" });
  // A click before hydration does nothing: click until it takes.
  await expect(async () => {
    await plainRadio.click();
    await expect(plainRadio).toBeChecked({ timeout: 1000 });
  }).toPass();
  await chooseOption(
    page.getByRole("combobox", { name: "Default font" }),
    "verdana"
  );
  await chooseOption(
    page.getByRole("combobox", { name: "Default size" }),
    "16px"
  );

  const composer = await openComposer(page);
  await expect(
    composer.locator("textarea[aria-label='Message']")
  ).toBeVisible();
  await composer.getByRole("button", { name: "More options" }).click();
  await page.getByRole("menuitemcheckbox", { name: "Plain text" }).click();
  // The editor shows the default font and size, as they will be sent.
  const editor = composer.getByRole("textbox", { name: "Message" });
  await expect(editor).toHaveCSS("font-size", "16px");
  await expect(editor).toHaveCSS("font-family", /Verdana/u);
});

// composer-form.tsx swaps the text signature when the sender changes in
// plain-text mode (the rich editor swaps its own block).
test("plain text: another sender swaps the text signature", async ({
  page,
}) => {
  const [jane] = jmap.identities;
  if (jane === undefined) {
    throw new Error("the fake has an identity");
  }
  jmap.identities.splice(
    0,
    1,
    { ...jane, textSignature: "Jane at work" },
    {
      ...jane,
      email: "[EMAIL]",
      htmlSignature: "<p>Jane at home</p>",
      id: "i2",
      name: "Jane at home",
      textSignature: "Jane at home",
    }
  );
  const composer = await openComposer(page);
  await composer.getByRole("button", { name: "More options" }).click();
  await page.getByRole("menuitemcheckbox", { name: "Plain text" }).click();
  const confirm = page.getByRole("alertdialog", {
    name: "Switch to plain text?",
  });
  const area = composer.locator("textarea[aria-label='Message']");
  // Asked only when there is formatting to lose: whichever comes first.
  await confirm.or(area).first().waitFor();
  if (await confirm.isVisible()) {
    await confirm.getByRole("button", { name: "Switch to plain text" }).click();
  }
  await expect(area).toHaveValue(/Jane at work/u);
  await area.press("ControlOrMeta+Home");
  await area.pressSequentially("Hola");

  await composer.getByRole("button", { name: /^From: /u }).click();
  await page.getByRole("menuitem", { name: /Jane at home/u }).click();
  // The typed text stays; only the signature changes.
  await expect(area).toHaveValue(/^Hola[\s\S]*Jane at home/u);
  await expect(area).not.toHaveValue(/Jane at work/u);

  await composer.getByRole("button", { name: /^From: /u }).click();
  await page.getByRole("menuitem", { name: /^Jane </u }).click();
  await expect(area).toHaveValue(/^Hola[\s\S]*Jane at work/u);
  await expect(area).not.toHaveValue(/Jane at home/u);
});
