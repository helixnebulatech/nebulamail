// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// renderEmail: the whole rendering pipeline, in order.
//   0. limits                       → limits.ts
//   1. Proton sanitizer (protonizer)       → proton.ts (vendored code)
//   2. remove base/meta/link/form controls → sanitize.ts
//   3. links: no target, rel=noopener…     → links.ts
//   4. inline images (data:, cid:)         → images.ts
//   5. trackers (list + spy pixels), always→ images.ts + trackers/
//   6. clean links (tidy-url)              → links.ts
//   7. remote images: off unless allowed, only via the image source
//   8. iframe document: CSP first in head  → document.ts
//   9. iframe attributes and helpers       → frame.ts (used by the app)
//  10. link confirmation                   → links.ts (used by the app on click)
//  11. privacy report                      → report.ts
//  12. plain-text view                     → text.ts
//  13. trusted senders                     → settings.ts (shouldLoadRemoteImages)
// Steps 4, 5 and 7 share one pass over the image references (each is
// decided once); steps 3 and 6 share one pass over the links.
// The result is checked once more after re-parsing (verify.ts).

import { findCssHazard } from "./css-guard";
import {
  buildContentSecurityPolicy,
  createFrameDocument,
  imageSourcesFor,
  serializeFrameDocument,
  validateImageSource,
} from "./document";
import type { TextScheme } from "./document";
import { processImages } from "./images";
import type { CidResolver, ImageSource } from "./images";
import {
  DEFAULT_LIMITS,
  checkHtmlLimits,
  measureTree,
  roughHtmlToText,
} from "./limits";
import type { LimitViolation, RenderLimits } from "./limits";
import { processLinks } from "./links";
import { applyStyleNonce, validateStyleNonce } from "./nonce";
import { SanitizerUnavailableError, protonize } from "./proton";
import { mintRenderedFrame } from "./rendered-frame";
import type { RenderedFrame } from "./rendered-frame";
import { ReportBuilder } from "./report";
import type { PrivacyReport } from "./report";
import { removeDangerousContent } from "./sanitize";
import { shouldLoadRemoteImages } from "./settings";
import type { PrivacySettings, RemoteImageContext } from "./settings";
import { appendPlainText } from "./text";
import { parseInertHtml } from "./trusted-types";
import { findInertnessProblems } from "./verify";

export interface EmailContent {
  /** The text/html body part, if any. */
  readonly html?: string | null;
  /** The text/plain body part, if any. */
  readonly text?: string | null;
}

export interface RenderOptions extends RemoteImageContext {
  /** Where remote images may load from. Default: none. */
  readonly images?: ImageSource;
  /** Resolves `cid:` references to `data:` URLs (see `neededCids`). */
  readonly cids?: CidResolver;
  /** Override the default view (the user pressed "View as text"). */
  readonly view?: "html" | "text";
  readonly limits?: Partial<RenderLimits>;
  /**
   * The current app page's CSP nonce. The srcdoc inherits the page's CSP,
   * so with `style-src 'nonce-…'` every `<style>` needs it (validated:
   * 16–64 base64/base64url characters, else InvalidStyleNonceError).
   */
  readonly styleNonce?: string;
  /**
   * Colours of a plain-text render (the text view, or the fallback): the
   * app's theme, so text mail isn't a white sheet in dark mode. Default
   * "light". HTML keeps its own colours (always a light document).
   */
  readonly textScheme?: TextScheme;
}

export type FallbackReason =
  | LimitViolation
  | "no-html"
  | "integrity-check-failed";

export interface RenderResult {
  readonly view: "html" | "text";
  /**
   * The document for the email's iframe: `iframe.srcdoc =
   * trustedSrcdoc(frame)`, or `fillFramePage(iframe, frame)` (frame.ts).
   * Only `renderEmail` makes these, and only after the final check.
   */
  readonly frame: RenderedFrame;
  /** The same document as a string (tests and diagnostics). */
  readonly srcdoc: string;
  /** The CSP that heads the document (for tests and diagnostics). */
  readonly csp: string;
  readonly report: PrivacyReport;
  /** Content-IDs referenced by the HTML; download them and pass `cids`. */
  readonly neededCids: readonly string[];
  /**
   * Remote image URLs (trackers excluded) the BFF may ticket. The BFF must
   * not trust this list: it extracts its own with
   * `extractRemoteImageUrls`. Useful to know whether to show "Show images".
   */
  readonly remoteImages: readonly string[];
  /** Whether remote images were restored in this render. */
  readonly remoteImagesLoaded: boolean;
  /** Why the HTML part is not shown, when `view` is "text" by necessity. */
  readonly fallbackReason?: FallbackReason;
  /** Details when `fallbackReason` is "integrity-check-failed". */
  readonly problems?: readonly string[];
}

const NONE: ImageSource = { kind: "none" };

/**
 * The srcdoc and the proof that it may go into an iframe. Only for the
 * srcdoc that is returned (after verification), never for one that failed.
 */
const trusted = (
  srcdoc: string
): { readonly frame: RenderedFrame; readonly srcdoc: string } => ({
  frame: mintRenderedFrame(srcdoc),
  srcdoc,
});

const requireDom = (): {
  implementation: DOMImplementation;
  parser: DOMParser;
} => {
  if (typeof document === "undefined" || typeof DOMParser === "undefined") {
    throw new TypeError("renderEmail needs a DOM (browser, or jsdom in tests)");
  }
  return { implementation: document.implementation, parser: new DOMParser() };
};

/** `direct` is only possible when the user turned the proxy off. */
const effectiveSource = (
  source: ImageSource | undefined,
  settings: PrivacySettings
): ImageSource => {
  const validated = validateImageSource(source ?? NONE);
  if (validated.kind === "direct" && settings.loadThroughProxy) {
    return NONE;
  }
  return validated;
};

/** How the plain-text document is built (nonce, colours). */
interface TextFrame {
  readonly styleNonce: string | undefined;
  readonly textScheme: TextScheme;
}

const renderText = (
  text: string,
  settings: PrivacySettings,
  limits: RenderLimits,
  fallbackReason: FallbackReason | undefined,
  report: ReportBuilder,
  frame: TextFrame,
  problems?: readonly string[]
): RenderResult => {
  const { styleNonce, textScheme } = frame;
  const { implementation } = requireDom();
  const csp = buildContentSecurityPolicy(imageSourcesFor(NONE));
  const doc = createFrameDocument(implementation, {
    csp,
    styleNonce,
    textScheme,
  });
  appendPlainText(doc.body, text.slice(0, limits.maxTextLength), {
    cleanLinks: settings.cleanLinks,
    report,
  });
  applyStyleNonce(doc, styleNonce);
  return {
    csp,
    neededCids: [],
    remoteImages: [],
    remoteImagesLoaded: false,
    report: report.build(),
    ...trusted(serializeFrameDocument(doc)),
    view: "text",
    ...(fallbackReason === undefined ? {} : { fallbackReason }),
    ...(problems === undefined ? {} : { problems }),
  };
};

/** Copies only presentational, non-loading attributes of `<body>`/`<html>`. */
const BODY_ATTRIBUTES = [
  "style",
  "class",
  "bgcolor",
  "text",
  "link",
  "vlink",
  "alink",
  "background",
  "dir",
  "lang",
];

const fallbackText = (
  input: EmailContent,
  html: string,
  limits: RenderLimits
): string =>
  typeof input.text === "string" && input.text !== ""
    ? input.text
    : roughHtmlToText(html, limits.maxTextLength);

interface HtmlRender {
  readonly html: string;
  readonly settings: PrivacySettings;
  readonly options: RenderOptions;
  readonly limits: RenderLimits;
  readonly report: ReportBuilder;
  readonly nonce: string | undefined;
  readonly source: ImageSource;
  readonly implementation: DOMImplementation;
  readonly parser: DOMParser;
  readonly fallback: (
    reason: FallbackReason,
    problems?: readonly string[]
  ) => RenderResult;
}

/** Steps 1–11 (the HTML view). May throw on hostile input: see renderEmail. */
const renderHtml = ({
  html,
  settings,
  options,
  limits,
  report,
  nonce,
  source,
  implementation,
  parser,
  fallback,
}: HtmlRender): RenderResult => {
  const parsed = parseInertHtml(html, parser);
  // The estimate before parsing is approximate: the parsed tree decides,
  // here and in the server extraction (extract.ts), before any more work.
  const raw = measureTree(parsed.documentElement);
  if (raw.elements > limits.maxElements || raw.depth > limits.maxNestingDepth) {
    return fallback(
      raw.elements > limits.maxElements ? "too-many-elements" : "too-deep"
    );
  }
  // CSS that Proton's vendored comment regex would be quadratic on.
  if (findCssHazard(parsed)) {
    return fallback("too-complex");
  }
  // Step 1: Proton's sanitizer, into an inert document.
  const sanitized = protonize(html);
  // DOMPurify drops a leading <frameset>: then there is no body at all.
  const sanitizedBody = sanitized.body as HTMLElement | null;
  if (sanitizedBody === null) {
    return fallback("integrity-check-failed", ["no-body"]);
  }
  const measured = measureTree(sanitized.documentElement);
  if (
    measured.elements > limits.maxElements ||
    measured.depth > limits.maxNestingDepth
  ) {
    return fallback(
      measured.elements > limits.maxElements ? "too-many-elements" : "too-deep"
    );
  }
  // Step 2: our removals.
  removeDangerousContent(sanitized, report);
  // Steps 3 + 6: links.
  processLinks(sanitized, { cleanLinks: settings.cleanLinks, report });
  // Steps 4 + 5 + 7 (+ 13 through shouldLoadRemoteImages): images.
  const neededCids = new Set<string>();
  const loadRemote =
    source.kind !== "none" && shouldLoadRemoteImages(settings, options);
  processImages(sanitized, {
    loadRemote,
    neededCids,
    report,
    settings,
    source,
    ...(options.cids === undefined ? {} : { cids: options.cids }),
  });

  // Every <style> (head and body) gets the nonce before it is connected to
  // the frame document (see document.ts); the inert parse doesn't check CSP.
  applyStyleNonce(sanitized, nonce);
  // Step 8: a fresh document with CSP first, then the sanitized content.
  const csp = buildContentSecurityPolicy(imageSourcesFor(source));
  const frame = createFrameDocument(implementation, {
    csp,
    styleNonce: nonce,
    dir: sanitized.documentElement.getAttribute("dir"),
    lang: sanitized.documentElement.getAttribute("lang"),
  });
  for (const style of sanitized.head.querySelectorAll("style")) {
    frame.head.append(frame.importNode(style, true));
  }
  for (const name of BODY_ATTRIBUTES) {
    const value = sanitizedBody.getAttribute(name);
    if (value !== null) {
      frame.body.setAttribute(name, value);
    }
  }
  for (const child of sanitizedBody.childNodes) {
    frame.body.append(frame.importNode(child, true));
  }
  applyStyleNonce(frame, nonce);
  const srcdoc = serializeFrameDocument(frame);

  // Re-parse and verify (mXSS defence). Never show something that fails.
  const reparsed = parseInertHtml(srcdoc, parser);
  const problems = findInertnessProblems(reparsed, csp, source);
  if (problems.length > 0) {
    return fallback("integrity-check-failed", problems);
  }

  return {
    csp,
    neededCids: [...neededCids],
    remoteImages: report.remoteUrls(),
    remoteImagesLoaded: loadRemote,
    report: report.build(),
    ...trusted(srcdoc),
    view: "html",
  };
};

export const renderEmail = (
  input: EmailContent,
  settings: PrivacySettings,
  options: RenderOptions = {}
): RenderResult => {
  const limits: RenderLimits = { ...DEFAULT_LIMITS, ...options.limits };
  const report = new ReportBuilder();
  const html = input.html ?? "";
  const view = options.view ?? settings.defaultView;
  const nonce =
    options.styleNonce === undefined
      ? undefined
      : validateStyleNonce(options.styleNonce);
  const textFrame: TextFrame = {
    styleNonce: nonce,
    textScheme: options.textScheme === "dark" ? "dark" : "light",
  };
  // Plain text, by choice or because there is no HTML part.
  if (view === "text" || html.trim() === "") {
    const reason = view === "text" ? undefined : "no-html";
    return renderText(
      input.text ?? "",
      settings,
      limits,
      reason,
      report,
      textFrame
    );
  }
  // Step 0: limits on the raw string, before any parsing.
  const violation = checkHtmlLimits(html, limits);
  if (violation !== null) {
    return renderText(
      fallbackText(input, html, limits),
      settings,
      limits,
      violation,
      report,
      textFrame
    );
  }

  const source = effectiveSource(options.images, settings);
  const { implementation, parser } = requireDom();
  const fallback = (
    reason: FallbackReason,
    problems?: readonly string[]
  ): RenderResult =>
    renderText(
      fallbackText(input, html, limits),
      settings,
      limits,
      reason,
      // A fresh report: nothing of the failed HTML render is shown.
      new ReportBuilder(),
      textFrame,
      problems
    );
  // Any problem in the HTML pipeline (a hostile email that makes the
  // sanitizer or the DOM throw) falls back to text: renderEmail never
  // throws because of the email. Only configuration errors propagate.
  try {
    return renderHtml({
      fallback,
      html,
      implementation,
      limits,
      nonce,
      options,
      parser,
      report,
      settings,
      source,
    });
  } catch (error) {
    if (error instanceof SanitizerUnavailableError) {
      throw error;
    }
    return fallback("integrity-check-failed", ["render-error"]);
  }
};
