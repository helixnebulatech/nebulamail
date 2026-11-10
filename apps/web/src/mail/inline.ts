// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which parts of an email are images of its HTML (`cid:`) and which are
// attachments to list. Decided like mail-render finds the images it needs
// (`classifyImageUrl` on the parsed document: entities decoded, `CID:` in
// any case, the same `normalizeCid`), so the reader shows an inline image
// and the editor keeps it.

import {
  SAFE_CID,
  classifyImageUrl,
  normalizeContentId,
  parseInertHtml,
} from "@nebulamail/mail-render";

/** Image types shown inline (never SVG). */
export const RASTER =
  /^image\/(?:png|jpeg|gif|webp|avif|bmp|x-icon|vnd\.microsoft\.icon)$/u;

/** Attributes whose value is one image URL (mail-render's images.ts). */
const IMAGE_ATTRIBUTES = ["src", "background", "poster"] as const;

/** `url(cid:…)` in CSS, quoted or not. */
const CSS_CID = /url\(\s*["']?\s*(?<url>cid:[^"')\s]+)/giu;

/**
 * Content-IDs (normalized) the HTML refers to; null when there is no DOM
 * to read it with (then nothing is decided by the HTML).
 */
export const referencedCids = (
  html: string | null
): ReadonlySet<string> | null => {
  if (html === null || html === "") {
    return new Set();
  }
  if (typeof DOMParser === "undefined") {
    return null;
  }
  const found = new Set<string>();
  const add = (url: string): void => {
    const classified = classifyImageUrl(url);
    if (classified.kind === "cid") {
      found.add(classified.cid);
    }
  };
  const addCss = (css: string | null): void => {
    for (const match of (css ?? "").matchAll(CSS_CID)) {
      add(match.groups?.url ?? "");
    }
  };
  const doc = parseInertHtml(html);
  for (const element of doc.querySelectorAll("*")) {
    for (const name of IMAGE_ATTRIBUTES) {
      const value = element.getAttribute(name);
      if (value !== null) {
        add(value);
      }
    }
    for (const candidate of (element.getAttribute("srcset") ?? "").split(",")) {
      add(candidate.trim().split(/\s+/u)[0] ?? "");
    }
    addCss(element.getAttribute("style"));
    if (element.localName === "style") {
      addCss(element.textContent);
    }
  }
  return found;
};

/**
 * An inline image of the HTML: a raster image with a Content-ID the
 * editor can write back, not marked as an attachment, that the HTML uses
 * (`referenced`; null: unknown). Anything else is an attachment: listed
 * in the reader and forwarded.
 */
export const isInlineImage = (
  part: {
    readonly cid: string;
    readonly type: string;
    readonly disposition: string | null | undefined;
  },
  referenced: ReadonlySet<string> | null
): boolean =>
  part.cid !== "" &&
  SAFE_CID.test(part.cid) &&
  part.disposition?.toLowerCase() !== "attachment" &&
  RASTER.test(part.type.toLowerCase()) &&
  (referenced === null || referenced.has(normalizeContentId(part.cid)));
