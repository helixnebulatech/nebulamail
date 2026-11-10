// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Folders shared with us: only what the folder's rights
// (`myRights`) allow is offered, and a forward from another account copies
// its files and inline images into ours first (Blob/copy, RFC 8620 §6.3).

import type { Locator } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { FakeJmap } from "./fake-jmap";
import { SHARED_ACCOUNT, installFakeJmap } from "./fake-jmap";
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

const expectNoButtons = async (
  scope: Locator,
  names: readonly string[]
): Promise<void> => {
  await Promise.all(
    names.map(async (name) => {
      await expect(scope.getByRole("button", { name })).toHaveCount(0);
    })
  );
};

test("a read-only shared folder offers nothing that changes it", async ({
  page,
}) => {
  await page.goto(`/mail/${SHARED_ACCOUNT}.reports/s2`);
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Monthly report from Bob" })
  ).toBeVisible();
  const toolbar = page.getByRole("toolbar", { name: "Message actions" });
  await expect(toolbar).toBeVisible();
  await expectNoButtons(toolbar, [
    "Archive",
    "Delete",
    "Report spam",
    "Move to…",
    "Mark as unread",
    "Mark as read",
    "Star",
  ]);
  // Reading still works, and replying (it is our own message).
  await expect(
    reader.getByRole("button", { exact: true, name: "Forward" })
  ).toBeVisible();
  // The shortcuts do nothing, and it isn't marked as read.
  await page.locator("body").press("e");
  await page.locator("body").press("s");
  await page.waitForTimeout(1000);
  await expect(
    reader.getByRole("heading", { name: "Monthly report from Bob" })
  ).toBeVisible();
  expect(
    jmap.updates.filter((update) => Object.keys(update).length > 0)
  ).toEqual([]);
});

test("a writable shared folder only moves where it may", async ({ page }) => {
  await page.goto(`/mail/${SHARED_ACCOUNT}.team/e1`);
  const toolbar = page.getByRole("toolbar", { name: "Message actions" });
  await expect(toolbar).toBeVisible();
  // Bob's account has no Archive, Trash or Junk, and Reports is read-only.
  await expectNoButtons(toolbar, [
    "Archive",
    "Delete",
    "Report spam",
    "Move to…",
  ]);
  if ((page.viewportSize()?.width ?? 0) > 1000) {
    await expect(toolbar.getByRole("button", { name: "Star" })).toBeVisible();
  }
});

test("forwarding from another account copies its files into ours", async ({
  page,
}) => {
  await page.goto(`/mail/${SHARED_ACCOUNT}.reports/s2`);
  const reader = page.getByRole("region", { name: "Message" });
  await expect(
    reader.getByRole("heading", { name: "Monthly report from Bob" })
  ).toBeVisible();
  await page.waitForLoadState("networkidle");
  await reader.getByRole("button", { exact: true, name: "Forward" }).click();
  const composer = page.locator("section[data-composer]");
  await expect(composer.getByText("budget.pdf")).toBeVisible();
  expect(jmap.blobCopies).toEqual([
    {
      accountId: "a",
      blobIds: ["b-s2-att", "b-s2-img"],
      fromAccountId: SHARED_ACCOUNT,
    },
  ]);
  // The inline image shows from our copy.
  const image = composer
    .getByRole("textbox", { name: "Message" })
    .locator(".nm-inline-image img");
  await expect(image).toHaveAttribute("src", /^data:image\/png;base64,/u);

  await composer.getByRole("combobox", { name: "To" }).fill("eve@example.com");
  await composer.getByRole("combobox", { name: "To" }).press("Enter");
  await expect.poll(() => jmap.created.length, { timeout: 10_000 }).toBe(1);
  const draft = JSON.stringify(jmap.created[0]);
  // Only blobs of our account: the copies, never Bob's originals.
  expect(draft).toContain('"copy-b-s2-att"');
  expect(draft).toContain('"copy-b-s2-img"');
  expect(draft).not.toMatch(/"b-s2-(?:att|img)"/u);
});
