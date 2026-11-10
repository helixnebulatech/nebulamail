// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Filters and the vacation response through the bridge's ManageSieve side
// and Dovecot's Pigeonhole: the same screens as with
// Stalwart. The bridge doesn't announce `mailboxid`, so folders go by the
// name the IMAP server knows them by.
//
// Its own Dovecot user (Dovecot takes any name), so other tests' scripts
// are not touched; everything is removed at the end.

import type { BlobId, JmapClient } from "@nebulamail/jmap-client";
import { IMAP_NAME_PROPERTY } from "@nebulamail/jmap-client";
import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { chooseOption } from "../e2e/select";
import { DOVECOT } from "./env";
import { jmapFor, signIn } from "./session";

const USER = "filters-e2e";
// Each save is several ManageSieve sessions (upload, validate, set, then
// reading it all back before the editor closes), and showing the banner
// a few more: slow on a busy machine.
const SAVED = { timeout: 20_000 } as const;
const FOLDER = "Filtered news";

const signedIn = async (page: Page): Promise<JmapClient> => {
  await signIn(page, USER, DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  return await jmapFor(page);
};

const scripts = async (client: JmapClient) => {
  const [result] = await client.request((b) => [
    b.call("SieveScript/get", { accountId: client.primaryAccountId() }),
  ]);
  return result.list;
};

const content = async (client: JmapClient, blobId: BlobId) => {
  const response = await client.download({
    accountId: client.primaryAccountId(),
    blobId,
    name: "script.siv",
    type: "application/sieve",
  });
  return await response.text();
};

/** The script `name`, or the active one. */
const findScript = async (client: JmapClient, name?: string) => {
  const all = await scripts(client);
  return all.find((item) =>
    name === undefined ? item.isActive : item.name === name
  );
};

/**
 * The text of the script (`name`, or the active one) if it is active. A
 * script the app rewrites between the list and the download has a new
 * blob id (the old one is gone): "" then, and the poll asks again.
 */
const activeText = async (client: JmapClient, name?: string) => {
  const found = await findScript(client, name);
  if (found?.isActive !== true) {
    return "";
  }
  try {
    return await content(client, found.blobId);
  } catch {
    return "";
  }
};

const cleanUpOnce = async (client: JmapClient) => {
  const accountId = client.primaryAccountId();
  const all = await scripts(client);
  await client.request((b) => [
    b.call("SieveScript/set", { accountId, onSuccessDeactivateScript: true }),
  ]);
  if (all.length > 0) {
    await client.request((b) => [
      b.call("SieveScript/set", {
        accountId,
        destroy: all.map((script) => script.id),
      }),
    ]);
  }
  const [boxes] = await client.request((b) => [
    b.call("Mailbox/get", { accountId, properties: ["name"] }),
  ]);
  const folder = boxes.list.find((box) => box.name === FOLDER);
  if (folder !== undefined) {
    await client.request((b) => [
      b.call("Mailbox/set", {
        accountId,
        destroy: [folder.id],
        onDestroyRemoveEmails: true,
      }),
    ]);
  }
};

/** The folder's name on the IMAP server (`INBOX.…` on some). */
const imapName = async (client: JmapClient) => {
  const [boxes] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId: client.primaryAccountId(),
      properties: ["name", IMAP_NAME_PROPERTY],
    }),
  ]);
  const folder = boxes.list.find((box) => box.name === FOLDER);
  return folder?.[IMAP_NAME_PROPERTY];
};

/** Retried: requests in flight from the app may hit the bridge's limit. */
const cleanUp = async (client: JmapClient) => {
  await expect(async () => {
    await cleanUpOnce(client);
  }).toPass({ timeout: 20_000 });
};

// Many saves each: the default 30 s can't hold them on a busy machine.
test.slow();

test.beforeEach(async ({ browser }) => {
  const context = await browser.newContext();
  const client = await signedIn(await context.newPage());
  await cleanUp(client);
  const [made] = await client.request((b) => [
    b.call("Mailbox/set", {
      accountId: client.primaryAccountId(),
      create: { f: { name: FOLDER } },
    }),
  ]);
  expect(made.created?.f).toBeDefined();
  await context.close();
});

test.afterAll(async ({ browser }) => {
  const context = await browser.newContext();
  await cleanUp(await signedIn(await context.newPage()));
  await context.close();
});

test("filters: a rule saved through ManageSieve files by folder path", async ({
  page,
}) => {
  const client = await signedIn(page);
  await page.goto("/settings/filters");
  await expect(page.getByText(/No filters yet/u)).toBeVisible();
  await page.getByRole("button", { name: "New filter" }).click();
  await page.getByLabel("Name").fill("Weekly news");
  await page.getByLabel("Condition 1: value").fill("weekly.example");
  await page.getByRole("button", { name: "Add action" }).click();
  await chooseOption(page.getByLabel("Action 1: folder"), { label: FOLDER });
  await page.getByRole("button", { name: "Save filter" }).click();
  await expect(page.getByText("Weekly news")).toBeVisible(SAVED);

  // On the server: our script, active, with the folder by its IMAP name.
  await expect
    .poll(async () => {
      const active = await findScript(client);
      return active?.name;
    }, SAVED)
    .toBe("nebula-mail-filters");
  const text = await activeText(client);
  expect(text).toContain('# Rule: "Weekly news"');
  const folder = await imapName(client);
  expect(folder).toContain(FOLDER);
  expect(text).toContain(`fileinto "${folder}";`);
  expect(text).not.toContain("mailboxid");

  // Read back as rules (not as a hand-written script), folder included.
  await page.reload();
  await expect(page.getByText("Weekly news")).toBeVisible();
  await expect(page.getByText(/written by hand/u)).toHaveCount(0);
  await expect(page.getByText(FOLDER).first()).toBeVisible();

  // Off and delete, saved at once.
  await page.getByRole("switch", { name: "Filter Weekly news on" }).click();
  await expect
    .poll(async () => await activeText(client), SAVED)
    .toContain("if false {");
});

test("vacation response: on with our filters keeps both, then off", async ({
  page,
}) => {
  const client = await signedIn(page);
  // Our filters first, so turning the vacation response on includes it.
  await page.goto("/settings/filters");
  await page.getByRole("button", { name: "New filter" }).click();
  await page.getByLabel("Name").fill("Receipts");
  await page.getByLabel("Condition 1: value").fill("shop.example");
  await page.getByRole("button", { name: "Add action" }).click();
  await chooseOption(page.getByLabel("Action 1: folder"), { label: FOLDER });
  await page.getByRole("button", { name: "Save filter" }).click();
  await expect(page.getByText("Receipts")).toBeVisible(SAVED);

  await page.goto("/settings/vacation");
  await page.getByRole("switch", { name: "Vacation response on" }).click();
  await page.getByLabel("Subject").fill("Out until Monday");
  await page.getByRole("textbox", { name: "Message" }).fill("I'm away.");
  await page.getByRole("button", { name: "Save", exact: true }).click();

  // Our script stays active and includes the bridge's vacation script.
  await expect
    .poll(async () => await activeText(client, "nebula-mail-filters"), SAVED)
    .toContain('include :personal :optional "vacation";');
  const vacation = await findScript(client, "vacation");
  expect(vacation?.isActive).toBe(false);
  const vacationText =
    vacation === undefined ? "" : await content(client, vacation.blobId);
  expect(vacationText).toContain("# Nebula Mail vacation response");
  expect(vacationText).toContain(':subject "Out until Monday"');
  const [response] = await client.request((b) => [
    b.call("VacationResponse/get", { accountId: client.primaryAccountId() }),
  ]);
  expect(response.list[0]).toMatchObject({ subject: "Out until Monday" });

  await page.goto("/mail/inbox");
  await expect(page.getByText("Your vacation response is on.")).toBeVisible(
    SAVED
  );

  // Off: the include goes, the filters stay.
  await page.goto("/settings/vacation");
  await page.getByRole("switch", { name: "Vacation response on" }).click();
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await expect
    .poll(async () => await activeText(client, "nebula-mail-filters"), SAVED)
    .toContain('# Rule: "Receipts"');
  await expect
    .poll(async () => await activeText(client, "nebula-mail-filters"), SAVED)
    .not.toContain("include :personal");
  await page.goto("/mail/inbox");
  await expect(page.getByText("Your vacation response is on.")).toHaveCount(0);
});
