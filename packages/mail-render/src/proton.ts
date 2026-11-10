// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Adapter over the vendored, unmodified Proton sanitizer.
// Proton's module uses DOMPurify's default instance, which binds to
// the global `window` when it is first imported: in a browser (or a test
// with a jsdom environment) that is the page window; on a server without a
// global window DOMPurify is "unsupported" and would return its input
// unchanged, so we refuse to run there.

import DOMPurify from "dompurify";

import { protonizer } from "../vendor/proton-sanitize/src/purify";

export class SanitizerUnavailableError extends Error {
  constructor() {
    super(
      "DOMPurify has no DOM here; mail-render's HTML pipeline needs a browser-like global window"
    );
    this.name = "SanitizerUnavailableError";
  }
}

export const sanitizerAvailable = (): boolean => DOMPurify.isSupported;

/**
 * Proton's `protonizer` config: whole document, scripts and dangerous
 * elements removed, every loading attribute renamed (`src`→`proton-src`…),
 * `url(`→`proton-url(` in styles, fixed/absolute positioning neutralized.
 * Returns the inert document DOMPurify parsed into (never the live page).
 */
export const protonize = (html: string): Document => {
  if (!sanitizerAvailable()) {
    throw new SanitizerUnavailableError();
  }
  const root = protonizer(html, true);
  const doc = root.ownerDocument;
  if (root.nodeName !== "HTML" || doc.documentElement !== root) {
    throw new Error("Unexpected protonizer result");
  }
  return doc;
};
