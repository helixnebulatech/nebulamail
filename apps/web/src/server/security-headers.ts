// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Security headers for the app. Pure functions so the policy is
// tested on its own; `__root.tsx` sends the CSP for pages (TanStack Start's
// official pattern: a per-request nonce in `ssr.nonce` and the root route's
// `headers`), `start.ts` adds the other headers to every response.

import { TRUSTED_TYPES_POLICIES } from "@nebulamail/mail-render/trusted-types";

import { FRAME_PAGE, FRAME_PAGE_CSP, framePage } from "../lib/frame-page";

/** Where this server serves the frame page (routes/email-frame[.]html.ts). */
export const FRAME_PAGE_PATH = `/${FRAME_PAGE}`;

export interface CspOptions {
  /** Per-request nonce, also given to the router (`ssr.nonce`). */
  readonly nonce: string;
  /** Origin of the JMAP/OIDC server: JMAP calls, push, blobs, login redirect. */
  readonly jmapOrigin: string;
  /**
   * JMAP_PROXY=on: all JMAP goes to this
   * origin's /jmap-proxy/, so pages may not connect to the JMAP server at
   * all; it stays in form-action for the login redirect only.
   */
  readonly jmapProxy?: boolean;
  /** Origin of the image proxy, once it exists. */
  readonly imageProxyOrigin?: string | undefined;
  /** Vite dev server: allows its WebSocket for hot reload. */
  readonly dev: boolean;
  /** The app is served over https (APP_URL): upgrade any http request. */
  readonly https: boolean;
  /**
   * APP_URL: the email frame page (FRAME_PAGE_PATH) is the only page the
   * app may put in an iframe (images straight from the sender).
   */
  readonly appUrl?: string | undefined;
}

const NONCE = /^[\w+/=-]{16,64}$/u;

/** A random nonce: 16 bytes in base64url. */
export const createNonce = (): string =>
  btoa(String.fromCodePoint(...crypto.getRandomValues(new Uint8Array(16))))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replace(/[=]+$/u, "");

const origin = (value: string): string => {
  const url = new URL(value);
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new Error(`CSP: ${value} is not an http(s) URL`);
  }
  return url.origin;
};

export const buildCsp = (options: CspOptions): string => {
  if (!NONCE.test(options.nonce)) {
    throw new Error("CSP: invalid nonce");
  }
  const nonce = `'nonce-${options.nonce}'`;
  const jmap = origin(options.jmapOrigin);
  const proxy =
    options.imageProxyOrigin === undefined
      ? []
      : [origin(options.imageProxyOrigin)];
  const directives: Record<string, readonly string[]> = {
    "base-uri": ["'none'"],
    // Push (SSE over fetch), JMAP API, blob downloads; the image proxy's
    // POST /batch.
    "connect-src": [
      "'self'",
      ...(options.jmapProxy === true ? [] : [jmap]),
      ...proxy,
      ...(options.dev ? ["ws:", "wss:"] : []),
    ],
    "default-src": ["'none'"],
    "font-src": ["'self'"],
    // Chrome also checks form-action on the redirect the login form
    // triggers: /auth/login answers with a redirect to the OIDC provider.
    "form-action": ["'self'", jmap],
    "frame-ancestors": ["'none'"],
    // Email bodies are `srcdoc` iframes (about:srcdoc), allowed by default,
    // or the frame page when their images come straight from the sender
    // (exactly that URL: its own CSP decides what it loads).
    "frame-src":
      options.appUrl === undefined
        ? ["'none'"]
        : [new URL(FRAME_PAGE_PATH, origin(options.appUrl)).href],
    "img-src": ["'self'", "data:", "blob:", ...proxy],
    "manifest-src": ["'self'"],
    "object-src": ["'none'"],
    // Trusted Types: HTML sinks only take values from
    // these policies (mail-render/src/trusted-types.ts).
    "require-trusted-types-for": ["'script'"],
    // `strict-dynamic`: scripts loaded by nonced scripts are trusted, so
    // the route chunks Vite splits out work without listing them.
    "script-src": [nonce, "'strict-dynamic'"],
    // StyleX emits static CSS files; inline <style> only with the nonce.
    "style-src": ["'self'", nonce],
    // `about:srcdoc` iframes inherit this policy, and email HTML is full of
    // `style=""` attributes. An attribute can't use selectors, and img-src /
    // connect-src stay closed, so this doesn't open CSS exfiltration.
    "style-src-attr": ["'unsafe-inline'"],
    "trusted-types": TRUSTED_TYPES_POLICIES,
    // Vite's dev client pings a restarted server from a blob: worker.
    "worker-src": ["'self'", ...(options.dev ? ["blob:"] : [])],
  };
  const policy = Object.entries(directives).map(
    ([name, values]) => `${name} ${values.join(" ")}`
  );
  if (options.https) {
    policy.push("upgrade-insecure-requests");
  }
  return policy.join("; ");
};

/**
 * Headers for every response (pages, server functions, /auth/*).
 * `framePage`: the email frame page, which only this app may frame.
 */
export const commonSecurityHeaders = (options: {
  readonly https: boolean;
  readonly framePage?: boolean;
}): Record<string, string> => ({
  "cross-origin-opener-policy": "same-origin",
  "cross-origin-resource-policy": "same-origin",
  "permissions-policy": [
    "camera=()",
    "microphone=()",
    "geolocation=()",
    "payment=()",
    "usb=()",
    "serial=()",
    "hid=()",
    "midi=()",
    "display-capture=()",
    "browsing-topics=()",
    "interest-cohort=()",
  ].join(", "),
  "referrer-policy": "no-referrer",
  "x-content-type-options": "nosniff",
  // Names in the app's own pages aren't resolved ahead (the email iframe
  // already says so in its own document, mail-render document.ts).
  "x-dns-prefetch-control": "off",
  "x-frame-options": options.framePage === true ? "SAMEORIGIN" : "DENY",
  ...(options.https
    ? {
        "strict-transport-security":
          "max-age=63072000; includeSubDomains; preload",
      }
    : {}),
});

/**
 * The frame page with its CSP as a header too: what loads in it is only
 * what mail-render allows an email with direct images, and only this
 * origin may frame it. The email itself comes sanitized from the app,
 * which also sandboxes the iframe (no scripts, forms or navigation).
 */
export const framePageResponse = (): Response =>
  new Response(framePage(), {
    headers: {
      "cache-control": "no-cache",
      "content-security-policy": `${FRAME_PAGE_CSP}; frame-ancestors 'self'`,
      "content-type": "text/html; charset=utf-8",
    },
  });
