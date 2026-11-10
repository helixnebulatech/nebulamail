// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// CSP nonce for the email's `<style>` elements. The app page sends
// `style-src 'self' 'nonce-<per-request>'`; an about:srcdoc iframe inherits
// the parent's policy container, so both the page's CSP and the iframe's
// own meta CSP apply inside it, and a `<style>` without the page's current
// nonce is blocked. Inline `style=""` attributes keep working through the
// page's `style-src-attr 'unsafe-inline'` plus the iframe's own
// `style-src 'unsafe-inline'`.

export class InvalidStyleNonceError extends Error {
  constructor() {
    super("styleNonce must be 16-64 base64/base64url characters");
    this.name = "InvalidStyleNonceError";
  }
}

const NONCE = /^[A-Za-z0-9+/_=-]{16,64}$/u;

/** Throws InvalidStyleNonceError unless `nonce` is 16–64 base64/base64url characters. */
export const validateStyleNonce = (nonce: string): string => {
  if (!NONCE.test(nonce)) {
    throw new InvalidStyleNonceError();
  }
  return nonce;
};

/**
 * The srcdoc inherits the app page's CSP (about:srcdoc shares the parent's
 * policy container). The app uses `style-src 'self' 'nonce-…'`, so every
 * `<style>` must carry the current page's nonce or it is blocked. Set as a
 * DOM attribute, never by string concatenation. Email content cannot bring
 * its own nonce: sanitize.ts strips `nonce` from everything it keeps.
 */
export const applyStyleNonce = (
  doc: Document,
  nonce: string | undefined
): void => {
  if (nonce === undefined) {
    return;
  }
  for (const style of doc.querySelectorAll("style")) {
    style.setAttribute("nonce", nonce);
  }
};
