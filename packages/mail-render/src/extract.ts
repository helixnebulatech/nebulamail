// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Server-side extraction of the remote image URLs of an email, for the BFF
// to decide what to ticket: the BFF downloads the
// email itself and never trusts a list sent by the browser.
//
// Why an injected parser instead of the full browser pipeline: Proton's
// vendored module uses DOMPurify's default instance, which binds to the
// global `window` at import time; on a server there is none (and faking a
// global window for the whole process is worse). Extraction is read-only,
// so it does not need sanitizing: it parses inertly, walks the same
// references the browser pipeline restores (img src/srcset, `background`,
// inline `style`, `<style>`), skips the subtrees our pipeline removes,
// normalizes CSS with Proton's pure escape functions and decides each URL
// with the same shared `createImageDecider` (trackers, spy pixels, blocked
// URLs, the per-email reference limit).
//
// The result is a superset-or-equal of what the browser shows in normal
// documents (`renderEmail().remoteImages`), which tests check on the
// hostile corpus. A difference can only mean an image the browser does not
// show gets a ticket (all such URLs are still from this email and not
// trackers) or an image the browser wanted gets none (it stays blocked).
//
// The parser must be inert (no script execution, no resource loading) AND
// follow the HTML spec, so the server sees the same tree as the browser:
// jsdom's DOMParser (parse5) or the browser's. Not linkedom: it isn't
// spec-compliant for fragments (`<p>a</p><img>` puts the <img> outside the
// document element), and most email bodies are fragments.

import { escapeURLinStyle } from "../vendor/proton-sanitize/src/escape";
import { cssCommentHazard, findCssUrls, neutralizeCss } from "./css";
import { createImageDecider } from "./image-policy";
import type {
  ImageDecider,
  ImageDecision,
  TrackerSettings,
} from "./image-policy";
import { DEFAULT_LIMITS, checkHtmlLimits, measureTree } from "./limits";
import type { RenderLimits } from "./limits";
import { DROPPED_WITH_CONTENT } from "./sanitize";
import { DEFAULT_PRIVACY } from "./settings";
import type { SpyPixelInput } from "./trackers/spy-pixel";
import { parseSrcset } from "./urls";

export interface HtmlParser {
  readonly parseFromString: (html: string, type: "text/html") => Document;
}

export interface ExtractOptions {
  readonly parser: HtmlParser;
  readonly settings?: TrackerSettings;
  readonly limits?: Partial<RenderLimits>;
}

export interface ExtractedImages {
  /** Remote http(s) image URLs to ticket, normalized, deduplicated, in document order. */
  readonly urls: readonly string[];
  /** Tracker references found (never ticketed). */
  readonly trackers: readonly {
    readonly service: string;
    readonly url: string;
  }[];
}

/**
 * What the extraction reads of an element: a DOM `Element` has it, and so
 * has the light tree built by the server's parser worker
 * (`server-extract.ts`).
 */
export interface ExtractElement {
  readonly localName: string;
  readonly namespaceURI: string | null;
  readonly textContent: string | null;
  readonly children: ArrayLike<ExtractElement> & Iterable<ExtractElement>;
  readonly getAttribute: (name: string) => string | null;
}

/** Attributes the extraction reads (the parser worker copies only these). */
export const EXTRACT_ATTRIBUTES: readonly string[] = [
  "background",
  "height",
  "src",
  "srcset",
  "style",
  "width",
];

/** Elements whose text the extraction reads. */
export const EXTRACT_TEXT_OF: readonly string[] = ["style"];

/** No images (empty, over a limit, or failed closed). */
export const NO_IMAGES: ExtractedImages = Object.freeze({
  trackers: [],
  urls: [],
});

/**
 * Asynchronous extraction, for servers: `createWorkerExtractor`
 * (`server-extract.ts`) parses in a worker thread with a deadline.
 * `null`: the HTML could not be parsed in time or within the limits, so
 * the email gets no images (fail closed).
 */
export interface ImageUrlExtractor {
  readonly extract: (
    html: string,
    options?: Omit<ExtractOptions, "parser">
  ) => Promise<ExtractedImages | null>;
}

/** HTML that is not parsed at all: the browser shows the text part. */
export const skipsHtml = (
  html: string,
  limits: Partial<RenderLimits> | undefined
): boolean =>
  html.trim() === "" ||
  checkHtmlLimits(html, { ...DEFAULT_LIMITS, ...limits }) !== null;

const hasCssHazard = (style: string | null, sheet: string | null): boolean =>
  (style !== null && cssCommentHazard(style)) ||
  (sheet !== null && cssCommentHazard(sheet));

/** An `<img>`'s decisions; a tracking one hides the rest (as the browser). */
const imgDecisions = (
  img: ExtractElement,
  decide: ImageDecider,
  dims: SpyPixelInput
): ImageDecision[] => {
  const src = img.getAttribute("src");
  const srcset = img.getAttribute("srcset");
  const decisions = [
    ...(src === null ? [] : [decide(src, dims)]),
    ...(srcset === null
      ? []
      : parseSrcset(srcset).map((c) => decide(c.url, dims))),
  ];
  const tracking = decisions.filter(
    (d) => d.kind === "tracker" || d.kind === "spy-pixel"
  );
  return tracking.length > 0 ? tracking : decisions;
};

/** The image URLs under `root` (the document element). */
export const extractFromElement = (
  root: ExtractElement,
  settings: TrackerSettings = DEFAULT_PRIVACY,
  partialLimits?: Partial<RenderLimits>
): ExtractedImages => {
  const limits: RenderLimits = { ...DEFAULT_LIMITS, ...partialLimits };
  // The estimate before parsing is approximate: the real tree decides, as
  // in the browser (which then shows the text part, so no images).
  const measured = measureTree(root);
  if (
    measured.elements > limits.maxElements ||
    measured.depth > limits.maxNestingDepth
  ) {
    return NO_IMAGES;
  }
  const decide = createImageDecider(settings);
  const urls = new Set<string>();
  const trackers: { service: string; url: string }[] = [];

  const record = (decision: ImageDecision): void => {
    if (decision.kind === "remote") {
      urls.add(decision.url);
    } else if (decision.kind === "tracker") {
      trackers.push({ service: decision.service, url: decision.url });
    } else if (decision.kind === "spy-pixel") {
      trackers.push({ service: "Spy pixel", url: decision.url });
    }
  };

  const fromCss = (css: string, dims?: SpyPixelInput): void => {
    for (const ref of findCssUrls(neutralizeCss(escapeURLinStyle(css)))) {
      if (ref.context === "image") {
        record(decide(ref.url, dims));
      }
    }
  };

  const stack: ExtractElement[] = [root];
  let element = stack.pop();
  while (element !== undefined) {
    const name = element.localName;
    // Subtrees whose content never reaches the browser output.
    const skipped =
      DROPPED_WITH_CONTENT.has(name) ||
      element.namespaceURI !== "http://www.w3.org/1999/xhtml";
    if (!skipped) {
      const style = element.getAttribute("style");
      const sheet = name === "style" ? (element.textContent ?? "") : null;
      if (hasCssHazard(style, sheet)) {
        // The browser refuses this email's HTML (`too-complex`) and shows
        // the text part: no images at all, not just this CSS (its spy-pixel
        // sizes would be lost and the image would get a ticket).
        return NO_IMAGES;
      }
      // Same normalization the browser pipeline sees after Proton.
      const dims: SpyPixelInput = {
        height: element.getAttribute("height"),
        style: style === null ? null : escapeURLinStyle(style),
        width: element.getAttribute("width"),
      };
      if (style !== null) {
        fromCss(style, dims);
      }
      if (sheet !== null) {
        fromCss(sheet);
      }
      const background = element.getAttribute("background");
      if (background !== null) {
        record(decide(background, dims));
      }
      if (name === "img") {
        // Same rule as the browser: a tracking image loads nothing.
        for (const decision of imgDecisions(element, decide, dims)) {
          record(decision);
        }
      }
      for (let i = element.children.length - 1; i >= 0; i -= 1) {
        const child = element.children[i];
        if (child !== undefined) {
          stack.push(child);
        }
      }
    }
    element = stack.pop();
  }
  return { trackers, urls: [...urls] };
};

export const extractRemoteImageUrls = (
  html: string,
  options: ExtractOptions
): ExtractedImages => {
  if (skipsHtml(html, options.limits)) {
    return NO_IMAGES;
  }
  const doc = options.parser.parseFromString(html, "text/html");
  return extractFromElement(
    doc.documentElement,
    options.settings,
    options.limits
  );
};

/**
 * The same, in process, with an injected parser: tests and the browser.
 * Never on a server: one hostile email would stop it (`server-extract.ts`).
 */
export const inProcessExtractor = (parser: HtmlParser): ImageUrlExtractor => ({
  extract: async (html, options = {}) =>
    await Promise.resolve(extractRemoteImageUrls(html, { ...options, parser })),
});
