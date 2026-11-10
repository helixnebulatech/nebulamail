// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Writing in hard conditions: the app opened without a
// connection, a forward that carries the original's inline image, and a
// reply to an email that couldn't be loaded.

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import type { FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName !== "chromium" ||
      !["desktop", "phone-412"].includes(info.project.name),
    "one desktop and one phone run are enough"
  );
});

let jmap: FakeJmap;

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
});

const openComposer = async (page: Page) => {
  // A click before the page has hydrated does nothing; on a busy machine
  // that can take a while after "load".
  await page.waitForLoadState("networkidle");
  await page
    .getByRole("button", { name: /^(?:New message|Compose)/u })
    .filter({ visible: true })
    .first()
    .click();
  const composer = page.locator("section[data-composer]");
  await expect(composer).toBeVisible();
  return composer;
};

const Backup = z.object({
  draft: z.object({
    identity: z.object({ email: z.string() }).nullable(),
    subject: z.string(),
  }),
});

/** Every record of a store of the app's IndexedDB (device-db.ts). */
const readStore = async (
  page: Page,
  store: "backups" | "identities"
): Promise<unknown> =>
  await page.evaluate(async (name): Promise<unknown> => {
    // oxlint-disable-next-line unicorn/consistent-function-scoping -- this function runs in the page: it can only use what is defined inside it.
    const settle = async <T>(request: IDBRequest<T>): Promise<T> => {
      const { promise, reject, resolve } = Promise.withResolvers<T>();
      request.addEventListener("success", () => {
        resolve(request.result);
      });
      request.addEventListener("error", () => {
        reject(request.error ?? new Error("IndexedDB request failed"));
      });
      return await promise;
    };
    const db = await settle(indexedDB.open("nebula-mail-drafts"));
    try {
      return db.objectStoreNames.contains(name)
        ? await settle(db.transaction(name).objectStore(name).getAll())
        : [];
    } finally {
      db.close();
    }
  }, store);

/** The draft copies on this device (draft-backup.ts). */
const deviceCopies = async (page: Page) =>
  z.array(Backup).parse(await readStore(page, "backups"));

const Created = z.object({
  bodyValues: z.object({ html: z.object({ value: z.string() }) }),
  from: z.array(z.object({ email: z.string() })),
  subject: z.string(),
});

test("compose works when the app opened without a connection", async ({
  context,
  page,
}) => {
  // The page loads, but the mail server never answers, then the network
  // goes away: no identities, nothing saved on this device yet.
  jmap.down = true;
  await page.goto("/mail/inbox");
  await expect(page.getByRole("main")).toBeVisible();
  // Let the app finish loading its code first: going offline mid-load is
  // the service worker's job (static files cached), not this
  // test's. The composer's code comes when the browser is idle after the
  // first screen (composer-dock.tsx), so wait for it by name.
  await page.waitForFunction(() =>
    performance
      .getEntriesByType("resource")
      .some((entry) => /\/assets\/composer-[\w-]+\.js$/u.test(entry.name))
  );
  await page.waitForLoadState("networkidle");
  await context.setOffline(true);

  const composer = await openComposer(page);
  await expect(composer.getByRole("status")).toContainText("You're offline");
  await composer.getByRole("combobox", { name: "To" }).fill("bob@example.com");
  await composer.getByRole("combobox", { name: "To" }).press("Enter");
  await composer
    .getByRole("textbox", { name: "Subject" })
    .fill("Written offline");
  await expect(composer.getByText("Saved on this device only")).toBeVisible({
    timeout: 10_000,
  });
  const copies = await deviceCopies(page);
  expect(copies.map((copy) => copy.draft)).toEqual([
    { ...copies[0]?.draft, identity: null, subject: "Written offline" },
  ]);
  // Sending waits for the connection.
  await composer.getByRole("button", { name: /^Send(?! options)/u }).click();
  await expect(composer.getByRole("alert")).toContainText("back online");

  // Back online: the sender appears and the draft goes to the server.
  jmap.down = false;
  await context.setOffline(false);
  await expect(composer.getByText("You're offline")).toHaveCount(0, {
    timeout: 15_000,
  });
  await expect
    .poll(() => jmap.created.map((email) => Created.parse(email)), {
      timeout: 15_000,
    })
    .toContainEqual(
      expect.objectContaining({
        from: [expect.objectContaining({ email: "jane@example.com" })],
        subject: "Written offline",
      })
    );
  await expect(composer.getByText(/^Saved at/u)).toBeVisible();
  await expect.poll(async () => await deviceCopies(page)).toEqual([]);
});

test("offline, the identities saved on this device are used", async ({
  page,
}) => {
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  // The identities were saved when they loaded (identity-cache.ts).
  await expect
    .poll(async () => await readStore(page, "identities"))
    .toHaveLength(1);

  jmap.down = true;
  await page.reload();
  const composer = await openComposer(page);
  await expect(composer.getByText("You're offline")).toHaveCount(0);
  // The signature comes from the saved identity.
  await expect(composer.getByText("Jane from Nebula")).toBeVisible();
  await composer
    .getByRole("textbox", { name: "Subject" })
    .fill("From the saved identity");
  await expect(composer.getByText("Saved on this device only")).toBeVisible({
    timeout: 10_000,
  });
  const [copy] = await deviceCopies(page);
  expect(copy?.draft.identity?.email).toBe("jane@example.com");
});

test("forward keeps the inline image, never loads remote ones", async ({
  page,
}) => {
  const remote: string[] = [];
  page.on("request", (request) => {
    if (/tracker\.example|cdn\.example/u.test(request.url())) {
      remote.push(request.url());
    }
  });
  await page.goto("/mail/inbox/e15");
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Team photo" })
  ).toBeVisible();
  await reader.getByRole("button", { exact: true, name: "Forward" }).click();
  const composer = page.locator("section[data-composer]");
  const editor = composer.getByRole("textbox", { name: "Message" });
  // The image shows from a data: copy; the cid: URL is never used.
  const image = editor.locator(".nm-inline-image img");
  await expect(image).toHaveAttribute("src", /^data:image\/png;base64,/u);
  await expect(image).toHaveAttribute("alt", "Team photo");
  await expect(page.locator('img[src^="cid:"]')).toHaveCount(0);
  await expect(editor.locator("img:not(.ProseMirror-separator)")).toHaveCount(
    1
  );
  // Not listed as a file: it's part of the body.
  await expect(composer.getByText("photo.png")).toHaveCount(0);

  await composer.getByRole("combobox", { name: "To" }).fill("bob@example.com");
  await composer.getByRole("combobox", { name: "To" }).press("Enter");
  await expect.poll(() => jmap.created.length, { timeout: 10_000 }).toBe(1);
  const draft = z
    .object({
      bodyStructure: z.unknown(),
      bodyValues: z.object({ html: z.object({ value: z.string() }) }),
    })
    .parse(jmap.created[0]);
  expect(draft.bodyValues.html.value).toContain('src="cid:photo@example.com"');
  expect(draft.bodyValues.html.value).not.toMatch(/tracker|cdn\.example/u);
  // multipart/related with the original's blob (no upload).
  expect(draft.bodyStructure).toMatchObject({
    subParts: [
      { type: "multipart/alternative" },
      {
        blobId: "b-e15-img",
        cid: "photo@example.com",
        disposition: "inline",
      },
    ],
    type: "multipart/related",
  });
  expect(remote).toEqual([]);
});

test("Ctrl/⌘ + S saves the draft right away", async ({ page }) => {
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  await composer.getByRole("button", { name: /^Save draft/u }).waitFor();
  await composer.getByRole("textbox", { name: "Subject" }).fill("Plan");
  const before = jmap.created.length;
  await composer
    .getByRole("textbox", { name: "Subject" })
    .press("ControlOrMeta+s");
  // Before the autosave (3 s) would have run.
  await expect
    .poll(() => jmap.created.length, { timeout: 2000 })
    .toBe(before + 1);
  await expect(page.getByText("Draft saved").first()).toBeVisible();
});

test("a reply hides the quoted message, still sends it", async ({ page }) => {
  await page.goto("/mail/inbox/e1");
  const reader = page.getByRole("region", { name: "Message" });
  await reader.getByRole("button", { name: /^Reply(?: R)?$/u }).click();
  const composer = page.locator("section[data-composer]");
  const quote = composer.locator("[data-quote]");
  const toggle = composer.getByRole("button", { name: "Show quoted text" });
  await expect(toggle).toHaveAttribute("aria-expanded", "false");
  await expect(quote).toBeHidden();

  // Hidden on screen only: the draft has it.
  const editor = composer.getByRole("textbox", { name: "Message" });
  await editor.pressSequentially("Sure!");
  await expect.poll(() => jmap.created.length, { timeout: 10_000 }).toBe(1);
  const draft = z
    .object({ bodyValues: z.object({ html: z.object({ value: z.string() }) }) })
    .parse(jmap.created[0]);
  expect(draft.bodyValues.html.value).toContain("quarterly report");

  await toggle.click();
  await expect(quote).toBeVisible();
  const hide = composer.getByRole("button", { name: "Hide quoted text" });
  await expect(hide).toHaveAttribute("aria-expanded", "true");

  // Selecting everything shows it: it is never replaced unseen.
  await hide.click();
  await expect(quote).toBeHidden();
  await editor.press("ControlOrMeta+a");
  await expect(quote).toBeVisible();
});

test("reply waits for the email and says so if it can't load", async ({
  page,
}) => {
  const { promise, resolve } = Promise.withResolvers<undefined>();
  jmap.bodiesGate = promise;
  await page.goto("/mail/inbox/e1");
  const reader = page.getByRole("region", { name: "Message" });
  const reply = reader.getByRole("button", { name: /^Reply(?: R)?$/u });
  await expect(reply).toBeDisabled();
  await expect(
    reader.getByRole("button", { exact: true, name: "Forward" })
  ).toBeDisabled();
  resolve(undefined);
  jmap.bodiesGate = null;
  await expect(reply).toBeEnabled();

  // Now the full email fails to load.
  jmap.failBodies.add("e3");
  await page.goto("/mail/inbox/e3");
  await expect(reader.getByText("Couldn't open this message.")).toBeVisible({
    timeout: 10_000,
  });
  await reply.click();
  const toast = page
    .getByText("Couldn't load the message to reply to.")
    .first();
  await expect(toast).toBeVisible();
  await expect(page.locator("section[data-composer]")).toHaveCount(0);

  jmap.failBodies.clear();
  // Base UI hides high-priority toasts from the accessibility tree until
  // focused (it announces them in a live region instead), so find the
  // button by its text inside the notifications region.
  await page
    .getByRole("region", { name: "Notifications" })
    .locator("button", { hasText: "Try again" })
    .click();
  await expect(
    reader.getByRole("button", { name: "Reply to Shop" })
  ).toBeVisible();
  await reply.click();
  await expect(
    page
      .locator("section[data-composer]")
      .getByRole("textbox", { name: "Subject" })
  ).toHaveValue("Re: Your October invoice");
});

test("the toolbar formats: title, colour, highlight, alignment, undo", async ({
  page,
}) => {
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  await composer.getByRole("button", { name: /^Save draft/u }).waitFor();
  const editor = composer.getByRole("textbox", { name: "Message" });
  const toolbar = composer.getByRole("toolbar", { name: "Formatting" });
  const choose = async (menu: string, item: string) => {
    await toolbar.getByRole("button", { name: menu }).click();
    await page.getByRole("menuitemradio", { name: item }).click();
  };

  // Text style, font, colours, alignment… are behind "Aa".
  await toolbar.getByRole("button", { name: "Formatting options" }).click();
  await editor.click();
  // At the start: the click lands at the end of the signature.
  await editor.press("ControlOrMeta+Home");
  await editor.pressSequentially("Plan");
  await choose("Text style", "Title");
  await expect(editor.locator("h1")).toHaveText("Plan");
  await editor.press("End");
  await editor.press("Enter");
  await editor.pressSequentially("rojo");
  await editor.press("Shift+Home");
  await choose("Text color", "Red");
  await choose("Highlight", "Yellow");
  await choose("Align", "Center");
  await toolbar.getByRole("button", { name: "Strikethrough" }).click();
  await expect(
    toolbar.getByRole("button", { name: "Strikethrough" })
  ).toHaveAttribute("aria-pressed", "true");

  // Undo takes back the last step (the strikethrough) only.
  await toolbar.getByRole("button", { name: "Undo" }).click();
  await expect(editor.locator("s")).toHaveCount(0);

  // Saved now, after the last change (not an earlier autosave).
  const before = jmap.created.length;
  await composer
    .getByRole("textbox", { name: "Subject" })
    .press("ControlOrMeta+s");
  await expect.poll(() => jmap.created.length).toBeGreaterThan(before);
  const html = z
    .object({ bodyValues: z.object({ html: z.object({ value: z.string() }) }) })
    .parse(jmap.created.at(-1)).bodyValues.html.value;
  expect(html).toContain("<h1>Plan</h1>");
  expect(html).toMatch(/<p style="text-align: center;?">/u);
  // Browsers write colours back as rgb() (the sanitizer makes them hex).
  expect(html).toMatch(/color: (?:#c5221f|rgb\(197, 34, 31\))/iu);
  expect(html).toMatch(/<mark data-color="#FFF3A3"/u);
  expect(html).not.toContain("<s>");
});

test("any text colour with the colour picker", async ({ page }) => {
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  await composer.getByRole("button", { name: /^Save draft/u }).waitFor();
  const editor = composer.getByRole("textbox", { name: "Message" });
  const toolbar = composer.getByRole("toolbar", { name: "Formatting" });
  await editor.click();
  await editor.press("ControlOrMeta+Home");
  await editor.pressSequentially("teal");
  await editor.press("Shift+Home");

  // Text style, font, colours, alignment… are behind "Aa".
  await toolbar.getByRole("button", { name: "Formatting options" }).click();
  // "Custom…" opens the system picker (not scriptable): set its value.
  await toolbar.getByRole("button", { name: "Text color" }).click();
  await page.getByRole("menuitem", { name: "Custom…" }).click();
  await composer.locator('input[type="color"]').first().fill("#0f766e");
  await expect(editor.locator("span[style]")).toHaveCSS(
    "color",
    "rgb(15, 118, 110)"
  );

  // The menu shows the custom colour as the chosen one.
  await toolbar.getByRole("button", { name: "Text color" }).click();
  await expect(
    page.getByRole("menuitemradio", { name: "#0F766E" })
  ).toHaveAttribute("aria-checked", "true");
});

/** Writes a short message to Bob and sends it. */
const writeAndSend = async (page: Page, subject: string) => {
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  const to = composer.getByRole("combobox", { name: "To" });
  await to.fill(`bob@example.com`);
  await to.press("Enter");
  await composer.getByRole("textbox", { name: "Subject" }).fill(subject);
  await composer
    .getByRole("textbox", { name: "Message" })
    .pressSequentially("Hi Bob");
  await composer.getByRole("button", { name: /^Send(?! options)/u }).click();
  return composer;
};

const at = (local: string, domain = "nebula.test"): string =>
  [local, domain].join("@");

/** A recipient chip (its text also has the avatar's initials). */
const chip = (text: string): RegExp =>
  new RegExp(`${text.replaceAll(".", String.raw`\.`)}$`, "u");

// RFC 8621 §6: the identity's Bcc goes on new emails (the user may take
// it off; another sender swaps it) and its Reply-To on what is sent.
test("the sender's Bcc and Reply-To come from its identity", async ({
  page,
}) => {
  const [jane] = jmap.identities;
  if (jane === undefined) {
    throw new Error("the fake has an identity");
  }
  jmap.identities.splice(
    0,
    1,
    {
      ...jane,
      bcc: [{ email: at("archive"), name: null }],
      replyTo: [{ email: at("help"), name: "Help desk" }],
    },
    {
      ...jane,
      bcc: [{ email: at("me", "home.test"), name: "Me" }],
      email: at("jane", "home.test"),
      id: "i2",
      name: "Jane at home",
      replyTo: null,
    }
  );
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  // The chips (the list's last item is the input).
  const bccChips = composer
    .getByRole("list", { name: "Bcc" })
    .getByRole("listitem")
    .filter({ has: page.getByRole("button", { name: /^Remove /u }) });
  await expect(bccChips).toHaveText([chip(at("archive"))]);

  // The user's own Bcc stays; the identity's is swapped.
  const bccInput = composer.getByRole("combobox", { name: "Bcc" });
  await bccInput.fill(at("boss"));
  await bccInput.press("Enter");
  await composer.getByRole("button", { name: /^From: /u }).click();
  await page.getByRole("menuitem", { name: /Jane at home/u }).click();
  await expect(bccChips).toHaveText([chip(at("boss")), chip("Me")]);
  await composer.getByRole("button", { name: /^From: /u }).click();
  await page.getByRole("menuitem", { name: /^Jane </u }).click();
  await expect(bccChips).toHaveText([chip(at("boss")), chip(at("archive"))]);

  const to = composer.getByRole("combobox", { name: "To" });
  await to.fill(at("bob", "example.com"));
  await to.press("Enter");
  await composer.getByRole("textbox", { name: "Subject" }).fill("From Jane");
  await composer.getByRole("button", { name: /^Send(?! options)/u }).click();
  await expect(composer).toBeHidden();

  expect(jmap.mailSends).toHaveLength(1);
  const sent = JSON.stringify(jmap.mailSends[0]);
  // Bcc recipients get it: they are in the envelope (RFC 8621 §7).
  for (const address of [at("bob", "example.com"), at("boss"), at("archive")]) {
    expect(sent).toContain(`"email":"${address}"`);
  }
  expect(sent).not.toContain(at("me", "home.test"));
  const email = JSON.stringify(
    jmap.created.findLast((created) =>
      JSON.stringify(created).includes("From Jane")
    )
  );
  expect(email).toContain(
    JSON.stringify({ email: at("help"), name: "Help desk" })
  );
});

/** The toasts (Base UI hides some from role queries: found by text). */
const notices = (page: Page) =>
  page.getByRole("region", { name: "Notifications" });

test("sending closes the composer and says «Sent»", async ({ page }) => {
  const composer = await writeAndSend(page, "Plain send");

  await expect(composer).toBeHidden();
  await expect(notices(page).getByText("Sent", { exact: true })).toBeVisible();
  expect(jmap.mailSends).toHaveLength(1);
});

test("sent but not filed: says so, never «couldn't send»", async ({ page }) => {
  jmap.submitOutcome = "notFiled";
  const composer = await writeAndSend(page, "Not filed");

  await expect(composer).toBeHidden();
  await expect(
    notices(page).getByText("Sent, but it's still in Drafts", { exact: false })
  ).toBeVisible();
  await expect(notices(page).getByText("Couldn't send.")).toHaveCount(0);
  expect(jmap.mailSends).toHaveLength(1);
});

test("unknown outcome: «we don't know if it was sent», nothing to resend", async ({
  page,
}) => {
  jmap.submitOutcome = "unknown";
  const composer = await writeAndSend(page, "Maybe sent");

  await expect(composer).toBeHidden();
  const notice = notices(page).getByText("We don't know if it was sent", {
    exact: false,
  });
  await expect(notice).toBeVisible();
  await expect(notices(page).getByText("Couldn't send.")).toHaveCount(0);
  // It stays until closed: it has a close button (timed toasts don't).
  // (By label: high-priority toasts are hidden from role queries.)
  await expect(notices(page).locator('[aria-label="Close"]')).toHaveCount(1);
  await expect(page.locator("section[data-composer]")).toHaveCount(0);
  expect(jmap.mailSends).toHaveLength(1);
});

// 2026-11-08, full BFF mode on Vercel: «Send» clicked when the bar was full
// did nothing. The bar fills when the file has left the browser; the blob
// id only comes with the server's answer (100–500 ms there, more through
// the proxy), and until then the click was refused as «still uploading».
// Now the click is kept and the email goes out once the upload ends.
test("Send while the last upload is being answered sends with the file", async ({
  page,
}) => {
  const { promise, resolve } = Promise.withResolvers<undefined>();
  jmap.uploadsGate = promise;
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  const to = composer.getByRole("combobox", { name: "To" });
  await to.fill("bob@example.com");
  await to.press("Enter");
  await composer.getByRole("textbox", { name: "Subject" }).fill("With file");
  await composer.locator("input[type=file][multiple]").setInputFiles({
    buffer: Buffer.from("hello"),
    mimeType: "text/plain",
    name: "notes.txt",
  });
  // The server hasn't answered yet. (In real life the bar may already be
  // full; a routed request reports no upload progress, so here it isn't.)
  await expect(
    composer.getByRole("progressbar", { name: "Uploading notes.txt" })
  ).toBeVisible();
  const sent = page.waitForRequest(
    (request) =>
      request.method() === "POST" &&
      (request.postData() ?? "").includes("EmailSubmission/set")
  );
  await composer.getByRole("button", { name: /^Send(?! options)/u }).click();
  resolve(undefined);
  jmap.uploadsGate = null;

  const request = await sent;
  expect(request.postData()).toContain('"blob-');
  await expect(composer).toBeHidden();
  await expect(notices(page).getByText("Sent", { exact: true })).toBeVisible();
  expect(jmap.mailSends).toHaveLength(1);
});

test("Send while uploading waits, but not for an upload that fails", async ({
  page,
}) => {
  const { promise, resolve } = Promise.withResolvers<undefined>();
  jmap.uploadsGate = promise;
  jmap.uploadsFail = true;
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  const to = composer.getByRole("combobox", { name: "To" });
  await to.fill("bob@example.com");
  await to.press("Enter");
  await composer.locator("input[type=file][multiple]").setInputFiles({
    buffer: Buffer.from("hello"),
    mimeType: "text/plain",
    name: "notes.txt",
  });
  await composer.getByRole("button", { name: /^Send(?! options)/u }).click();
  resolve(undefined);
  jmap.uploadsGate = null;

  // The failed file stays on screen and nothing goes out without it.
  await expect(composer.getByText("Couldn't upload it")).toBeVisible();
  await expect(composer).toBeVisible();
  await expect(
    composer.getByRole("button", { name: /^Send(?! options)/u })
  ).toBeEnabled();
  expect(jmap.mailSends).toHaveLength(0);
});

test("an attachment shows how its upload goes", async ({ page }) => {
  const { promise, resolve } = Promise.withResolvers<undefined>();
  jmap.uploadsGate = promise;
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  await composer.locator("input[type=file][multiple]").setInputFiles({
    buffer: Buffer.from("hello"),
    mimeType: "text/plain",
    name: "notes.txt",
  });
  const bar = composer.getByRole("progressbar", {
    name: "Uploading notes.txt",
  });
  await expect(bar).toBeVisible();
  // JMAP is on another origin here, so the upload goes by XHR and the bar
  // is a real one (same-origin uploads, with fetch, get an indeterminate
  // bar: unit tests in attachments.test.ts and jmap-client's xhr.test.ts).
  await expect(bar).toHaveAttribute("aria-valuenow", /^\d/u);
  await expect(bar).not.toHaveAttribute("data-indeterminate");
  resolve(undefined);
  jmap.uploadsGate = null;
  await expect(bar).toHaveCount(0);
  await expect(
    composer.getByRole("list", { name: "Attachments" }).getByText("notes.txt")
  ).toBeVisible();
});

// The body is serialized when needed, not on every keystroke
// (use-compose.ts `editBody`): what was typed just before acting is there.
test("Send right after typing sends the last keystrokes", async ({ page }) => {
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  const to = composer.getByRole("combobox", { name: "To" });
  await to.fill(at("bob", "example.com"));
  await to.press("Enter");
  await composer.getByRole("textbox", { name: "Subject" }).fill("Quick one");
  await composer
    .getByRole("textbox", { name: "Message" })
    .pressSequentially("See you at nine");
  await composer.getByRole("button", { name: /^Send(?! options)/u }).click();
  await expect(composer).toBeHidden();

  expect(jmap.mailSends).toHaveLength(1);
  const sent = JSON.stringify(
    jmap.created.findLast((created) =>
      JSON.stringify(created).includes("Quick one")
    )
  );
  expect(sent).toContain("See you at nine");
});

test("minimizing right after typing keeps the text", async ({ page }, info) => {
  test.skip(info.project.name !== "desktop", "phones open it full screen");
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  await composer
    .getByRole("textbox", { name: "Message" })
    .pressSequentially("Keep every word");
  await composer.getByRole("button", { name: "Minimize" }).click();
  await page.getByRole("button", { name: "Restore" }).click();
  await expect(
    page
      .locator("section[data-composer]")
      .getByRole("textbox", { name: "Message" })
  ).toContainText("Keep every word");
});
