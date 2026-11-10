// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Contacts through the real bridge with Radicale
// what users do in the app (list, search, create, edit, delete,
// groups, importing a .vcf, suggestions in the composer), checked on the
// CardDAV server itself, and a card another client writes there showing
// up in the app. Alice's own Dovecot/Radicale user; everything this run
// makes has RUN in it and is deleted at the end. Nothing is sent.

import type { JmapClient } from "@nebulamail/jmap-client";
import { CAPABILITIES } from "@nebulamail/jmap-client";
import type { Browser, Locator, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { openComposer, shot } from "../e2e-live/ui";
import { davCollections, davPut, davText } from "./dav";
import { DOVECOT } from "./env";
import { jmapFor, signIn } from "./session";

const USER = "alice";
/** In every name this run writes (letters only: a word to search for). */
const RUN = `zq${Date.now().toString(36).replaceAll(/\d/gu, "")}`;

/** A sync round of the bridge (poll_seconds = 10) plus the push. */
const SYNC = 30_000;

test.describe.configure({ mode: "serial", timeout: 120_000 });

const signedIn = async (browser: Browser): Promise<Page> => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await signIn(page, USER, DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  return page;
};

const list = (page: Page): Locator =>
  page.getByRole("list", { name: "Contacts list" });

const detail = (page: Page): Locator =>
  page.getByRole("region", { name: "Contact" });

/** The contacts page, loaded (a click before hydration does nothing). */
const openContacts = async (page: Page): Promise<void> => {
  await page.goto("/contacts");
  await expect(page.getByText(/^\d+ contacts?$/u)).toBeVisible();
};

const cardsText = async (): Promise<string> =>
  await davText(USER, "addressbook");

/** The vCard on the DAV server with this text in it ("" if none). */
const cardWith = async (needle: string): Promise<string> => {
  const text = await cardsText();
  return (
    text.split(/(?=BEGIN:VCARD)/u).find((card) => card.includes(needle)) ?? ""
  );
};

/** Our default address book's id. */
const defaultBook = async (client: JmapClient): Promise<string> => {
  const accountId = client.primaryAccountId(CAPABILITIES.contacts);
  const [books] = await client.request((b) => [
    b.call("AddressBook/get", { accountId }),
  ]);
  const book = books.list.find((entry) => entry.isDefault) ?? books.list[0];
  if (book === undefined) {
    throw new Error("No address book (the bridge makes one)");
  }
  return book.id;
};

/** Contacts made straight with JMAP (for the tests that need some). */
const createCards = async (
  client: JmapClient,
  people: readonly { readonly name: string; readonly email: string }[]
): Promise<void> => {
  const accountId = client.primaryAccountId(CAPABILITIES.contacts);
  const book = await defaultBook(client);
  const [result] = await client.request((b) => [
    b.call("ContactCard/set", {
      accountId,
      create: Object.fromEntries(
        people.map((person, index) => [
          `c${String(index)}`,
          {
            "@type": "Card",
            addressBookIds: { [book]: true },
            emails: { e1: { address: person.email } },
            name: { full: person.name },
            uid: `urn:uuid:${crypto.randomUUID()}`,
            version: "1.0",
          },
        ])
      ),
    }),
  ]);
  expect(Object.keys(result.created)).toHaveLength(people.length);
};

test.afterAll(async ({ browser }) => {
  const page = await signedIn(browser);
  const client = await jmapFor(page);
  const accountId = client.primaryAccountId(CAPABILITIES.contacts);
  const [all] = await client.request((b) => [
    b.call("ContactCard/get", { accountId, properties: ["name", "emails"] }),
  ]);
  const ours = all.list
    .filter((card) => JSON.stringify(card).includes(RUN))
    .map((card) => card.id);
  if (ours.length > 0) {
    await client.request((b) => [
      b.call("ContactCard/set", { accountId, destroy: ours }),
    ]);
  }
  await page.context().close();
});

test("create, find, edit and delete a contact, on the DAV server too", async ({
  browser,
}) => {
  const page = await signedIn(browser);
  await openContacts(page);
  const email = `dora.${RUN}@example.com`;
  await expect(async () => {
    await page
      .getByRole("button", { exact: true, name: "New contact" })
      .click();
    await expect(page.getByRole("dialog")).toBeVisible({ timeout: 1000 });
  }).toPass();
  const editor = page.getByRole("dialog", { name: "New contact" });
  await editor.getByLabel("First name").fill("Dora");
  await editor.getByLabel("Last name").fill("Díaz");
  await editor.getByRole("button", { name: "Add email" }).click();
  await editor.getByLabel("Email", { exact: true }).fill(email);
  await editor.getByLabel("Company").fill(`Acme ${RUN}`);
  await editor.getByRole("button", { name: "Save" }).click();
  await expect(
    detail(page).getByRole("heading", { name: "Dora Díaz" })
  ).toBeVisible();
  // The bridge wrote it to Radicale (a vCard, with what was typed).
  await expect.poll(cardsText).toContain(`EMAIL:${email}`);
  expect(await cardsText()).toContain(`ORG:Acme ${RUN}`);

  // Searching finds it by the start of a word, without accents.
  await page.getByLabel("Search contacts").fill(RUN.slice(0, 6));
  await expect(list(page).getByRole("button")).toHaveCount(1);
  await page.getByLabel("Search contacts").fill("diaz");
  await expect(
    list(page).getByRole("button", { name: /Dora Díaz/u })
  ).toBeVisible();
  await page.screenshot(shot("bridge-contacts"));

  await detail(page).getByRole("button", { name: "Edit" }).click();
  const edit = page.getByRole("dialog", { name: "Edit contact" });
  await edit.getByLabel("Job title").fill("CTO");
  await edit.getByRole("button", { name: "Save" }).click();
  await expect(edit).toBeHidden();
  await expect(detail(page).getByText("CTO")).toBeVisible();
  await expect.poll(cardsText).toContain("TITLE:CTO");
  // What wasn't edited is still there.
  expect(await cardsText()).toContain(`EMAIL:${email}`);

  await detail(page).getByRole("button", { name: "Delete" }).click();
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Delete" })
    .click();
  await expect(list(page).getByRole("button", { name: /Dora/u })).toHaveCount(
    0
  );
  await expect.poll(cardsText).not.toContain(email);
  await page.context().close();
});

test("a card another client puts on the server shows up, and keeps its extras", async ({
  browser,
}) => {
  const [book] = await davCollections(USER, "addressbook");
  if (book === undefined) {
    throw new Error("No address book on the DAV server");
  }
  // vCard 3.0 the way Apple writes it, with a label the app doesn't show.
  await davPut(
    USER,
    book,
    `${RUN}-eva.vcf`,
    [
      "BEGIN:VCARD",
      "VERSION:3.0",
      "PRODID:-//Apple Inc.//iPhone OS 18.0//EN",
      `UID:${RUN}-eva`,
      `N:Ortega ${RUN};Eva;;;`,
      `FN:Eva Ortega ${RUN}`,
      `item1.EMAIL;type=INTERNET:eva.${RUN}@example.com`,
      "item1.X-ABLabel:_$!<Other>!$_",
      "END:VCARD",
      "",
    ].join("\r\n"),
    "text/vcard"
  );

  const page = await signedIn(browser);
  await openContacts(page);
  const eva = list(page).getByRole("button", { name: /Eva Ortega/u });
  // The bridge's next sync round, then the push to the open page.
  await expect(eva).toBeVisible({ timeout: SYNC });
  await eva.click();
  await expect(detail(page).getByText(`eva.${RUN}@example.com`)).toBeVisible();

  await detail(page).getByRole("button", { name: "Edit" }).click();
  const edit = page.getByRole("dialog", { name: "Edit contact" });
  await edit.getByLabel("Job title").fill("Pilot");
  await edit.getByRole("button", { name: "Save" }).click();
  await expect(edit).toBeHidden();
  await expect
    .poll(async () => await cardWith(`UID:${RUN}-eva`))
    .toContain("TITLE:Pilot");
  // Still a 3.0 card, with Apple's label.
  const card = await cardWith(`UID:${RUN}-eva`);
  expect(card).toContain("VERSION:3.0");
  // (Property names are case-insensitive: calcard writes X-ABLABEL.)
  expect(card.toLowerCase()).toContain("item1.x-ablabel:_$!<other>!$_");
  await page.context().close();
});

test("a group with members, and suggestions in the composer", async ({
  browser,
}) => {
  const page = await signedIn(browser);
  await createCards(await jmapFor(page), [
    { email: `lia.${RUN}@example.com`, name: `Lia Sanz ${RUN}` },
    { email: `teo.${RUN}@example.com`, name: `Teo Vidal ${RUN}` },
  ]);
  await openContacts(page);
  await expect(
    list(page).getByRole("button", { name: /Teo Vidal/u })
  ).toBeVisible();

  await page.getByRole("button", { name: "More contact actions" }).click();
  await page.getByRole("menuitem", { name: "New group" }).click();
  const editor = page.getByRole("dialog", { name: "New group" });
  await editor.getByLabel("Group name").fill(`Crew ${RUN}`);
  await editor.getByLabel("Search contacts to add").fill(RUN);
  await editor.getByRole("checkbox", { name: /Lia Sanz/u }).check();
  await editor.getByRole("checkbox", { name: /Teo Vidal/u }).check();
  await editor.getByRole("button", { name: "Save" }).click();
  await expect(editor).toBeHidden();

  await page.getByRole("radio", { name: "Groups" }).click();
  await list(page)
    .getByRole("button", { name: new RegExp(`Crew ${RUN}`, "u") })
    .click();
  await expect(
    detail(page).getByRole("link", { name: /Lia Sanz/u })
  ).toBeVisible();
  await expect(
    detail(page).getByRole("link", { name: /Teo Vidal/u })
  ).toBeVisible();
  await page.screenshot(shot("bridge-contacts-group"));
  await expect.poll(cardsText).toContain(`FN:Crew ${RUN}`);
  const group = await cardWith(`FN:Crew ${RUN}`);
  // vCard 4.0 (RFC 6350 KIND/MEMBER) only where the server announces it;
  // else 3.0 (RFC 6352 §6.2.2), with Apple's X-ADDRESSBOOKSERVER-* form.
  const prefix = /^VERSION:4\.0\r?$/mu.test(group)
    ? ""
    : "X-ADDRESSBOOKSERVER-";
  expect(group).toMatch(new RegExp(`^${prefix}KIND:group\\r?$`, "imu"));
  expect(
    group.match(new RegExp(`^${prefix}MEMBER:urn:uuid:`, "gmu")) ?? []
  ).toHaveLength(2);

  // The address book in the composer's suggestions.
  await page.goto("/mail/inbox");
  const composer = await openComposer(page);
  await composer.getByRole("combobox", { name: "To" }).fill("teo");
  const option = page.getByRole("option", { name: /Teo Vidal/u });
  await expect(option).toBeVisible();
  await page.screenshot(shot("bridge-contacts-suggestion"));
  await option.click();
  await expect(composer.getByText(`Teo Vidal ${RUN}`)).toBeVisible();
  await composer.getByRole("button", { name: "Discard draft" }).click();
  await page.context().close();
});

test("import a .vcf file: preview first, then only what is confirmed", async ({
  browser,
}) => {
  const page = await signedIn(browser);
  await openContacts(page);
  const file = [
    "BEGIN:VCARD",
    "VERSION:4.0",
    `FN:Uma Prieto ${RUN}`,
    `EMAIL:uma.${RUN}@example.com`,
    "END:VCARD",
    "BEGIN:VCARD",
    "VERSION:3.0",
    `FN:Iker Gil ${RUN}`,
    `N:Gil ${RUN};Iker;;;`,
    `TEL;TYPE=CELL:+34 600 000 000`,
    "END:VCARD",
    "",
  ].join("\r\n");
  await page.locator('input[type="file"][accept^=".vcf"]').setInputFiles({
    buffer: Buffer.from(file),
    mimeType: "text/vcard",
    name: `${RUN}.vcf`,
  });
  const dialog = page.getByRole("dialog", { name: "Import contacts" });
  await expect(dialog.getByText(`Uma Prieto ${RUN}`)).toBeVisible();
  await expect(dialog.getByText(`Iker Gil ${RUN}`)).toBeVisible();
  await page.screenshot(shot("bridge-contacts-import"));
  // Nothing written before confirming.
  expect(await cardsText()).not.toContain(`Uma Prieto ${RUN}`);
  await dialog.getByRole("button", { name: "Import 2 contacts" }).click();
  await expect(page.getByText("Imported 2 contacts")).toBeVisible();
  await expect(
    list(page).getByRole("button", { name: /Iker Gil/u })
  ).toBeVisible();
  await expect.poll(cardsText).toContain(`FN:Uma Prieto ${RUN}`);
  expect(await cardsText()).toContain(`FN:Iker Gil ${RUN}`);
  await page.context().close();
});
