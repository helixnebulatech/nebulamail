// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the browser and the server share about the full BFF mode
// (server/jmap-proxy/). No server code here: the
// pages, the service worker and the server import it.

/** Every JMAP request goes under this path of the app's own origin. */
export const JMAP_PROXY_PREFIX = "/jmap-proxy/";

/** The session the browser connects to (its URLs all point back here). */
export const JMAP_PROXY_SESSION_PATH = `${JMAP_PROXY_PREFIX}session`;

/**
 * What the proxy says on /auth/token instead of a token (404): the
 * browser then uses the proxy (session/bff.ts, pwa/sw.ts).
 */
export const JMAP_PROXY_TOKEN_ERROR = "jmap-proxy";

/**
 * Required on every proxy request (anti-CSRF): a page on another site
 * can't send a custom header without a CORS preflight, which the proxy
 * never answers. `1` from the tabs; `background` from the service worker
 * (a push), which doesn't count as use of the app (idle limit).
 */
export const JMAP_PROXY_HEADER = "X-Nebula-Proxy";
export const JMAP_PROXY_FOREGROUND = "1";
export const JMAP_PROXY_BACKGROUND = "background";

/**
 * A random id per page load, on the tab's proxy requests. A push stream
 * opened again with the same id (and the same types) ends the old one at
 * once: some hosts (Vercel) never tell the server that the browser left,
 * and every dropped connection would keep its slot until it times out.
 */
export const JMAP_PROXY_CLIENT_HEADER = "X-Nebula-Client";
/** What a client id may look like (a UUID fits). */
export const JMAP_PROXY_CLIENT_ID = /^[\w-]{8,64}$/u;
