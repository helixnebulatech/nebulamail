// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The Content-Security-Policy of the static build. It
// goes in a <meta> of index.html, so it can't depend on the request:
//   - scripts by hash ('strict-dynamic' trusts what they load), never by
//     nonce;
//   - one nonce fixed at build time, for <style> only (Base UI, the email
//     iframes): knowing it lets someone add CSS, never run code;
//   - everything on the page's own origin (the app talks to Stalwart on
//     its own name, see session/oauth.ts), plus the image proxy if the zip
//     was built with one.
// What a <meta> can't do (frame-ancestors) comes from Stalwart's
// `Http.responseHeaders` (docs/static.md).

import { validateProxyOrigin } from "@nebulamail/mail-render/document";
import { TRUSTED_TYPES_POLICIES } from "@nebulamail/mail-render/trusted-types";

export interface StaticCspOptions {
  /** `sha256-…` of every inline script of index.html. */
  readonly scriptHashes: readonly string[];
  readonly styleNonce: string;
  /** The image proxy's origin, or "" for none. */
  readonly imageProxyOrigin: string;
}

/** The Trusted Types policy of the entry loader (static/plugin.ts). */
export const LOADER_POLICY = "nebula-loader";

const NONCE = /^[\w-]{16,64}$/u;
const HASH = /^sha256-[A-Za-z\d+/]{43}=$/u;

/** https (or http on localhost, for development), as mail-render wants it. */
const proxyOrigin = (value: string): string[] =>
  value === "" ? [] : [validateProxyOrigin(value)];

export const buildStaticCsp = (options: StaticCspOptions): string => {
  if (!NONCE.test(options.styleNonce)) {
    throw new Error("CSP: invalid style nonce");
  }
  if (options.scriptHashes.length === 0) {
    throw new Error("CSP: index.html has no inline script to hash");
  }
  for (const hash of options.scriptHashes) {
    if (!HASH.test(hash)) {
      throw new Error(`CSP: invalid script hash ${hash}`);
    }
  }
  const proxy = proxyOrigin(options.imageProxyOrigin);
  const directives: Record<string, readonly string[]> = {
    // Stalwart rewrites <base href> to where it mounts the app.
    "base-uri": ["'self'"],
    // The proxy only serves images (option D): no connections to it.
    "connect-src": ["'self'"],
    "default-src": ["'none'"],
    "font-src": ["'self'"],
    "form-action": ["'self'"],
    // Email bodies are `srcdoc` iframes, or the frame page of the zip when
    // their images load straight from the sender (mail/remote-images.ts).
    "frame-src": ["'self'"],
    "img-src": ["'self'", "data:", "blob:", ...proxy],
    "manifest-src": ["'self'"],
    "object-src": ["'none'"],
    "require-trusted-types-for": ["'script'"],
    "script-src": [
      ...new Set(options.scriptHashes.map((hash) => `'${hash}'`)),
      "'strict-dynamic'",
    ],
    "style-src": ["'self'", `'nonce-${options.styleNonce}'`],
    // Same reasoning as the BFF build (server/security-headers.ts).
    "style-src-attr": ["'unsafe-inline'"],
    "trusted-types": [...TRUSTED_TYPES_POLICIES, LOADER_POLICY],
    // The session worker (session/worker.ts) and the service worker.
    "worker-src": ["'self'"],
  };
  return Object.entries(directives)
    .map(([name, values]) => `${name} ${values.join(" ")}`)
    .join("; ");
};
