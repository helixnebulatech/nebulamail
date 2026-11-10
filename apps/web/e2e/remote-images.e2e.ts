// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Remote images in the build with our server: "never /
// ask / trusted senders / always" and "through the proxy / from the
// sender", within what the admin allows. The test server has no ticket
// keys (no proxy) and no REMOTE_IMAGES_* variables; other admin settings
// are given to the browser by rewriting the grant (/auth/token), the
// way the server sends them (server/images/policy.ts has their unit
// tests). The proxy is answered here: tickets from the BFF, then its
// `/batch` in batch mode (no storage), so the images are blob: URLs.

import type { Page, Request, Route } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { AdminImagePolicy } from "../src/mail/image-policy";
import { DEFAULT_ADMIN_IMAGE_POLICY } from "../src/mail/image-policy";
import type { FakeJmap } from "./fake-jmap";
import { PIXEL_PNG, installFakeJmap, syncedPreferences } from "./fake-jmap";
import { signIn } from "./session";

/** The email with a tracker pixel and one real remote image. */
const EMAIL = "/mail/inbox/e15";
const BANNER = "https://cdn.example/banner.png";
const PROXY = "http://img.test";

let jmap: FakeJmap;

/** Every request that could fetch a remote image, by any route. */
interface Watch {
  readonly sender: string[];
  readonly tracker: string[];
  readonly proxy: string[];
  readonly tickets: string[];
}

const watch = (page: Page): Watch => {
  const seen: Watch = { proxy: [], sender: [], tickets: [], tracker: [] };
  page.on("request", (request: Request) => {
    const url = request.url();
    if (url.startsWith("https://cdn.example/")) {
      seen.sender.push(url);
    } else if (url.startsWith("https://tracker.example/")) {
      seen.tracker.push(url);
    } else if (url.startsWith(`${PROXY}/`)) {
      seen.proxy.push(url);
    } else if (url.includes("/api/images/tickets")) {
      seen.tickets.push(url);
    }
  });
  return seen;
};

/**
 * The admin's settings as the server would send them. The base allows
 * direct too (REMOTE_IMAGES_ALLOW_DIRECT=true), so both routes are there;
 * the defaults alone don't allow it with a proxy.
 */
const adminPolicy = async (
  page: Page,
  changes: Partial<AdminImagePolicy>
): Promise<void> => {
  await page.route("**/auth/token*", async (route: Route) => {
    const response = await route.fetch();
    const grant: unknown = await response.json();
    await route.fulfill({
      json: {
        ...(typeof grant === "object" && grant !== null ? grant : {}),
        imagePolicy: {
          ...DEFAULT_ADMIN_IMAGE_POLICY,
          direct: true,
          ...changes,
        },
      },
      response,
    });
  });
};

/** The BFF's tickets and the proxy's batch answer (no storage). */
const fakeProxy = async (page: Page): Promise<void> => {
  await page.route("**/api/images/tickets", async (route) => {
    await route.fulfill({
      json: {
        images: [{ src: `${PROXY}/i/t1`, ticket: "t1", url: BANNER }],
        proxyOrigin: PROXY,
      },
    });
  });
  await page.route(`${PROXY}/**`, async (route) => {
    const body: unknown = route.request().postDataJSON();
    const cache =
      typeof body === "object" &&
      body !== null &&
      "mode" in body &&
      body.mode === "cache";
    await route.fulfill(
      cache
        ? { json: { error: "no storage" }, status: 400 }
        : {
            json: {
              images: {
                t1: { data: PIXEL_PNG.toString("base64"), type: "image/png" },
              },
            },
          }
    );
  });
};

/** Straight from the sender: its server answers with a pixel. */
const fakeSender = async (page: Page): Promise<void> => {
  await page.route("https://cdn.example/**", async (route) => {
    await route.fulfill({ body: PIXEL_PNG, contentType: "image/png" });
  });
  await page.route("https://tracker.example/**", async (route) => {
    await route.fulfill({ body: PIXEL_PNG, contentType: "image/png" });
  });
};

const choose = async (page: Page, group: string, option: string) => {
  const radios = page.getByRole("radiogroup", { name: group });
  await expect(radios).toBeVisible();
  await radios.getByRole("radio", { exact: true, name: option }).click();
  await expect(
    radios.getByRole("radio", { exact: true, name: option })
  ).toBeChecked();
};

const openSettings = async (page: Page) => {
  await page.goto("/settings/general");
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Privacy" })).toBeVisible();
};

const reader = (page: Page) => page.getByRole("region", { name: "Message" });

const openEmail = async (page: Page) => {
  await page.goto(EMAIL);
  // A busy machine: the first open of a page can be slow.
  await expect(
    reader(page).getByRole("heading", { name: "Team photo" })
  ).toBeVisible({ timeout: 20_000 });
  // The body is in its iframe (the render is done).
  await expect(
    reader(page).locator("iframe[title]").first().contentFrame().locator("body")
  ).toContainText("team photo", { timeout: 20_000 });
};

/** The email's banner image, as rendered in its iframe. */
const banner = (page: Page) =>
  reader(page)
    .locator("iframe[title]")
    .first()
    .contentFrame()
    .getByRole("img", { name: "Banner" });

test.beforeEach(async ({ context, page }) => {
  // Several pages per test (settings, then the email).
  test.slow();
  await signIn(context);
  jmap = await installFakeJmap(page);
});

test("never: no image request at all, through the proxy or the sender", async ({
  page,
}) => {
  // Even with the proxy and direct allowed, and the admin's default "always".
  await adminPolicy(page, { defaultLoad: "always", proxy: true });
  await fakeProxy(page);
  await fakeSender(page);
  const seen = watch(page);
  await openSettings(page);
  await choose(page, "Remote images", "Never");
  // "How they load" doesn't matter then.
  await expect(
    page.getByRole("radiogroup", { name: "How they load" })
  ).toHaveCount(0);

  await openEmail(page);
  await expect(
    reader(page).getByText(/images blocked by your settings/u)
  ).toBeVisible();
  await expect(
    reader(page).getByRole("button", { name: /Show images|Load from sender/u })
  ).toHaveCount(0);
  // Long enough for anything that would load at once.
  await page.waitForTimeout(1500);
  await expect(banner(page)).not.toHaveAttribute("src", /./u);
  expect(seen).toStrictEqual({
    proxy: [],
    sender: [],
    tickets: [],
    tracker: [],
  });
  await reader(page).getByRole("button", { name: "View report" }).click();
  await expect(
    page.getByText(/Your settings never load remote images/u)
  ).toBeVisible();
});

test("ask (default) through the proxy: only after the click", async ({
  page,
}) => {
  await adminPolicy(page, { proxy: true });
  await fakeProxy(page);
  const seen = watch(page);
  await openEmail(page);
  await page.waitForTimeout(500);
  expect(seen.tickets).toHaveLength(0);
  await reader(page).getByRole("button", { name: "Show images" }).click();
  await expect(banner(page)).toHaveAttribute("src", /^blob:/u);
  await expect(
    reader(page).getByText(/images through the proxy/u)
  ).toBeVisible();
  expect(seen.tickets).toHaveLength(1);
  // Never from the sender, and the tracker stays removed.
  expect(seen.sender).toHaveLength(0);
  expect(seen.tracker).toHaveLength(0);
});

test("always through the proxy: loads at once, never from the sender", async ({
  page,
}) => {
  await adminPolicy(page, { proxy: true });
  await fakeProxy(page);
  await fakeSender(page);
  const seen = watch(page);
  await openSettings(page);
  await choose(page, "Remote images", "Always");
  await choose(page, "How they load", "Through the proxy");
  await openEmail(page);
  await expect(banner(page)).toHaveAttribute("src", /^blob:/u);
  await expect(
    reader(page).getByRole("button", { name: "Show images" })
  ).toHaveCount(0);
  expect(seen.tickets).toHaveLength(1);
  expect(seen.sender).toHaveLength(0);
  expect(seen.tracker).toHaveLength(0);
});

test("always from the sender: loads at once in the frame page, with the IP notice", async ({
  page,
}) => {
  await adminPolicy(page, { proxy: true });
  await fakeSender(page);
  const seen = watch(page);
  const framePages: { csp: string | null; frame: string | null }[] = [];
  page.on("response", (response) => {
    if (new URL(response.url()).pathname === "/email-frame.html") {
      framePages.push({
        csp: response.headers()["content-security-policy"] ?? null,
        frame: response.headers()["x-frame-options"] ?? null,
      });
    }
  });
  await openSettings(page);
  await choose(page, "Remote images", "Always");
  await choose(page, "How they load", "From the sender");
  await expect(
    page.getByText(/the sender sees your IP address/u)
  ).toBeVisible();

  await openEmail(page);
  await expect(banner(page)).toHaveAttribute("src", BANNER);
  await expect(
    reader(page).getByText(/the sender saw your IP address/u)
  ).toBeVisible();
  await expect.poll(() => seen.sender.length).toBeGreaterThan(0);
  expect(seen.tracker).toHaveLength(0);
  expect(seen.tickets).toHaveLength(0);
  // The page's own policy: https images, nothing that runs; only we frame it.
  expect(framePages[0]).toStrictEqual({
    csp: "default-src 'none'; img-src data: https:; style-src 'unsafe-inline'; font-src data:; form-action 'none'; base-uri 'none'; frame-ancestors 'self'",
    frame: "SAMEORIGIN",
  });
  // Still sandboxed like every email: no scripts, forms or navigation.
  await expect(reader(page).locator("iframe[title]").first()).toHaveAttribute(
    "sandbox",
    "allow-same-origin"
  );
});

test("without a proxy (this server): direct only, after the IP warning", async ({
  page,
}) => {
  // What this server says without a proxy (direct allowed), whatever a
  // local .env.local adds (its IMAGE_PROXY_KEYS would turn the proxy on).
  await adminPolicy(page, { proxy: false });
  await fakeSender(page);
  const seen = watch(page);
  await openSettings(page);
  // "Through the proxy" isn't offered; loading at once needs the switch.
  await expect(
    page.getByRole("radiogroup", { name: "How they load" })
  ).toHaveCount(0);
  const auto = page.getByRole("switch", {
    name: "Load from the sender without asking",
  });
  await expect(auto).not.toBeChecked();
  await choose(page, "Remote images", "Always");

  // "Always", but direct isn't chosen: asks first, with the warning.
  await openEmail(page);
  await page.waitForTimeout(500);
  expect(seen.sender).toHaveLength(0);
  await reader(page).getByRole("button", { name: "Load from sender" }).click();
  await expect(
    reader(page).getByText(/the sender will see your IP address/u)
  ).toBeVisible();
  expect(seen.sender).toHaveLength(0);
  await reader(page).getByRole("button", { name: "Load anyway" }).click();
  await expect(banner(page)).toHaveAttribute("src", BANNER);
  await expect.poll(() => seen.sender.length).toBeGreaterThan(0);

  // Switched on: at once.
  await openSettings(page);
  await auto.click();
  await expect(auto).toBeChecked();
  await openEmail(page);
  await expect(banner(page)).toHaveAttribute("src", BANNER);
  expect(seen.tracker).toHaveLength(0);
});

test("trusted senders: asks, then 'Always from this sender' loads it at once", async ({
  page,
}) => {
  await adminPolicy(page, { proxy: true });
  await fakeProxy(page);
  const seen = watch(page);
  await openSettings(page);
  await choose(page, "Remote images", "Trusted senders");
  await openEmail(page);
  await page.waitForTimeout(500);
  expect(seen.tickets).toHaveLength(0);
  await reader(page)
    .getByRole("button", { name: "Always from this sender" })
    .click();
  await expect(banner(page)).toHaveAttribute("src", /^blob:/u);

  // Next time, at once, and the bar says why.
  await openEmail(page);
  await expect(banner(page)).toHaveAttribute("src", /^blob:/u);
  await expect(reader(page).getByText(/trusted sender/u)).toBeVisible();
  expect(seen.tickets).toHaveLength(2);

  // Settings shows the list and can forget it.
  await openSettings(page);
  await expect(page.getByText(/1 sender trusted/u)).toBeVisible();
  await page.getByRole("button", { name: "Forget them" }).click();
  await expect(page.getByText(/sender trusted/u)).toHaveCount(0);
});

test("the admin forbids direct: only the proxy, and no 'Load from sender'", async ({
  page,
}) => {
  await adminPolicy(page, { direct: false, proxy: true });
  await fakeProxy(page);
  await fakeSender(page);
  const seen = watch(page);
  await openSettings(page);
  await expect(
    page.getByRole("radiogroup", { name: "How they load" })
  ).toHaveCount(0);
  await expect(
    page.getByText(/doesn’t allow loading them from the sender/u)
  ).toBeVisible();
  await openEmail(page);
  await expect(
    reader(page).getByRole("button", { name: "Load from sender" })
  ).toHaveCount(0);
  await reader(page).getByRole("button", { name: "Show images" }).click();
  await expect(banner(page)).toHaveAttribute("src", /^blob:/u);
  expect(seen.sender).toHaveLength(0);
});

test("the admin forbids direct without a proxy, or forces never: nothing loads", async ({
  page,
}) => {
  await adminPolicy(page, { direct: false, proxy: false });
  await fakeSender(page);
  const seen = watch(page);
  await openSettings(page);
  await expect(page.getByText(/can’t be shown on this server/u)).toBeVisible();
  await expect(
    page.getByRole("radiogroup", { name: "Remote images" })
  ).toHaveCount(0);
  await openEmail(page);
  await expect(
    reader(page).getByRole("button", { name: /Show images|Load from sender/u })
  ).toHaveCount(0);

  await page.unrouteAll({ behavior: "wait" });
  jmap = await installFakeJmap(page);
  await adminPolicy(page, {
    defaultLoad: "never",
    forceNever: true,
    proxy: true,
  });
  await fakeSender(page);
  await openSettings(page);
  await expect(
    page.getByText("Your administrator blocks remote images for everyone.")
  ).toBeVisible();
  await openEmail(page);
  await page.waitForTimeout(1000);
  expect(seen).toStrictEqual({
    proxy: [],
    sender: [],
    tickets: [],
    tracker: [],
  });
});

test("the choice persists and is synced", async ({ page }) => {
  jmap.fileNode = true;
  await adminPolicy(page, { proxy: true });
  await openSettings(page);
  await choose(page, "Remote images", "Always");
  await choose(page, "How they load", "From the sender");
  // Both choices reach the account's synced copy (prefs-sync).
  await expect
    .poll(() => JSON.stringify(syncedPreferences(jmap)?.values ?? {}), {
      timeout: 15_000,
    })
    .toMatch(
      /"remoteImagesLoad":\{"at":\d+,"value":"always"\},"remoteImagesVia":\{"at":\d+,"value":"direct"\}/u
    );
  await page.reload();
  await expect(
    page
      .getByRole("radiogroup", { name: "Remote images" })
      .getByRole("radio", { exact: true, name: "Always" })
  ).toBeChecked();
  await expect(
    page
      .getByRole("radiogroup", { name: "How they load" })
      .getByRole("radio", { exact: true, name: "From the sender" })
  ).toBeChecked();
});
