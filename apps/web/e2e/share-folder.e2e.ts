// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Sharing one of our folders (mail:share): from its context menu in the
// sidebar and from ⌘K, also with the sidebar collapsed (the dialog lives
// in ShareFolderProvider, around both sidebars).

import { expect, test } from "@playwright/test";

import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test.skip(
  ({ isMobile }) => isMobile,
  "a right click and the collapse button are desktop things"
);

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  await installFakeJmap(page);
});

test("a folder's context menu shares it", async ({ page }) => {
  await page.goto("/mail/inbox");
  // A click before the page has hydrated does nothing.
  await page.waitForLoadState("networkidle");
  const folder = page
    .getByRole("navigation")
    .getByRole("link", { name: /^Projects/u });
  await folder.click({ button: "right" });
  await page.getByRole("menuitem", { name: "Share…" }).click();
  const dialog = page.getByRole("dialog", { name: "Share “Projects”" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Close" }).click();
  await expect(dialog).toHaveCount(0);
});

test("⌘K shares the open folder, also with the sidebar collapsed", async ({
  page,
}) => {
  await page.goto("/mail/projects");
  await page.waitForLoadState("networkidle");
  await page.getByRole("button", { name: "Collapse sidebar" }).click();
  await expect(
    page.getByRole("button", { name: "Expand sidebar" })
  ).toBeVisible();

  await page.keyboard.press("ControlOrMeta+k");
  const palette = page.getByRole("dialog", { name: "Command palette" });
  await expect(palette).toBeVisible();
  await palette.getByRole("option", { name: /Share “Projects”/u }).click();
  await expect(
    page.getByRole("dialog", { name: "Share “Projects”" })
  ).toBeVisible();
});
