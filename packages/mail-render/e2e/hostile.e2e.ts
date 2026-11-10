// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// Every hostile email in security/hostile-emails, rendered by the real
// pipeline into the real sandboxed iframe, in Chromium and Firefox
//
//   - images off: zero network requests;
//   - images on: requests only to the image proxy;
//   - never a dialog, popup or navigation of the page.
// Each email runs with all defences, without the page CSP, and with the
// sanitizer alone (the iframe's CSP meta stripped too): each layer must
// hold on its own.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";
import { z } from "zod";

import type { HarnessOptions, HarnessResult, Layer } from "./harness";

const APP = "https://app.test";
const PROXY = "https://proxy.test";
const NONCE = "e2eNonceAbCdEfGhIjKlMn";
const CORPUS = path.join(
  import.meta.dirname,
  "../../../security/hostile-emails"
);

// Same policy as apps/web/src/server/security-headers.ts (keep in sync).
const PAGE_CSP = [
  "default-src 'none'",
  "base-uri 'none'",
  `connect-src 'self' ${PROXY}`,
  "font-src 'self'",
  "form-action 'self'",
  "frame-ancestors 'none'",
  "frame-src 'none'",
  `img-src 'self' data: blob: ${PROXY}`,
  "object-src 'none'",
  `script-src 'nonce-${NONCE}' 'strict-dynamic'`,
  `style-src 'self' 'nonce-${NONCE}'`,
  "style-src-attr 'unsafe-inline'",
].join("; ");

const bundleHarness = (): string => {
  const outdir = mkdtempSync(path.join(tmpdir(), "mail-render-e2e-"));
  execFileSync(
    "bun",
    [
      "build",
      path.join(import.meta.dirname, "harness.ts"),
      "--target=browser",
      "--format=esm",
      `--outdir=${outdir}`,
    ],
    { stdio: "pipe" }
  );
  return readFileSync(path.join(outdir, "harness.js"), "utf-8");
};

const harness = bundleHarness();

const fixtures = readdirSync(CORPUS)
  .filter((file) => file.endsWith(".html"))
  .toSorted();

const Expectations = z.looseObject({
  expect: z
    .looseObject({
      view: z.enum(["html", "text"]).optional(),
      visibleText: z.array(z.string()).optional(),
    })
    .optional(),
});

const expectations = (fixture: string): z.infer<typeof Expectations> =>
  Expectations.parse(
    JSON.parse(
      readFileSync(
        path.join(CORPUS, fixture.replace(/\.html$/u, ".json")),
        "utf-8"
      )
    )
  );

interface Observed {
  readonly outside: string[];
  readonly dialogs: string[];
  readonly popups: number;
}

/** Serves the fake app and proxy; records everything else and aborts it. */
const prepare = async (page: Page, layer: Layer): Promise<Observed> => {
  const observed: Observed = { dialogs: [], outside: [], popups: 0 };
  page.on("dialog", (dialog) => {
    observed.dialogs.push(dialog.message());
    void dialog.dismiss();
  });
  page.context().on("page", () => {
    (observed as { popups: number }).popups += 1;
  });
  await page.context().route("**/*", async (route) => {
    const url = route.request().url();
    if (url === `${APP}/`) {
      await route.fulfill({
        body: `<!doctype html><html><head><meta charset="utf-8"><title>e2e</title></head><body><script type="module" nonce="${NONCE}" src="/harness.js"></script></body></html>`,
        contentType: "text/html",
        headers: layer === "all" ? { "content-security-policy": PAGE_CSP } : {},
      });
    } else if (url === `${APP}/harness.js`) {
      await route.fulfill({ body: harness, contentType: "text/javascript" });
    } else {
      observed.outside.push(url);
      // The proxy answers with a 1×1 transparent GIF; anything else is a leak.
      await (url.startsWith(`${PROXY}/`)
        ? route.fulfill({
            body: Buffer.from(
              "R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==",
              "base64"
            ),
            contentType: "image/gif",
          })
        : route.abort());
    }
  });
  await page.goto(`${APP}/`);
  await page.waitForFunction(() => typeof window.renderHostile === "function");
  return observed;
};

const run = async (
  page: Page,
  fixture: string,
  options: HarnessOptions
): Promise<HarnessResult> => {
  const html = readFileSync(path.join(CORPUS, fixture), "utf-8");
  return await page.evaluate(
    async ([source, opts]) => await window.renderHostile(source, opts),
    [html, options] as const
  );
};

const LAYERS: readonly Layer[] = ["all", "no-page-csp", "sanitizer-only"];

// Proves the harness sees requests made from inside the srcdoc iframe:
// without the pipeline, the same email must leak.
test("control: an unsanitized email is caught leaking", async ({ page }) => {
  const observed = await prepare(page, "no-page-csp");
  await run(page, "spy-pixels.html", {
    images: false,
    layer: "no-page-csp",
    nonce: NONCE,
    proxyOrigin: PROXY,
    raw: true,
  });

  expect(observed.outside.length).toBeGreaterThan(0);
});

for (const fixture of fixtures) {
  test.describe(fixture, () => {
    for (const layer of LAYERS) {
      test(`images off, ${layer}: no request leaves the page`, async ({
        page,
      }) => {
        const observed = await prepare(page, layer);
        const result = await run(page, fixture, {
          images: false,
          layer,
          nonce: NONCE,
          proxyOrigin: PROXY,
        });

        expect(observed.outside).toStrictEqual([]);
        expect([observed.dialogs, observed.popups, page.url()]).toStrictEqual([
          [],
          0,
          `${APP}/`,
        ]);
        expect(result.view).toBe(
          expectations(fixture).expect?.view ?? result.view
        );
        expect(result.view).toMatch(/^(?:html|text)$/u);
      });

      test(`images on, ${layer}: only the image proxy`, async ({ page }) => {
        const observed = await prepare(page, layer);
        await run(page, fixture, {
          images: true,
          layer,
          nonce: NONCE,
          proxyOrigin: PROXY,
        });
        const foreign = observed.outside.filter(
          (url) => !url.startsWith(`${PROXY}/i/`)
        );

        expect(foreign).toStrictEqual([]);
        expect([observed.dialogs, observed.popups, page.url()]).toStrictEqual([
          [],
          0,
          `${APP}/`,
        ]);
      });
    }

    test("still shows the email's legitimate text", async ({ page }) => {
      await prepare(page, "all");
      const result = await run(page, fixture, {
        images: false,
        layer: "all",
        nonce: NONCE,
        proxyOrigin: PROXY,
      });
      const wanted = expectations(fixture).expect?.visibleText ?? [];

      expect(
        wanted.filter((text) => !result.text.includes(text))
      ).toStrictEqual([]);
    });
  });
}
