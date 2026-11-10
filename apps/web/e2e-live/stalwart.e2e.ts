// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Delivery status and read receipts against a real Stalwart
// the app
// signed in through Stalwart's own OAuth page, as alice and bob of `bun
// run dev:setup`. What Stalwart does, seen on 1.0.1:
//
// - `deliveryStatus` follows its queue (support.stalw.art/t/2004): a local
//   recipient that doesn't exist is `no` with «550 5.1.1 Mailbox does not
//   exist.» at once, an outside domain with a null MX `no` with «Domain
//   does not accept messages (null MX)», and bob `yes` once delivered.
// - No `urn:ietf:params:jmap:mdn` (MDN/send and MDN/parse are
//   `unknownMethod`), and a receipt that arrives doesn't change
//   `displayed` (`mdnBlobIds` stays empty).
//
// So the app shows the bounces with why, "delivered" for bob, and no read
// receipt options. Skipped unless these are set (passwords in
// docker/.env); CI runs it (.github/workflows/ci.yml):
//
//   MAIL_LIVE_URL=http://localhost:8080 \
//   MAIL_LIVE_ALICE_PASSWORD=… MAIL_LIVE_BOB_PASSWORD=… \
//   bun run test:live
//
// Stalwart allows about 25 sends an hour from alice to bob: a run sends
// four messages, only one of them from alice to bob (and one from bob to
// alice). Everything it sends or receives is deleted at the end.

import type {
  EmailId,
  EmailSubmission,
  FetchLike,
  JmapClient,
  MailboxId,
} from "@nebulamail/jmap-client";
import { JmapClient as Client } from "@nebulamail/jmap-client";
import type { Browser, Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { allowIfAsked, destroyMatching, reader, row, send, shot } from "./ui";

const env = {
  alicePassword: process.env.MAIL_LIVE_ALICE_PASSWORD ?? "",
  bobPassword: process.env.MAIL_LIVE_BOB_PASSWORD ?? "",
  url: process.env.MAIL_LIVE_URL ?? "",
};
const configured = Object.values(env).every((value) => value !== "");

const ALICE = "alice@nebulamail.test";
const BOB = "bob@nebulamail.test";

test.skip(!configured, "set MAIL_LIVE_* (see the comment at the top)");
test.describe.configure({ timeout: 240_000 });

/** In every subject of this run (bounces and receipts quote it). */
const RUN = `nebulalive${Date.now().toString(36)}`;
const subject = (what: string): string => `Live ${RUN} ${what}`;

/** Stalwart's queue, its DSNs and the search index take a moment. */
const ROUND_TRIP = 120_000;

/** Stalwart takes basic auth too; swap the client's Bearer header for it. */
const basicFetch =
  (user: string, password: string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("Authorization", `Basic ${btoa(`${user}:${password}`)}`);
    return await fetch(input, { ...init, headers });
  };

/** JMAP straight to Stalwart, for what the test sets up and checks. */
const connect = async (user: string, password: string): Promise<JmapClient> =>
  await Client.connect({
    fetch: basicFetch(user, password),
    getAccessToken: async () => await Promise.resolve("unused"),
    sessionUrl: `${env.url}/.well-known/jmap`,
  });

/** Signs in on Stalwart's own page, in a fresh browser context. */
const signedIn = async (
  browser: Browser,
  user: string,
  password: string
): Promise<Page> => {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto("/login");
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();
  await page.getByRole("textbox", { name: "Username" }).fill(user);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { exact: true, name: "Sign in" }).click();
  await allowIfAsked(page, /\/mail\/inbox/u);
  return page;
};

const mailbox = async (
  client: JmapClient,
  role: string
): Promise<MailboxId> => {
  const [boxes] = await client.request((b) => [
    b.call("Mailbox/get", {
      accountId: client.primaryAccountId(),
      properties: ["role"],
    }),
  ]);
  const found = boxes.list.find((box) => box.role === role);
  if (found === undefined) {
    throw new Error(`No ${role} folder`);
  }
  return found.id;
};

/**
 * Sends a ready-made message as another mail client would: imported
 * (into Sent, so the app lists it there) and submitted.
 */
const sendRaw = async (
  client: JmapClient,
  lines: readonly string[]
): Promise<{ emailId: EmailId; submissionId: string }> => {
  const accountId = client.primaryAccountId();
  const raw = new TextEncoder().encode(`${lines.join("\r\n")}\r\n`);
  const { blobId } = await client.upload(accountId, raw, {
    type: "message/rfc822",
  });
  const sent = await mailbox(client, "sent");
  const [identities, imported] = await client.request((b) => [
    b.call("Identity/get", { accountId, properties: ["email"] }),
    b.call("Email/import", {
      accountId,
      emails: {
        raw: {
          blobId,
          keywords: { $seen: true },
          mailboxIds: { [sent]: true },
        },
      },
    }),
  ]);
  const [identity] = identities.list;
  const email = imported.created?.raw;
  if (identity === undefined || email === undefined || email === null) {
    throw new Error("Could not import the message");
  }
  const [submitted] = await client.request((b) => [
    b.call("EmailSubmission/set", {
      accountId,
      create: { s: { emailId: email.id, identityId: identity.id } },
    }),
  ]);
  const submission = submitted.created?.s;
  if (submission === undefined || submission === null) {
    throw new Error(
      `Not sent: ${JSON.stringify(submitted.notCreated?.s ?? null)}`
    );
  }
  return { emailId: email.id, submissionId: submission.id };
};

type Status = NonNullable<EmailSubmission["deliveryStatus"]>;

/** The `deliveryStatus` of the submission(s) of an email. */
const statusOf = async (client: JmapClient, emailId: EmailId) => {
  const [, got] = await client.request((b) => {
    const query = b.call("EmailSubmission/query", {
      accountId: client.primaryAccountId(),
      filter: { emailIds: [emailId] },
    });
    return [
      query,
      b.call("EmailSubmission/get", {
        "#ids": query.ref("/ids"),
        accountId: client.primaryAccountId(),
        properties: ["deliveryStatus", "dsnBlobIds", "mdnBlobIds"],
      }),
    ];
  });
  const [submission] = got.list;
  if (submission === undefined) {
    throw new Error(`No submission for ${emailId}`);
  }
  return submission;
};

/** The ids of emails in a folder whose text has `marker`. */
const findIn = async (
  client: JmapClient,
  role: string,
  marker: string
): Promise<EmailId[]> => {
  const inMailbox = await mailbox(client, role);
  const [query] = await client.request((b) => [
    b.call("Email/query", {
      accountId: client.primaryAccountId(),
      filter: { inMailbox, text: marker },
    }),
  ]);
  return [...query.ids];
};

const countIn = async (
  client: JmapClient,
  role: string,
  marker: string
): Promise<number> => {
  const ids = await findIn(client, role, marker);
  return ids.length;
};

/** The email the app just sent, by its subject. */
const sentEmail = async (client: JmapClient, title: string) => {
  let id: EmailId | undefined;
  await expect
    .poll(
      async () => {
        [id] = await findIn(client, "sent", title);
        return id;
      },
      { timeout: ROUND_TRIP }
    )
    .toBeDefined();
  if (id === undefined) {
    throw new Error("unreachable");
  }
  return id;
};

/** Bob's, once delivered (1.0 spells the reply out). */
const delivered: Status[string] = {
  delivered: "yes",
  displayed: "unknown",
  smtpReply: "Code: 250, Enhanced code: 2.1.5, Message: OK",
};

let alice: JmapClient;
let bob: JmapClient;

test.beforeAll(async () => {
  alice = await connect(ALICE, env.alicePassword);
  bob = await connect(BOB, env.bobPassword);
});

test.afterAll(async () => {
  await destroyMatching(alice, RUN);
  await destroyMatching(bob, RUN);
});

test("a local address that doesn't exist: Sent shows the bounce and why", async ({
  browser,
}) => {
  const page = await signedIn(browser, ALICE, env.alicePassword);
  const title = subject("local bounce");
  const nobody = `nobody-${RUN}@nebulamail.test`;
  // No MDN on Stalwart: the composer doesn't offer a receipt.
  await send(page, {
    body: "To an address that doesn't exist.",
    receipt: "unavailable",
    subject: title,
    to: nobody,
  });

  // Stalwart knows at once (local recipient).
  const emailId = await sentEmail(alice, title);
  const first = await statusOf(alice, emailId);
  expect(first.deliveryStatus).toStrictEqual({
    [nobody]: {
      delivered: "no",
      displayed: "unknown",
      smtpReply: "550 5.1.1 Mailbox does not exist.",
    },
  });

  await page.goto("/mail/sent");
  await expect(row(page, title).locator("[data-delivery]")).toHaveAttribute(
    "data-delivery",
    "failed"
  );
  await page.screenshot(shot("stalwart-sent-bounce"));
  await row(page, title).click();
  await expect(
    reader(page)
      .getByRole("list", { name: "Delivery" })
      .locator('[data-delivery="failed"]', { hasText: nobody })
  ).toBeVisible();
  await expect(
    reader(page).getByText("550 5.1.1 Mailbox does not exist.")
  ).toBeVisible();
  await page.screenshot(shot("stalwart-open-bounce"));
});

test("an outside address that bounces: Sent shows it and why", async ({
  browser,
}) => {
  const page = await signedIn(browser, ALICE, env.alicePassword);
  const title = subject("outside bounce");
  // example.com has a null MX (RFC 7505): Stalwart bounces it at once.
  const outside = `nobody-${RUN}@example.com`;
  await send(page, {
    body: "To a domain that takes no mail.",
    subject: title,
    to: outside,
  });
  const emailId = await sentEmail(alice, title);

  // The submission says so.
  const reason = "Domain does not accept messages (null MX)";
  await expect
    .poll(
      async () => {
        const after = await statusOf(alice, emailId);
        return after.deliveryStatus;
      },
      { timeout: ROUND_TRIP }
    )
    .toStrictEqual({
      [outside]: { delivered: "no", displayed: "unknown", smtpReply: reason },
    });

  await page.goto("/mail/sent");
  await expect(row(page, title).locator("[data-delivery]")).toHaveAttribute(
    "data-delivery",
    "failed"
  );
  await row(page, title).click();
  await expect(reader(page).getByText(reason)).toBeVisible();
});

test("to bob: delivered, no read receipt request, a receipt changes nothing", async ({
  browser,
}) => {
  const title = subject("receipt");
  // Another mail client asks bob for a read receipt.
  const { emailId } = await sendRaw(alice, [
    `From: Alice <${ALICE}>`,
    `To: Bob <${BOB}>`,
    `Subject: ${title}`,
    `Date: ${new Date().toUTCString()}`,
    `Message-ID: <${RUN}.request@nebulamail.test>`,
    `Disposition-Notification-To: <${ALICE}>`,
    "MIME-Version: 1.0",
    "Content-Type: text/plain; charset=utf-8",
    "",
    "Please confirm you got this.",
  ]);

  // Delivered to bob, and Stalwart says so.
  await expect
    .poll(async () => await countIn(bob, "inbox", title), {
      timeout: ROUND_TRIP,
    })
    .toBe(1);
  await expect
    .poll(
      async () => {
        const status = await statusOf(alice, emailId);
        return status.deliveryStatus;
      },
      { timeout: ROUND_TRIP }
    )
    .toStrictEqual({ [BOB]: delivered });

  // Bob: no banner (no MDN on the server) and no setting for it.
  const bobPage = await signedIn(browser, BOB, env.bobPassword);
  await expect(row(bobPage, title)).toBeVisible({ timeout: ROUND_TRIP });
  await row(bobPage, title).click();
  await expect(reader(bobPage).getByText(title).first()).toBeVisible();
  await expect(reader(bobPage).getByTestId("read-receipt")).toHaveCount(0);
  await bobPage.screenshot(shot("stalwart-no-banner"));
  await bobPage.goto("/settings/general");
  await expect(bobPage.getByText("Sync between devices")).toBeVisible();
  await expect(
    bobPage.getByRole("radiogroup", { name: "Read receipts" })
  ).toHaveCount(0);

  // A receipt from another client of bob's reaches alice…
  await sendRaw(bob, [
    `From: Bob <${BOB}>`,
    `To: Alice <${ALICE}>`,
    `Subject: Read: ${title}`,
    `Date: ${new Date().toUTCString()}`,
    `Message-ID: <${RUN}.receipt@nebulamail.test>`,
    "MIME-Version: 1.0",
    'Content-Type: multipart/report; report-type=disposition-notification; boundary="r"',
    "",
    "--r",
    "Content-Type: text/plain; charset=utf-8",
    "",
    "Displayed.",
    "--r",
    "Content-Type: message/disposition-notification",
    "",
    "Reporting-UA: live-test",
    `Final-Recipient: rfc822;${BOB}`,
    `Original-Message-ID: <${RUN}.request@nebulamail.test>`,
    "Disposition: manual-action/MDN-sent-manually; displayed",
    "--r--",
  ]);
  await expect
    .poll(async () => await countIn(alice, "inbox", title), {
      timeout: ROUND_TRIP,
    })
    .toBe(1);
  // …and Stalwart doesn't record it.
  const after = await statusOf(alice, emailId);
  expect(after.deliveryStatus).toStrictEqual({ [BOB]: delivered });
  expect(after.mdnBlobIds).toStrictEqual([]);

  // Alice's Sent: "delivered" (not "read").
  const page = await signedIn(browser, ALICE, env.alicePassword);
  await page.goto("/mail/sent");
  await expect(row(page, title).locator("[data-delivery]")).toHaveAttribute(
    "data-delivery",
    "delivered"
  );
  await page.screenshot(shot("stalwart-sent-delivered"));
});
