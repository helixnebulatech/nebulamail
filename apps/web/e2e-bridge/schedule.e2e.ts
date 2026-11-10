// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Undo send and scheduled send against the real bridge
// the bridge announces FUTURERELEASE and holds the message itself,
// so the app works as with Stalwart. Nothing here goes out: the undo and
// the scheduled message are both cancelled (the plain send of mail.e2e.ts
// is the one held 10 s and then sent). The last test (`@postfix`) lets one
// go out after alice signs out (`send_while_signed_out`, start-bridge.ts)
// and checks that bob gets it.

import type { JmapClient } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  EmailId,
  SEND_WHEN_SIGNED_OUT,
  sendsWhenSignedOut,
} from "@nebulamail/jmap-client";
import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import { DOVECOT, SMTP } from "./env";
import { jmapFor, signIn } from "./session";

test.use({ locale: "en-US" });

/** Builds an address (keeps the test free of address literals). */
const addr = (local: string): string => `${local}@nebulamail.test`;

const notices = (page: Page) =>
  page.getByRole("region", { name: "Notifications" });

const write = async (page: Page, subject: string) => {
  const composer = page.locator("section[data-composer]");
  const to = composer.getByRole("combobox", { name: "To" });
  // A click before the page has hydrated does nothing: click until it opens.
  await expect(async () => {
    await page.getByRole("button", { name: /^New message/u }).click();
    await expect(to).toBeVisible({ timeout: 1000 });
  }).toPass();
  await to.fill(addr("bob"));
  await to.press("Enter");
  await composer.getByRole("textbox", { name: "Subject" }).fill(subject);
  await composer
    .getByRole("textbox", { name: "Message" })
    .fill("Held by the bridge browser test. Safe to delete.");
  return composer;
};

const Submissions = z.object({
  list: z.array(
    z.object({
      emailId: EmailId,
      envelope: z.object({
        mailFrom: z.object({
          parameters: z.record(z.string(), z.string()).nullable(),
        }),
      }),
      id: z.string(),
      undoStatus: z.enum(["pending", "final", "canceled"]),
    })
  ),
});

/** The submissions of the emails with this subject. */
const submissionsOf = async (client: JmapClient, subject: string) => {
  const accountId = client.primaryAccountId();
  const [emails] = await client.request((b) => [
    b.call("Email/query", { accountId, filter: { subject } }),
  ]);
  const [, got] = await client.request((b) => {
    const found = b.call("EmailSubmission/query", {
      accountId,
      filter: { emailIds: emails.ids },
      sort: [{ isAscending: true, property: "sentAt" }],
    });
    return [
      found,
      b.call("EmailSubmission/get", {
        "#ids": found.ref("/ids"),
        accountId,
      }),
    ] as const;
  });
  return Submissions.parse(got).list;
};

test.beforeEach(async ({ page }) => {
  await signIn(page, "alice", DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
});

/** The ids of the emails with this subject in the account. */
const emailsWithSubject = async (client: JmapClient, subject: string) => {
  const [found] = await client.request((b) => [
    b.call("Email/query", {
      accountId: client.primaryAccountId(),
      filter: { subject },
    }),
  ]);
  return found.ids;
};

/** Holds a message to bob until `at`, marked to go out after sign-out. */
const scheduleSignedOut = async (
  client: JmapClient,
  subject: string,
  at: Date
) => {
  const accountId = client.primaryAccountId();
  const [boxes, identities] = await client.request((b) => [
    b.call("Mailbox/get", { accountId, properties: ["role"] }),
    b.call("Identity/get", { accountId, properties: ["email"] }),
  ]);
  const drafts = boxes.list.find((box) => box.role === "drafts")?.id;
  const sent = boxes.list.find((box) => box.role === "sent")?.id;
  const [identity] = identities.list;
  if (identity === undefined || drafts === undefined || sent === undefined) {
    throw new Error("alice has no identity, Drafts or Sent");
  }
  const holdUntil = new Date(Math.ceil(at.getTime() / 1000) * 1000)
    .toISOString()
    .replace(/\.\d{3}Z$/u, "Z");
  const [, submitted] = await client.request((b) => {
    b.using(CAPABILITIES.bridge);
    return [
      b.call("Email/set", {
        accountId,
        create: {
          draft: {
            bodyValues: {
              t: { value: "Sent after sign-out. Safe to delete." },
            },
            from: [{ email: identity.email }],
            keywords: { $draft: true, $seen: true },
            mailboxIds: { [drafts]: true as const },
            subject,
            textBody: [{ partId: "t", type: "text/plain" }],
            to: [{ email: addr("bob") }],
          },
        },
      }),
      b.call("EmailSubmission/set", {
        accountId,
        create: {
          send: {
            [SEND_WHEN_SIGNED_OUT]: true,
            emailId: "#draft",
            envelope: {
              mailFrom: {
                email: identity.email,
                parameters: { HOLDUNTIL: holdUntil },
              },
              rcptTo: [{ email: addr("bob") }],
            },
            identityId: identity.id,
          },
        },
        onSuccessUpdateEmail: {
          "#send": {
            "keywords/$draft": null,
            [`mailboxIds/${drafts}`]: null,
            [`mailboxIds/${sent}`]: true as const,
          },
        },
      }),
    ] as const;
  });
  expect(submitted.created.send?.undoStatus).toBe("pending");
};

test("the bridge holds a send for undo, and undo cancels it", async ({
  page,
}) => {
  const subject = `Nebula bridge undo ${Date.now()}`;
  const composer = await write(page, subject);
  await composer.getByRole("button", { name: /^Send/u }).click();
  await expect(composer).toBeHidden();
  await notices(page).getByRole("button", { name: "Undo" }).click();
  await expect(
    notices(page).getByText("Sending cancelled. Your draft is back.")
  ).toBeVisible();

  const client = await jmapFor(page);
  const submissions = await submissionsOf(client, subject);
  expect(submissions).toHaveLength(1);
  expect(submissions[0]?.undoStatus).toBe("canceled");
  expect(submissions[0]?.envelope.mailFrom.parameters).toStrictEqual({
    HOLDFOR: "10",
  });

  // The draft is back in Drafts: clean it up.
  const accountId = client.primaryAccountId();
  await client.request((b) => [
    b.call("Email/set", {
      accountId,
      destroy: submissions.map((item) => item.emailId),
    }),
  ]);
});

test("schedules, changes the time and cancels from «Scheduled»", async ({
  page,
}) => {
  const subject = `Nebula bridge scheduled ${Date.now()}`;
  const composer = await write(page, subject);
  await composer.getByRole("button", { name: "More send options" }).click();
  await page
    .getByRole("menu")
    .getByRole("menuitem", { name: /^Tomorrow morning/u })
    .click();
  await expect(composer).toBeHidden();
  await expect(
    notices(page)
      .getByText(/^Scheduled for /u)
      .first()
  ).toBeVisible();

  await page
    .getByRole("navigation", { name: /./u })
    .getByRole("link", { name: "Scheduled" })
    .first()
    .click();
  await expect(page).toHaveURL(/\/mail\/scheduled$/u);
  const row = page
    .getByRole("list", { name: "Scheduled" })
    .getByRole("listitem")
    .filter({ hasText: subject });
  await expect(row).toBeVisible();
  await expect(row).toContainText(`To: ${addr("bob")}`);
  await expect(row).toContainText(/Sends .*8:00/u);

  // Change the time: the bridge cancels the old one, holds a new one.
  await row.getByRole("button", { name: "Change time" }).click();
  const dialog = page.getByRole("dialog", { name: "Change send time" });
  await dialog.getByRole("button", { name: /^Tomorrow afternoon/u }).click();
  await expect(
    notices(page)
      .getByText(/^Time changed: sends /u)
      .first()
  ).toBeVisible();
  await expect(row).toContainText(/Sends .*2:00/u);

  // Cancel: back to Drafts.
  await row.getByRole("button", { name: "Cancel send" }).click();
  await expect(
    notices(page).getByText("Send cancelled. The message is in Drafts.")
  ).toBeVisible();
  await expect(row).toBeHidden();

  const client = await jmapFor(page);
  const submissions = await submissionsOf(client, subject);
  expect(submissions.map((item) => item.undoStatus)).toStrictEqual([
    "canceled",
    "canceled",
  ]);
  expect(
    submissions.every(
      (item) => item.envelope.mailFrom.parameters?.HOLDUNTIL !== undefined
    )
  ).toBe(true);
  const accountId = client.primaryAccountId();
  await client.request((b) => [
    b.call("Email/set", {
      accountId,
      destroy: [...new Set(submissions.map((item) => item.emailId))],
    }),
  ]);
});

test(
  "a message marked to go out after sign-out reaches bob with alice signed out",
  { tag: "@postfix" },
  async ({ browser, page }) => {
    test.skip(
      SMTP.name !== "postfix",
      "needs the test Postfix: E2E_BRIDGE_SMTP=postfix (docker/README.md)"
    );
    // Held at least a minute, then up to a minute more for the scheduler.
    // It tests the sign-out path only if alice has no other session left:
    // earlier tests of the run never sign out, so run it alone (`--grep
    // "after sign-out"`; each run starts the bridge with a fresh database)
    // and look for "password kept after sign-out" in the bridge's log.
    test.setTimeout(300_000);
    const client = await jmapFor(page);
    expect(sendsWhenSignedOut(client.session)).toBe(true);
    const subject = `Nebula bridge signed out ${Date.now()}`;
    await scheduleSignedOut(client, subject, new Date(Date.now() + 70_000));

    // «Scheduled» shows that it leaves anyway.
    await page.goto("/mail/scheduled");
    const row = page
      .getByRole("list", { name: "Scheduled" })
      .getByRole("listitem")
      .filter({ hasText: subject });
    await expect(
      row.getByRole("switch", { name: "Sent even if you sign out" })
    ).toBeChecked();

    // Sign out: nothing to warn about, every scheduled one is marked.
    await page.getByRole("button", { name: /^Account/u }).click();
    await page.getByRole("menuitem", { name: "Sign out" }).click();
    await expect(page).toHaveURL(/\/login/u);

    // Bob gets it while alice is signed out.
    const context = await browser.newContext();
    const bobPage = await context.newPage();
    await signIn(bobPage, "bob", DOVECOT.password);
    await expect(bobPage).toHaveURL(/\/mail\/inbox/u);
    const bob = await jmapFor(bobPage);
    await expect
      .poll(
        async () => {
          const ids = await emailsWithSubject(bob, subject);
          return ids.length;
        },
        { intervals: [5000], timeout: 240_000 }
      )
      .toBe(1);

    // Clean up both mailboxes.
    const atBob = await emailsWithSubject(bob, subject);
    await bob.request((b) => [
      b.call("Email/set", {
        accountId: bob.primaryAccountId(),
        destroy: atBob,
      }),
    ]);
    await context.close();
    const alicePage = await browser.newPage();
    await signIn(alicePage, "alice", DOVECOT.password);
    await expect(alicePage).toHaveURL(/\/mail\/inbox/u);
    const alice = await jmapFor(alicePage);
    const atAlice = await emailsWithSubject(alice, subject);
    await alice.request((b) => [
      b.call("Email/set", {
        accountId: alice.primaryAccountId(),
        destroy: atAlice,
      }),
    ]);
    await alicePage.close();
  }
);
