// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The «Verified» badge (src/mail/auth-results.ts): only a
// `dmarc=pass` from our server whose `header.from` is the From's domain
// counts, and only with the setting on (off by default). A pass for
// another domain or a DMARC fail never shows it.

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import type { FakeEmail, FakeJmap } from "./fake-jmap";
import { installFakeJmap } from "./fake-jmap";
import { signIn } from "./session";

test.use({ locale: "en-US" });

test.beforeEach(({ browserName }, info) => {
  test.skip(
    browserName !== "chromium" || info.project.name !== "desktop",
    "the header is the same at every width"
  );
});

let jmap: FakeJmap;

/** Builds an address (keeps the test free of address literals). */
const at = (local: string, domain: string): string => `${local}@${domain}`;

/** From billing at bank.example, with our server's (jmap.test) results. */
const fromBank = (
  id: string,
  subject: string,
  authResults: string
): FakeEmail => ({
  authResults,
  from: { email: at("billing", "bank.example"), name: "Bank" },
  html: `<p>${subject}</p>`,
  id,
  keywords: { $seen: true },
  mailboxIds: { inbox: true },
  receivedAt: "2026-11-03T09:00:00Z",
  subject,
});

const SPF_DKIM = "dkim=pass header.d=bank.example; spf=pass";

test.beforeEach(async ({ context, page }) => {
  await signIn(context);
  jmap = await installFakeJmap(page);
  jmap.emails.unshift(
    fromBank(
      "v-aligned",
      "Aligned pass",
      `jmap.test; ${SPF_DKIM}; dmarc=pass header.from=bank.example`
    ),
    fromBank(
      "v-other",
      "Pass for another domain",
      "jmap.test; dkim=pass header.d=attacker.example; spf=pass; dmarc=pass header.from=attacker.example"
    ),
    fromBank(
      "v-fail",
      "DMARC fail",
      `jmap.test; ${SPF_DKIM}; dmarc=fail header.from=bank.example`
    )
  );
});

const reader = (page: Page) => page.getByRole("region", { name: "Message" });

const open = async (page: Page, id: string, subject: string) => {
  await page.goto(`/mail/inbox/${id}`);
  await expect(
    reader(page).getByRole("heading", { name: subject })
  ).toBeVisible();
  // The header has the sender once the body (and its headers) loaded.
  await expect(reader(page).getByText("Bank").first()).toBeVisible();
  await page.waitForLoadState("networkidle");
};

const badge = (page: Page) =>
  reader(page).getByText("Verified", { exact: true });

/** Turns «Show “Verified sender” badge» on or off in Settings › General. */
const setBadge = async (page: Page, on: boolean) => {
  await page.goto("/settings/general");
  const toggle = page.getByRole("switch", { name: /Verified sender/u });
  await expect(toggle).toBeVisible();
  await page.waitForLoadState("networkidle");
  if ((await toggle.isChecked()) !== on) {
    await toggle.click();
  }
  await expect(toggle).toBeChecked({ checked: on });
};

test("aligned dmarc=pass shows «Verified»", async ({ page }) => {
  await setBadge(page, true);
  await open(page, "v-aligned", "Aligned pass");
  await expect(badge(page)).toBeVisible();
});

test("a dmarc=pass for another domain shows no badge", async ({ page }) => {
  await setBadge(page, true);
  await open(page, "v-other", "Pass for another domain");
  await expect(badge(page)).toHaveCount(0);
});

test("dmarc=fail shows no «Verified» (but «Not verified»)", async ({
  page,
}) => {
  await setBadge(page, true);
  await open(page, "v-fail", "DMARC fail");
  await expect(badge(page)).toHaveCount(0);
  await expect(
    reader(page).getByText("Not verified", { exact: true })
  ).toBeVisible();
});

test("with the setting off, an aligned pass shows no badge", async ({
  page,
}) => {
  // Off by default; turning it on and off again checks the switch too.
  await setBadge(page, true);
  await setBadge(page, false);
  await open(page, "v-aligned", "Aligned pass");
  await expect(badge(page)).toHaveCount(0);
});
