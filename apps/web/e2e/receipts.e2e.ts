// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Delivery status of sent mail and read receipts, against
// the fake JMAP server: the Sent list and the open email show delivered,
// bounced (with the server's reply), delayed and read; a received email
// that asks for a receipt shows Send / Ignore, both set `$mdnsent`; the
// "never / ask / always" setting; and the composer's option.

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import type { FakeEmail, FakeJmap, FakeSubmission } from "./fake-jmap";
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
  jmap.mdn = true;
});

const JANE = { email: "jane@example.com", name: "Jane" };
const AT = "2026-10-24T10:00:00Z";

const sent = (id: string, subject: string): FakeEmail => ({
  from: JANE,
  html: `<p>${subject}</p>`,
  id,
  keywords: { $seen: true },
  mailboxIds: { sent: true },
  receivedAt: AT,
  subject,
});

const submission = (
  emailId: string,
  status: FakeSubmission["deliveryStatus"]
): FakeSubmission => ({
  deliveryStatus: status,
  emailId,
  id: `sub-${emailId}`,
  sendAt: AT,
  undoStatus: "final",
});

const one = (delivered: string, displayed = "unknown", smtpReply = "") => ({
  delivered,
  displayed,
  smtpReply,
});

/** An inbox email whose sender asks for a receipt. */
const asking = (
  id: string,
  subject: string,
  patch: Partial<FakeEmail> = {}
): FakeEmail => ({
  from: { email: "carla@example.com", name: "Carla Gómez" },
  html: `<p>${subject}</p>`,
  id,
  keywords: {},
  mailboxIds: { inbox: true },
  readReceiptTo: "carla@example.com",
  receivedAt: AT,
  returnPath: "<carla@example.com>",
  subject,
  ...patch,
});

const Disposition = z.object({
  send: z.record(
    z.string(),
    z.object({
      disposition: z.object({ sendingMode: z.string(), type: z.string() }),
      forEmailId: z.string(),
    })
  ),
});

const mdnOf = (index: number) => {
  const parsed = Disposition.parse(jmap.mdnSends[index]);
  const [first] = Object.values(parsed.send);
  return first;
};

const reader = (page: Page) => page.getByRole("region", { name: "Message" });

const row = (page: Page, subject: string) =>
  page.getByRole("listitem").filter({ hasText: subject });

const chooseReceipts = async (
  page: Page,
  choice: "Never" | "Ask" | "Always"
) => {
  await page.goto("/settings/general");
  const group = page.getByRole("radiogroup", { name: "Read receipts" });
  await expect(group).toBeVisible();
  await group.getByRole("radio", { name: choice }).click();
  await expect(group.getByRole("radio", { name: choice })).toBeChecked();
};

test("sent mail shows delivered, bounced, read and delayed, and follows push", async ({
  page,
}) => {
  jmap.emails.push(
    sent("s-ok", "Delivered one"),
    sent("s-bad", "Bounced one"),
    sent("s-read", "Read one"),
    sent("s-wait", "Waiting one")
  );
  const waiting = submission("s-wait", {
    "bob@example.com": one("queued", "unknown", "451 4.7.1 try again later"),
  });
  jmap.submissions.push(
    submission("s-ok", { "bob@example.com": one("yes", "unknown", "250 OK") }),
    submission("s-bad", {
      "bob@example.com": one("yes"),
      "nobody@example.com": one("no", "unknown", "550 5.1.1 user unknown"),
    }),
    submission("s-read", { "bob@example.com": one("yes", "yes") }),
    waiting
  );

  await page.goto("/mail/sent");
  await expect(
    row(page, "Delivered one").locator('[data-delivery="delivered"]')
  ).toBeVisible();
  await expect(
    row(page, "Bounced one").locator('[data-delivery="failed"]')
  ).toBeVisible();
  await expect(
    row(page, "Read one").locator('[data-delivery="read"]')
  ).toBeVisible();
  await expect(
    row(page, "Waiting one").locator('[data-delivery="queued"]')
  ).toBeVisible();
  // Received mail never shows it.
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  await expect(page.locator("[data-delivery]")).toHaveCount(0);

  // The bounce, per recipient, with the server's reason written out.
  await page.goto("/mail/sent/s-bad");
  const delivery = reader(page).getByRole("list", { name: "Delivery" });
  await expect(
    delivery.locator('[data-delivery="delivered"]', {
      hasText: "bob@example.com",
    })
  ).toBeVisible();
  await expect(
    delivery.locator('[data-delivery="failed"]', {
      hasText: "nobody@example.com",
    })
  ).toBeVisible();
  await expect(reader(page).getByText("550 5.1.1 user unknown")).toBeVisible();

  // Delivered later: push says EmailSubmission changed.
  await page.goto("/mail/sent");
  await expect(
    row(page, "Waiting one").locator('[data-delivery="queued"]')
  ).toBeVisible();
  waiting.deliveryStatus = { "bob@example.com": one("yes", "unknown") };
  jmap.pushChanges.push("EmailSubmission");
  await expect(
    row(page, "Waiting one").locator('[data-delivery="delivered"]')
  ).toBeVisible({ timeout: 10_000 });
});

test("without submission data nothing is shown", async ({ page }) => {
  jmap.emails.push(sent("s-none", "No status"));
  await page.goto("/mail/sent/s-none");
  await expect(reader(page).getByText("No status").first()).toBeVisible();
  await expect(page.locator("[data-delivery]")).toHaveCount(0);
});

test("a read receipt request: Send sends one MDN and sets $mdnsent", async ({
  page,
}) => {
  jmap.emails.push(asking("r1", "Please confirm"));
  await page.goto("/mail/inbox/r1");
  const banner = reader(page).getByTestId("read-receipt");
  await expect(banner).toContainText(
    "carla@example.com asks for a read receipt."
  );
  // Nothing goes out before the user says so.
  expect(jmap.mdnSends).toHaveLength(0);
  await banner.getByRole("button", { name: "Send receipt" }).click();
  await expect(banner).toBeHidden();
  expect(jmap.mdnSends).toHaveLength(1);
  expect(jmap.mdnSends[0]).toMatchObject({
    identityId: "i1",
    onSuccessUpdateEmail: { "#receipt": { "keywords/$mdnsent": true } },
    send: {
      receipt: {
        disposition: {
          actionMode: "manual-action",
          sendingMode: "mdn-sent-manually",
          type: "displayed",
        },
        forEmailId: "r1",
        includeOriginalMessage: false,
        subject: "Read: Please confirm",
        textBody: expect.stringContaining("Please confirm"),
      },
    },
  });
  expect(
    jmap.emails.find((email) => email.id === "r1")?.keywords
  ).toHaveProperty("$mdnsent");
  // Not asked again.
  await page.reload();
  await expect(reader(page).getByText("Please confirm").first()).toBeVisible();
  await expect(reader(page).getByTestId("read-receipt")).toHaveCount(0);
});

test("Ignore sends nothing and stops asking", async ({ page }) => {
  jmap.emails.push(asking("r2", "Did you see this?"));
  await page.goto("/mail/inbox/r2");
  const banner = reader(page).getByTestId("read-receipt");
  await banner.getByRole("button", { name: "Ignore" }).click();
  await expect(banner).toBeHidden();
  expect(jmap.mdnSends).toHaveLength(0);
  expect(jmap.updates).toContainEqual({ r2: { "keywords/$mdnsent": true } });
  await page.reload();
  await expect(
    reader(page).getByText("Did you see this?").first()
  ).toBeVisible();
  await expect(reader(page).getByTestId("read-receipt")).toHaveCount(0);
});

test("a receipt for another address says so", async ({ page }) => {
  jmap.emails.push(
    asking("r3", "Elsewhere", {
      readReceiptTo: "tracker@other.example",
      returnPath: "<tracker@other.example>",
    })
  );
  await page.goto("/mail/inbox/r3");
  const banner = reader(page).getByTestId("read-receipt");
  await expect(banner).toContainText("tracker@other.example");
  await expect(banner).toContainText("isn’t the sender’s");
});

test("'never' shows nothing and sends nothing", async ({ page }) => {
  jmap.emails.push(asking("r4", "Never ask me"));
  await chooseReceipts(page, "Never");
  await page.goto("/mail/inbox/r4");
  await expect(reader(page).getByText("Never ask me").first()).toBeVisible();
  await expect(reader(page).getByTestId("read-receipt")).toHaveCount(0);
  expect(jmap.mdnSends).toHaveLength(0);
});

test("'always' sends once on its own, but asks when it isn't safe", async ({
  page,
}) => {
  jmap.emails.push(
    asking("r5", "Auto receipt"),
    asking("r6", "Different return path", {
      returnPath: "<bounces@lists.example>",
    }),
    asking("r7", "Spammy receipt", { mailboxIds: { junk: true } })
  );
  await chooseReceipts(page, "Always");

  await page.goto("/mail/inbox/r5");
  await expect(reader(page).getByText("Auto receipt").first()).toBeVisible();
  await expect.poll(() => jmap.mdnSends.length).toBe(1);
  expect(mdnOf(0)?.disposition.sendingMode).toBe("mdn-sent-automatically");
  await expect(reader(page).getByTestId("read-receipt")).toHaveCount(0);
  // Opening it again sends nothing more.
  await page.reload();
  await expect(reader(page).getByText("Auto receipt").first()).toBeVisible();
  // Shown with `$mdnsent` from the server: nothing to send any more.
  expect(jmap.mdnSends).toHaveLength(1);

  // Return-Path isn't the receipt address (RFC 8098 §2.1): ask.
  await page.goto("/mail/inbox/r6");
  await expect(reader(page).getByTestId("read-receipt")).toBeVisible();
  // Junk: never automatically.
  await page.goto("/mail/junk/r7");
  await expect(reader(page).getByTestId("read-receipt")).toBeVisible();
  expect(jmap.mdnSends).toHaveLength(1);
});

test("without MDN on the server there is no banner and no setting", async ({
  page,
}) => {
  jmap.mdn = false;
  jmap.emails.push(asking("r8", "No MDN here"));
  await page.goto("/mail/inbox/r8");
  await expect(reader(page).getByText("No MDN here").first()).toBeVisible();
  await expect(reader(page).getByTestId("read-receipt")).toHaveCount(0);
  await page.goto("/settings/general");
  await expect(page.getByText("Sync between devices")).toBeVisible();
  await expect(
    page.getByRole("radiogroup", { name: "Read receipts" })
  ).toHaveCount(0);
});

const DraftHeader = z.object({
  "header:Disposition-Notification-To:asAddresses": z
    .array(z.object({ email: z.string(), name: z.string().nullable() }))
    .optional(),
});

test("the composer asks for a read receipt only when chosen", async ({
  page,
}) => {
  await page.goto("/mail/inbox");
  await page.waitForLoadState("networkidle");
  await page
    .getByRole("button", { name: /^(?:New message|Compose)/u })
    .filter({ visible: true })
    .first()
    .click();
  const composer = page.locator("section[data-composer]");
  await expect(composer).toBeVisible();
  await composer
    .getByRole("textbox", { name: "Subject" })
    .fill("Receipt please");

  // Off by default.
  await composer.getByRole("button", { name: "More send options" }).click();
  const option = page.getByRole("menuitemcheckbox", {
    name: "Request read receipt",
  });
  await expect(option).not.toBeChecked();
  await option.click();
  await page.keyboard.press("Escape");
  await composer
    .getByRole("textbox", { name: "Message" })
    .press("ControlOrMeta+s");
  await expect.poll(() => jmap.created.length).toBeGreaterThan(0);
  const saved = DraftHeader.parse(jmap.created.at(-1));
  expect(saved["header:Disposition-Notification-To:asAddresses"]).toStrictEqual(
    [{ email: "jane@example.com", name: "Jane" }]
  );
});
