// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// oxlint-disable vitest/no-conditional-expect, vitest/no-conditional-tests -- data-driven: each fixture's metadata decides which checks apply

import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";
import { z } from "zod";

import {
  DEFAULT_PRIVACY,
  analyzeAttachment,
  analyzeDisplayText,
  analyzeLink,
  extractRemoteImageUrls,
  isTrackerUrl,
  renderEmail,
} from "../src/index";
import type { ImageSource } from "../src/index";
import {
  imagesOffPolicy,
  imagesOnPolicy,
  scanNetworkSinks,
} from "./network-sinks";

const CORPUS = path.join(
  import.meta.dirname,
  "../../../security/hostile-emails"
);
const PROXY = "https://img.test";
const TICKET_PREFIX = `${PROXY}/t/`;

const proxy: ImageSource = {
  kind: "proxy",
  origin: PROXY,
  rewrite: (url) => `${TICKET_PREFIX}${encodeURIComponent(url)}`,
};

const Metadata = z.object({
  attachments: z
    .array(z.object({ name: z.string(), type: z.string() }))
    .optional(),
  attacks: z.array(z.string()).min(1),
  expect: z
    .object({
      forbiddenCss: z.array(z.string()).optional(),
      inlineImagesAtLeast: z.number().optional(),
      remoteImagesAtLeast: z.number().optional(),
      remoteImagesExactly: z.array(z.string()).optional(),
      spyPixels: z.number().optional(),
      trackerServicesInclude: z.array(z.string()).optional(),
      view: z.enum(["html", "text"]).optional(),
      visibleText: z.array(z.string()).optional(),
    })
    .optional(),
  from: z.object({ email: z.string(), name: z.string() }).optional(),
  links: z
    .array(
      z.object({
        href: z.string(),
        text: z.string(),
        warnings: z.array(z.string()),
      })
    )
    .optional(),
  references: z.array(z.string()).optional(),
  subject: z.string().optional(),
  title: z.string(),
});

const fixtures = readdirSync(CORPUS)
  .filter((file) => file.endsWith(".html"))
  .toSorted()
  .map((file) => {
    const name = file.replace(/\.html$/u, "");
    return {
      html: readFileSync(path.join(CORPUS, file), "utf-8"),
      meta: Metadata.parse(
        JSON.parse(readFileSync(path.join(CORPUS, `${name}.json`), "utf-8"))
      ),
      name,
    };
  });

/** Every attribute value and style text of the parsed output. */
const loadedValues = (srcdoc: string): string[] =>
  [
    ...new DOMParser()
      .parseFromString(srcdoc, "text/html")
      .querySelectorAll("*"),
  ].flatMap((element) => [
    ...[...element.attributes].map((attr) => attr.value),
    ...(element.localName === "style" ? [element.textContent ?? ""] : []),
  ]);

const bodyText = (srcdoc: string): string =>
  (
    new DOMParser().parseFromString(srcdoc, "text/html").body.textContent ?? ""
  ).replaceAll(/\s+/gu, " ");

describe("hostile email corpus", () => {
  it("has fixtures, each with metadata", () => {
    expect(fixtures.length).toBeGreaterThanOrEqual(15);
    const all = readdirSync(CORPUS);
    for (const file of all.filter((f) => f.endsWith(".json"))) {
      expect(all).toContain(file.replace(/\.json$/u, ".html"));
    }
  });

  describe.each(fixtures)("$name", ({ html, meta }) => {
    const off = renderEmail({ html }, DEFAULT_PRIVACY, { images: proxy });
    const on = renderEmail({ html }, DEFAULT_PRIVACY, {
      images: proxy,
      userApproved: true,
    });

    const view = meta.expect?.view ?? "html";

    it(`renders as ${view} (no unexpected fallback)`, () => {
      if (view === "html") {
        expect(off.problems).toBeUndefined();
      }
      expect(off.view).toBe(view);
      expect(on.view).toBe(view);
    });

    it("images off: nothing executable, nothing that loads", () => {
      expect(
        scanNetworkSinks(off.srcdoc, imagesOffPolicy(off.csp))
      ).toStrictEqual([]);
      expect(off.report.remoteImagesLoaded).toBe(0);
    });

    it("images on: only proxy URLs, never a tracker", () => {
      expect(
        scanNetworkSinks(on.srcdoc, imagesOnPolicy(on.csp, PROXY))
      ).toStrictEqual([]);
      const proxied = [
        ...loadedValues(on.srcdoc)
          .join(" ")
          .matchAll(/https:\/\/img\.test\/t\/(?<ticket>[^"'\s),\\]+)/gu),
      ].map((m) => decodeURIComponent(m.groups?.ticket ?? ""));
      for (const url of proxied) {
        expect(url).toMatch(/^https?:\/\//u);
        expect(isTrackerUrl(url)).toBeNull();
        expect(on.remoteImages).toContain(url);
      }
    });

    it("plain-text view is inert too", () => {
      const text = renderEmail({ html, text: html }, DEFAULT_PRIVACY, {
        view: "text",
      });
      expect(
        scanNetworkSinks(text.srcdoc, imagesOffPolicy(text.csp))
      ).toStrictEqual([]);
      expect(bodyText(text.srcdoc)).toContain("SPDX-License-Identifier");
    });

    it("the BFF extractor finds every image the browser would load", () => {
      const extracted = extractRemoteImageUrls(html, {
        parser: new DOMParser(),
      });
      for (const url of on.remoteImages) {
        expect(extracted.urls).toContain(url);
      }
      for (const url of extracted.urls) {
        expect(isTrackerUrl(url)).toBeNull();
      }
    });

    it("meets the fixture's expectations", () => {
      const e = meta.expect ?? {};
      const text = bodyText(off.srcdoc);
      for (const visible of e.visibleText ?? []) {
        expect(text).toContain(visible);
      }
      if (e.spyPixels !== undefined) {
        const spy = off.report.trackers.find((t) => t.service === "Spy pixel");
        expect(spy?.count).toBe(e.spyPixels);
      }
      if (e.remoteImagesExactly !== undefined) {
        expect(off.remoteImages).toStrictEqual(e.remoteImagesExactly);
      }
      if (e.remoteImagesAtLeast !== undefined) {
        expect(off.remoteImages.length).toBeGreaterThanOrEqual(
          e.remoteImagesAtLeast
        );
      }
      for (const service of e.trackerServicesInclude ?? []) {
        expect(off.report.trackers.map((t) => t.service)).toContain(service);
      }
      for (const css of e.forbiddenCss ?? []) {
        expect(off.srcdoc.toLowerCase()).not.toContain(css);
      }
      if (e.inlineImagesAtLeast !== undefined) {
        const inline = (off.srcdoc.match(/src="data:image\//giu) ?? []).length;
        expect(inline).toBeGreaterThanOrEqual(e.inlineImagesAtLeast);
      }
    });

    if (meta.subject !== undefined || meta.from !== undefined) {
      it("display texts lose bidi and invisible characters", () => {
        for (const value of [meta.subject, meta.from?.name]) {
          if (value !== undefined) {
            const result = analyzeDisplayText(value);
            expect(result.altered).toBeTruthy();
            expect(result.text).not.toMatch(
              /[\u202A-\u202E\u2066-\u2069\u200B]/u
            );
          }
        }
      });
    }

    if (meta.attachments !== undefined) {
      it("attachments get warnings", () => {
        for (const attachment of meta.attachments ?? []) {
          expect(analyzeAttachment(attachment).warnings.length).toBeGreaterThan(
            0
          );
        }
      });
    }

    if (meta.links !== undefined) {
      it("deceptive links are flagged", () => {
        for (const link of meta.links ?? []) {
          expect(analyzeLink(link.href, link.text)?.warnings).toStrictEqual(
            link.warnings
          );
        }
      });
    }
  });
});
