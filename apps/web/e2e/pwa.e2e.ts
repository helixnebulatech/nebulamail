// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The installable app: manifest and icons, a service worker
// that works under the strict CSP, caches only this build's static files
// and shows the offline page without network.

import { expect, test } from "@playwright/test";
import { z } from "zod";

import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName !== "chromium" ||
      !["desktop", "phone-412"].includes(info.project.name),
    "one desktop and one phone run are enough"
  );
});

const Manifest = z.object({
  display: z.string(),
  icons: z.array(
    z.object({
      purpose: z.string().optional(),
      sizes: z.string(),
      src: z.string(),
    })
  ),
  name: z.string(),
  start_url: z.string(),
});

/** Files the worker may ever cache: build assets and the offline page. */
const ALLOWED =
  /^\/(?:assets\/[\w.-]+|offline\.html|offline\.css|favicon\.svg)$/u;

test("manifest and icons", async ({ page, request }) => {
  await page.goto("/login");
  const href = await page.locator('link[rel="manifest"]').getAttribute("href");
  expect(href).toBe("/manifest.webmanifest");
  const response = await request.get(href ?? "");
  const manifest = Manifest.parse(await response.json());
  expect(manifest.name).toBe("Nebula Mail");
  expect(manifest.display).toBe("standalone");
  expect(manifest.start_url).toBe("/mail/inbox");
  const sizes = manifest.icons.map(
    (icon) => `${icon.sizes}:${icon.purpose ?? "any"}`
  );
  expect(sizes).toEqual(
    expect.arrayContaining(["192x192:any", "512x512:any", "512x512:maskable"])
  );
  const icons = await Promise.all(
    manifest.icons.map(async (icon) => {
      const answer = await request.get(icon.src);
      return `${icon.src}: ${answer.status()}`;
    })
  );
  expect(icons).toEqual(manifest.icons.map((icon) => `${icon.src}: 200`));
  await expect(
    page.locator('meta[name="theme-color"]').first()
  ).toHaveAttribute("content", /^#[0-9A-F]{6}$/u);
});

test("service worker: static files only, offline page", async ({
  context,
  page,
}) => {
  const violations: string[] = [];
  page.on("console", (message) => {
    if (/Content Security Policy/u.test(message.text())) {
      violations.push(message.text());
    }
  });
  await signIn(context);
  await installFakeJmap(page);
  await page.goto("/mail/inbox");
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  const scope = await page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    return registration.scope;
  });
  expect(scope).toBe(`${new URL(page.url()).origin}/`);

  // Controlled after a reload; then read some mail so the app has made
  // JMAP calls, server function calls and page loads under the worker.
  await page.reload();
  await expect(page.getByText("Weekly digest").first()).toBeVisible();
  expect(
    await page.evaluate(() => navigator.serviceWorker.controller !== null)
  ).toBe(true);
  await page.getByText("Weekly digest").first().click();
  await expect(
    page.getByRole("heading", { name: "Weekly digest" })
  ).toBeVisible();

  const cached = await page.evaluate(async () => {
    const names = await caches.keys();
    const lists = await Promise.all(
      names.map(async (name) => {
        const cache = await caches.open(name);
        const entries = await cache.keys();
        return entries.map((entry) => entry.url);
      })
    );
    return lists.flat();
  });
  expect(cached.length).toBeGreaterThan(3);
  for (const url of cached) {
    const parsed = new URL(url);
    expect(parsed.origin, url).toBe(new URL(page.url()).origin);
    expect(parsed.pathname, url).toMatch(ALLOWED);
    expect(parsed.search, url).toBe("");
  }

  // No network: pages get the static offline page (no user data).
  await context.setOffline(true);
  await page.goto("/mail/inbox").catch(() => null);
  await expect(
    page.getByRole("heading", { name: "You're offline" })
  ).toBeVisible();
  await expect(page.getByText("jane@example.com")).toHaveCount(0);
  await context.setOffline(false);

  expect(violations).toEqual([]);
});
