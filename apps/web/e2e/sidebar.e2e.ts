// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The collapsed sidebar is in the preferences cookie (lib/preferences.ts),
// so the server renders it as it was left: no expanded first paint that
// collapses after hydration.

import { readFileSync } from "node:fs";

import { expect, test } from "@playwright/test";

import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

/** Where older versions kept it (localStorage), as shell.tsx names it. */
const LEGACY_KEY =
  /const SIDEBAR_KEY = "(?<key>[^"]+)"/u.exec(
    readFileSync(
      new URL("../src/components/mail/shell.tsx", import.meta.url),
      "utf-8"
    )
  )?.groups?.key ?? "";

test("the server renders the sidebar as it was left", async ({
  context,
  page,
}, info) => {
  test.skip(
    info.project.name !== "desktop",
    "phones and tablets show the sidebar as a drawer"
  );
  await signIn(context);
  await installFakeJmap(page);
  await page.goto("/mail/inbox");
  // A click before the page has hydrated does nothing: click until it
  // collapses.
  await expect(async () => {
    await page.getByRole("button", { name: "Collapse sidebar" }).click();
    await expect(
      page.getByRole("button", { name: "Expand sidebar" })
    ).toBeVisible({ timeout: 1000 });
  }).toPass();

  const cookies = await context.cookies();
  const prefs = cookies.find((cookie) => cookie.name === "wm-prefs");
  expect(prefs?.value).toMatch(/\.collapsed(?:\.|$)/u);
  // The page as the server sends it, before any script runs.
  const collapsed = await page.request.get("/mail/inbox");
  const html = await collapsed.text();
  expect(html).toContain('aria-label="Expand sidebar"');
  expect(html).not.toContain('aria-label="Collapse sidebar"');

  await page.getByRole("button", { name: "Expand sidebar" }).click();
  const expanded = await page.request.get("/mail/inbox");
  expect(await expanded.text()).toContain('aria-label="Collapse sidebar"');
});

test("an older version's choice moves to the cookie", async ({
  context,
  page,
}, info) => {
  test.skip(
    info.project.name !== "desktop",
    "phones and tablets show the sidebar as a drawer"
  );
  await signIn(context);
  await installFakeJmap(page);
  expect(LEGACY_KEY).not.toBe("");
  await context.addInitScript((key) => {
    if (sessionStorage.getItem("migrated") === null) {
      sessionStorage.setItem("migrated", "1");
      localStorage.setItem(key, "1");
    }
  }, LEGACY_KEY);
  await page.goto("/mail/inbox");
  await expect(
    page.getByRole("button", { name: "Expand sidebar" })
  ).toBeVisible();
  expect(
    await page.evaluate((key) => localStorage.getItem(key), LEGACY_KEY)
  ).toBeNull();
  const reloaded = await page.request.get("/mail/inbox");
  expect(await reloaded.text()).toContain('aria-label="Expand sidebar"');
});
