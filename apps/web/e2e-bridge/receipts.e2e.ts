// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Delivery status and read receipts through the real bridge
// Dovecot for
// IMAP and the test Postfix for sending (`E2E_BRIDGE_SMTP=postfix`), so
// bounces (DSN) and receipts (MDN) really come back to the sender's
// mailbox. Skipped with Dovecot's submission, which relays everything to
// Stalwart (nothing would come back to Dovecot).
//
// The users are Dovecot's `alice` and `bob` (addresses `@nebulamail.test`),
// the same mailboxes as the other tests: Postfix delivers there itself.
// Nothing goes through Stalwart, so its send limit doesn't count.
// Six messages per run: four from alice, two receipts from bob.

import type { EmailId, JmapClient } from "@nebulamail/jmap-client";
import type { Browser, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import {
  destroyMatching,
  emailsWithSubject,
  reader,
  row,
  send,
  shot,
} from "../e2e-live/ui";
import { DOVECOT, SMTP } from "./env";
import { jmapFor, signIn } from "./session";

const ALICE = "alice";
const BOB = "bob";
/** Their addresses (the bridge's `email_domain`). */
const ALICE_ADDRESS = "alice@nebulamail.test";
const BOB_ADDRESS = "bob@nebulamail.test";

/** DSNs and receipts go through Postfix, LMTP and the bridge's sync. */
const ROUND_TRIP = 90_000;

test.skip(
  SMTP.name !== "postfix",
  "needs the test Postfix: E2E_BRIDGE_SMTP=postfix (docker/README.md)"
);
// The last test looks at what the others sent.
test.describe.configure({ mode: "serial", timeout: 240_000 });

/** In every subject of this run (receipts and bounces quote it). */
const RUN = `nebularcpt${Date.now().toString(36)}`;
const subject = (what: string): string => `Receipts ${RUN} ${what}`;
/**
 * Postfix accepts anything for this domain and then bounces it. Not
 * `nobody@`: Postfix's aliases send that one to root, which Dovecot takes.
 */
const BOUNCES = `nobody-${RUN}@bounce.nebulamail.test`;

const signedIn = async (
  browser: Browser,
  user: string
): Promise<{ page: Page; client: JmapClient }> => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await signIn(page, user, DOVECOT.password);
  await expect(page).toHaveURL(/\/mail\/inbox/u);
  return { client: await jmapFor(page), page };
};

const chooseReceipts = async (
  page: Page,
  choice: "Never" | "Ask" | "Always"
): Promise<void> => {
  await page.goto("/settings/general");
  const radio = page
    .getByRole("radiogroup", { name: "Read receipts" })
    .getByRole("radio", { name: choice });
  // A click before the page has hydrated does nothing.
  await expect(async () => {
    await radio.click();
    await expect(radio).toBeChecked({ timeout: 1000 });
  }).toPass();
};

/** Opens a received email from the inbox once it has arrived. */
const openReceived = async (page: Page, title: string): Promise<void> => {
  await page.goto("/mail/inbox");
  await expect(row(page, title)).toBeVisible({ timeout: ROUND_TRIP });
  await row(page, title).click();
  await expect(reader(page).getByText(title).first()).toBeVisible();
};

const sentRowState = (page: Page, title: string) =>
  row(page, title).locator("[data-delivery]");

/** The read receipts (MDN) about `title` in the account, by subject. */
const receiptsFor = async (
  client: JmapClient,
  title: string
): Promise<EmailId[]> => {
  const ids = await emailsWithSubject(client, title);
  if (ids.length === 0) {
    return [];
  }
  const [got] = await client.request((b) => [
    b.call("Email/get", {
      accountId: client.primaryAccountId(),
      ids,
      properties: ["subject", "header:Content-Type"],
    }),
  ]);
  return got.list
    .filter((email) => {
      const type = email["header:Content-Type"];
      return (
        email.subject === `Read: ${title}` &&
        typeof type === "string" &&
        type.includes("disposition-notification")
      );
    })
    .map((email) => email.id);
};

/** Waits until bob's receipt about `title` is in alice's mailbox. */
const expectReceipt = async (alice: JmapClient, title: string) => {
  await expect
    .poll(
      async () => {
        const ids = await receiptsFor(alice, title);
        return ids.length;
      },
      { timeout: ROUND_TRIP }
    )
    .toBe(1);
};

test.afterAll(async ({ browser }) => {
  // Everything this run sent or received, in both mailboxes.
  await Promise.all(
    [ALICE, BOB].map(async (user) => {
      const { client, page } = await signedIn(browser, user);
      await destroyMatching(client, RUN);
      await page.context().close();
    })
  );
});

test(
  "a bounce comes back as a DSN and Sent shows it with the reason",
  { tag: "@postfix" },
  async ({ browser }) => {
    const { page } = await signedIn(browser, ALICE);
    const title = subject("bounce");
    await send(page, {
      body: "To an address that bounces.",
      subject: title,
      to: BOUNCES,
    });

    await page.goto("/mail/sent");
    // The bridge reads Postfix's report when it reaches alice's INBOX.
    await expect(sentRowState(page, title)).toHaveAttribute(
      "data-delivery",
      "failed",
      { timeout: ROUND_TRIP }
    );
    await page.screenshot(shot("bridge-sent-bounce"));
    await row(page, title).click();
    const delivery = reader(page).getByRole("list", { name: "Delivery" });
    await expect(
      delivery.locator('[data-delivery="failed"]', { hasText: BOUNCES })
    ).toBeVisible();
    // Postfix's reason is "X-Postfix; unknown user", which the bridge
    // doesn't keep (only `smtp;` codes): its smtpReply is still Postfix's
    // "250 2.0.0 Ok: queued", which is not a reason and isn't shown.
    await expect(reader(page)).not.toContainText("250 2.0.0");
    await page.screenshot(shot("bridge-open-bounce"));
  }
);

const read = { always: subject("always"), send: subject("send") };

test(
  "bob sends the receipt alice asked for and it reaches her",
  { tag: "@postfix" },
  async ({ browser }) => {
    const alice = await signedIn(browser, ALICE);
    await send(alice.page, {
      body: "Please confirm you got this.",
      receipt: "request",
      subject: read.send,
      to: BOB_ADDRESS,
    });

    const bob = await signedIn(browser, BOB);
    await openReceived(bob.page, read.send);
    const banner = reader(bob.page).getByTestId("read-receipt");
    await expect(banner).toContainText(
      `${ALICE_ADDRESS} asks for a read receipt.`
    );
    // Dovecot's Return-Path is alice: no "other address" warning.
    await expect(banner).not.toContainText("isn’t the sender’s");
    await bob.page.screenshot(shot("bridge-banner"));
    await banner.getByRole("button", { name: "Send receipt" }).click();
    await expect(banner).toBeHidden();

    await expectReceipt(alice.client, read.send);
  }
);

test(
  "'never' hides the request, Ignore sends nothing, 'always' sends alone",
  { tag: "@postfix" },
  async ({ browser }) => {
    const alice = await signedIn(browser, ALICE);
    const ignored = subject("ignore");
    for (const title of [ignored, read.always]) {
      // oxlint-disable-next-line no-await-in-loop -- one composer at a time.
      await send(alice.page, {
        body: "Please confirm you got this.",
        receipt: "request",
        subject: title,
        to: BOB_ADDRESS,
      });
    }

    const bob = await signedIn(browser, BOB);
    await chooseReceipts(bob.page, "Never");
    await openReceived(bob.page, ignored);
    await expect(reader(bob.page).getByTestId("read-receipt")).toHaveCount(0);

    await chooseReceipts(bob.page, "Ask");
    await openReceived(bob.page, ignored);
    const banner = reader(bob.page).getByTestId("read-receipt");
    await banner.getByRole("button", { name: "Ignore" }).click();
    await expect(banner).toBeHidden();
    // Only `$mdnsent`, so no client asks again.
    const [ignoredId] = await emailsWithSubject(bob.client, ignored);
    expect(ignoredId).toBeDefined();
    const [kept] = await bob.client.request((b) => [
      b.call("Email/get", {
        accountId: bob.client.primaryAccountId(),
        ids: ignoredId === undefined ? [] : [ignoredId],
        properties: ["keywords"],
      }),
    ]);
    expect(kept.list[0]?.keywords).toHaveProperty("$mdnsent", true);

    // "always": the Return-Path is the receipt address, so it goes alone.
    await chooseReceipts(bob.page, "Always");
    await openReceived(bob.page, read.always);
    await expect(reader(bob.page).getByTestId("read-receipt")).toHaveCount(0);

    await expectReceipt(alice.client, read.always);
    // The ignored one sent nothing (the automatic one, sent after it, is
    // already there).
    expect(await receiptsFor(alice.client, ignored)).toHaveLength(0);
  }
);

test(
  "Sent shows «read» once the receipts are matched",
  { tag: "@postfix" },
  async ({ browser }) => {
    const { page } = await signedIn(browser, ALICE);
    await page.goto("/mail/sent");
    for (const title of [read.send, read.always]) {
      // oxlint-disable-next-line no-await-in-loop -- the same page, in order.
      await expect(sentRowState(page, title)).toHaveAttribute(
        "data-delivery",
        "read",
        // The receipts are already in the mailbox: only the sync is left.
        { timeout: 30_000 }
      );
    }
    await page.screenshot(shot("bridge-sent-read"));
  }
);
