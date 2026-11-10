// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// unsubscribe.html of the static build (static/unsubscribe-page.ts)
// in a real browser. Both origins are answered here (no server needed):
// the page's and the sender's, which has a `SameSite=None` cookie. RFC 8058
// §3.1: the POST carries no cookies, authorization or other context.

import type { Request } from "@playwright/test";
import { expect, test } from "@playwright/test";

import { unsubscribePage } from "../src/static/unsubscribe-page";

const TEXTS = {
  body: "Ask {host}?",
  confirm: "Unsubscribe now",
  failed: "Could not send",
  invalid: "Bad link",
  sent: "Sent to {host}",
  title: "Unsubscribe?",
};
const PAGE = "https://mail.example.test/unsubscribe.html";
const TARGET = "https://lists.example.org/u/abc?list=1";

test.skip(
  ({ browserName, isMobile }) => browserName !== "chromium" || isMobile,
  "one desktop browser is enough for a static page"
);

test.beforeEach(async ({ context }) => {
  await context.addCookies([
    {
      domain: "lists.example.org",
      name: "tracker",
      path: "/",
      sameSite: "None",
      secure: true,
      value: "who-are-you",
    },
  ]);
  await context.route(PAGE, async (route) => {
    await route.fulfill({
      body: unsubscribePage({ en: TEXTS, es: TEXTS }),
      contentType: "text/html; charset=utf-8",
    });
  });
});

test("POSTs only after the click, without cookies, referrer or origin", async ({
  context,
  page,
}) => {
  const seen: { request: Request; headers: Record<string, string> }[] = [];
  await context.route("https://lists.example.org/**", async (route) => {
    const request = route.request();
    seen.push({ headers: await request.allHeaders(), request });
    await route.fulfill({ body: "ok", status: 200 });
  });

  await page.goto(`${PAGE}#${encodeURIComponent(TARGET)}`);
  await expect(page.getByText("Ask lists.example.org?")).toBeVisible();
  // The URL left the address bar; nothing was sent yet.
  expect(page.url()).toBe(PAGE);
  expect(seen).toHaveLength(0);

  await page.getByRole("button", { name: "Unsubscribe now" }).click();
  await expect(page.getByText("Sent to lists.example.org")).toBeVisible();
  await expect(page.getByRole("button")).toBeHidden();

  expect(seen).toHaveLength(1);
  const [first] = seen;
  if (first === undefined) {
    throw new Error("no request");
  }
  const { headers, request } = first;
  expect(request.url()).toBe(TARGET);
  expect(request.method()).toBe("POST");
  expect(request.postData()).toBe("List-Unsubscribe=One-Click");
  expect(headers["content-type"]).toBe("application/x-www-form-urlencoded");
  expect(headers.cookie).toBeUndefined();
  expect(headers.authorization).toBeUndefined();
  expect(headers.referer).toBeUndefined();
  // no-cors + no-referrer: the app's origin isn't sent either.
  expect(headers.origin).toBe("null");

  // Control: the same browser does send the cookie on a navigation, so its
  // absence above is the page's doing.
  await page.goto("https://lists.example.org/control");
  expect(seen[1]?.headers.cookie).toBe("tracker=who-are-you");
});

test("says so when the request can't be sent, and lets the user retry", async ({
  context,
  page,
}) => {
  let calls = 0;
  await context.route("https://lists.example.org/**", async (route) => {
    calls += 1;
    await (calls === 1
      ? route.abort("connectionrefused")
      : route.fulfill({ status: 204 }));
  });

  await page.goto(`${PAGE}#${encodeURIComponent(TARGET)}`);
  const button = page.getByRole("button", { name: "Unsubscribe now" });
  await button.click();
  await expect(page.getByText("Could not send")).toBeVisible();
  await button.click();
  await expect(page.getByText("Sent to lists.example.org")).toBeVisible();
  expect(calls).toBe(2);
});

test("a private address shows the error and no button", async ({ page }) => {
  await page.goto(`${PAGE}#${encodeURIComponent("https://[IP]/u")}`);
  await expect(page.getByText("Bad link")).toBeVisible();
  await expect(page.getByRole("button")).toBeHidden();
});
