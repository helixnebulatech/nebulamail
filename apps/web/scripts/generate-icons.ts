// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// After changing the icon, bump the ?v= of the icon links (__root.tsx,
// manifest.webmanifest) so browsers fetch it again.
//
// Generates the PWA icons in public/icons from public/favicon.svg (the
// Helix app icon, packages/ui AppIcon): `bun run icons`. Renders with
// Playwright's Chromium, so the PNGs match the SVG exactly.
//
// - icon-192/512.png: the rounded icon as is ("any").
// - maskable-192/512.png and apple-touch-icon.png: full-bleed square (the
//   system cuts its own shape) with the mark inside the safe zone.

import { readFile } from "node:fs/promises";
import path from "node:path";

import { chromium } from "@playwright/test";

const publicDir = path.join(import.meta.dirname, "../public");
const svg = await readFile(path.join(publicDir, "favicon.svg"), "utf-8");

/** Full-bleed variant: square corners, no hairline, the mark at `scale`. */
const fullBleed = (scale: number): string => {
  const offset = (64 - 64 * scale) / 2;
  return svg
    .replaceAll(' rx="16"', "")
    .replace(/<rect x="\.5"[^>]*\/>/u, "")
    .replace(
      'transform="translate(5 5) scale(.84375)"',
      `transform="translate(${offset} ${offset}) scale(${scale})"`
    );
};

const icons = [
  { file: "icon-192.png", size: 192, source: svg },
  { file: "icon-512.png", size: 512, source: svg },
  // Maskable safe zone: a circle of 80 % of the size; the mark stays inside.
  { file: "maskable-192.png", size: 192, source: fullBleed(0.6) },
  { file: "maskable-512.png", size: 512, source: fullBleed(0.6) },
  { file: "apple-touch-icon.png", size: 180, source: fullBleed(0.72) },
];

const browser = await chromium.launch();
try {
  await Promise.all(
    icons.map(async (icon) => {
      const page = await browser.newPage({
        viewport: { height: icon.size, width: icon.size },
      });
      const url = `data:image/svg+xml;base64,${Buffer.from(icon.source).toString("base64")}`;
      await page.setContent(
        `<style>html,body{margin:0;background:transparent}img{display:block}</style><img src="${url}" width="${icon.size}" height="${icon.size}">`
      );
      await page.locator("img").evaluate(async (img: HTMLImageElement) => {
        await img.decode();
      });
      await page.screenshot({
        omitBackground: true,
        path: path.join(publicDir, "icons", icon.file),
      });
      await page.close();
    })
  );
} finally {
  await browser.close();
}
