// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The synced preferences in a mail folder through the
// bridge and Dovecot: the bridge has no FileNode, so the
// copy is a message in the unsubscribed «Nebula Mail» folder, imported
// with Email/import (an IMAP APPEND) and the older one destroyed.
//
// Its own Dovecot user (Dovecot takes any name), so that other tests that
// use alice see no «Nebula Mail» folder; the folder is removed at the end.

import type { JmapClient } from "@nebulamail/jmap-client";
import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { EMPTY_DOC, withValue } from "../src/prefs-sync/model";
import { SYNC_MAILBOX, backendFor, remoteFor } from "../src/prefs-sync/remote";
import { DOVECOT } from "./env";
import { jmapFor, signIn } from "./session";

const USER = "prefs-e2e";

const findMailbox = async (client: JmapClient) => {
  const [boxes] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId: client.primaryAccountId(),
      properties: ["name", "parentId", "isSubscribed"],
    }),
  ]);
  return boxes.list.find(
    (box) => box.name === SYNC_MAILBOX && (box.parentId ?? null) === null
  );
};

const removeMailbox = async (client: JmapClient) => {
  const box = await findMailbox(client);
  if (box === undefined) {
    return;
  }
  const [result] = await client.request((b) => [
    b.call("Mailbox/set", {
      accountId: client.primaryAccountId(),
      destroy: [box.id],
      onDestroyRemoveEmails: true,
    }),
  ]);
  expect(result.destroyed).toContain(box.id);
};

const messagesIn = async (client: JmapClient) => {
  const box = await findMailbox(client);
  if (box === undefined) {
    return [];
  }
  const [query] = await client.request((b) => [
    b.call("Email/query", {
      accountId: client.primaryAccountId(),
      filter: { inMailbox: box.id },
    }),
  ]);
  return query.ids;
};

const signedIn = async (page: Page) => {
  await signIn(page, USER, DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  return await jmapFor(page);
};

test.afterEach(async ({ browser }) => {
  // Clean up with a fresh sign-in (the test's own pages may be closed).
  const context = await browser.newContext();
  const page = await context.newPage();
  await removeMailbox(await signedIn(page));
  await context.close();
});

test("remote.ts: writes, replaces and reads back the copy in a mail folder", async ({
  page,
}) => {
  const client = await signedIn(page);
  await removeMailbox(client);
  // No FileNode on the bridge: the mail folder, only when allowed.
  expect(backendFor(client, false)).toBeNull();
  expect(backendFor(client, true)).toBe("mail");

  const remote = remoteFor(client, "mail");
  await expect(remote.read()).resolves.toBeNull();
  const first = withValue(EMPTY_DOC, "theme", "dark", 1);
  await remote.write(first);
  await expect(remote.read()).resolves.toStrictEqual(first);
  const second = withValue(
    withValue(first, "vacationPausedScript", { id: "x", name: "Ñ 🌌" }, 2),
    "theme",
    "light",
    3
  );
  await remote.write(second);
  await expect(remote.read()).resolves.toStrictEqual(second);

  // One message, in an unsubscribed folder.
  const box = await findMailbox(client);
  expect(box?.isSubscribed).toBe(false);
  expect(await messagesIn(client)).toHaveLength(1);
});

test("a choice on one device reaches another through the mail folder", async ({
  browser,
}) => {
  // Device A: turns the mail folder on and picks a density.
  const a = await browser.newContext();
  const pageA = await a.newPage();
  const client = await signedIn(pageA);
  await removeMailbox(client);
  await pageA.goto("/settings/general");
  await expect(pageA.getByText(/stay on this device/u)).toBeVisible();
  // A click before the page has hydrated does nothing: until it is on.
  const switchA = pageA.getByRole("switch", {
    name: "Save settings in a mail folder",
  });
  await expect(async () => {
    await switchA.click();
    await expect(switchA).toBeChecked({ timeout: 1000 });
  }).toPass();
  await pageA.getByRole("radio", { name: "Comfortable" }).click();
  await expect(
    pageA.getByText(/saved in the “Nebula Mail” folder/u)
  ).toBeVisible();
  await expect
    .poll(async () => {
      const doc = await remoteFor(client, "mail").read();
      return doc?.values.density?.value;
    })
    .toBe("comfortable");
  expect(await messagesIn(client)).toHaveLength(1);
  // The folder is not in our folder list (this user has no other folder;
  // the logo link is also called "Nebula Mail").
  await pageA.goto("/mail/inbox");
  await expect(pageA.getByText("No folders yet")).toBeVisible();
  await a.close();

  // Device B: the same account in a new browser; once it allows the mail
  // folder too, it takes A's density.
  const b = await browser.newContext();
  const pageB = await b.newPage();
  await signedIn(pageB);
  await pageB.goto("/settings/general");
  const switchB = pageB.getByRole("switch", {
    name: "Save settings in a mail folder",
  });
  await expect(async () => {
    await switchB.click();
    await expect(switchB).toBeChecked({ timeout: 1000 });
  }).toPass();
  await expect(pageB.getByRole("radio", { name: "Comfortable" })).toBeChecked();
  await expect
    .poll(async () => {
      const cookies = await b.cookies();
      return cookies.find((cookie) => cookie.name === "wm-prefs")?.value ?? "";
    })
    .toContain("comfortable");
  await b.close();
});
