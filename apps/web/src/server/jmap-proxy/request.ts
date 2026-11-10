// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the JMAP proxy accepts from the browser
// (§8): the anti-CSRF checks, the routes and their parameters,
// and the request and response headers that cross the proxy. Pure
// functions, tested on their own (request.test.ts).
//
// Nothing from the browser becomes part of an upstream URL except the
// values parsed here, which are then re-encoded by the URL template
// (`expandTemplate`): ids with the strict JMAP `Id` syntax, a file name
// without control characters that is never `.` or `..`, a media type, and
// the push parameters.

import {
  JMAP_PROXY_BACKGROUND,
  JMAP_PROXY_FOREGROUND,
  JMAP_PROXY_HEADER,
  JMAP_PROXY_PREFIX,
} from "../../lib/jmap-proxy";

/** The proxy's routes, under JMAP_PROXY_PREFIX. */
export type ProxyRoute =
  | { readonly kind: "grant" }
  | { readonly kind: "session" }
  | { readonly kind: "api" }
  | { readonly kind: "upload"; readonly accountId: string }
  | {
      readonly kind: "download";
      readonly accountId: string;
      readonly blobId: string;
      readonly name: string;
      readonly type: string;
    }
  | {
      readonly kind: "eventsource";
      readonly types: string;
      readonly closeafter: string;
      readonly ping: string;
    };

export type RouteMatch =
  | { readonly ok: true; readonly route: ProxyRoute }
  | { readonly ok: false; readonly status: 400 | 404 | 405 };

/** RFC 8620 §1.2: 1–255 characters of the URL-safe base64 alphabet. */
const JMAP_ID = /^[\w-]{1,255}$/u;
/** RFC 6838 type/subtype, without parameters. */
const MEDIA_TYPE = /^[\w!#$&^.+-]{1,64}\/[\w!#$&^.+-]{1,64}$/u;
/** Data type names (RFC 8620 §7.3: `types`), comma-separated, or `*`. */
const TYPE_LIST = /^(?:\*|[A-Za-z]{1,64}(?:,[A-Za-z]{1,64}){0,31})$/u;
const PING = /^\d{1,4}$/u;
// oxlint-disable-next-line no-control-regex -- control characters are what we refuse
const CONTROL = /[\u0000-\u001F\u007F-\u009F]/u;
const MAX_NAME_LENGTH = 255;

export const DEFAULT_DOWNLOAD_TYPE = "application/octet-stream";

/** One path segment, percent-decoded; `undefined` if malformed. */
const decodeSegment = (segment: string): string | undefined => {
  try {
    return decodeURIComponent(segment);
  } catch {
    return undefined;
  }
};

const jmapId = (segment: string | undefined): string | undefined => {
  const value = segment === undefined ? undefined : decodeSegment(segment);
  return value !== undefined && JMAP_ID.test(value) ? value : undefined;
};

/**
 * A download's file name: any text (it is re-encoded into the upstream
 * URL), but no control characters (CR/LF included), not empty, and never
 * `.` or `..`, which survive encoding as dot segments of a path.
 */
export const downloadName = (segment: string): string | undefined => {
  const value = decodeSegment(segment);
  if (
    value === undefined ||
    value === "" ||
    value === "." ||
    value === ".." ||
    value.length > MAX_NAME_LENGTH ||
    CONTROL.test(value) ||
    !value.isWellFormed()
  ) {
    return undefined;
  }
  return value;
};

const mediaType = (value: string | null): string | undefined => {
  if (value === null || value === "") {
    return DEFAULT_DOWNLOAD_TYPE;
  }
  return MEDIA_TYPE.test(value) ? value.toLowerCase() : undefined;
};

const bad = { ok: false, status: 400 } as const;
const notFound = { ok: false, status: 404 } as const;
const wrongMethod = { ok: false, status: 405 } as const;

const routeFor = (
  method: string,
  wanted: "GET" | "POST",
  route: ProxyRoute
): RouteMatch => (method === wanted ? { ok: true, route } : wrongMethod);

const downloadRoute = (
  method: string,
  rest: readonly string[],
  url: URL
): RouteMatch => {
  if (rest.length !== 3) {
    return notFound;
  }
  const [account = "", blob = "", file = ""] = rest;
  const accountId = jmapId(account);
  const blobId = jmapId(blob);
  const name = downloadName(file);
  const type = mediaType(url.searchParams.get("type"));
  if (
    accountId === undefined ||
    blobId === undefined ||
    name === undefined ||
    type === undefined
  ) {
    return bad;
  }
  return routeFor(method, "GET", {
    accountId,
    blobId,
    kind: "download",
    name,
    type,
  });
};

const eventSourceRoute = (
  method: string,
  rest: readonly string[],
  url: URL
): RouteMatch => {
  if (rest.length !== 0) {
    return notFound;
  }
  const types = url.searchParams.get("types") ?? "*";
  const closeafter = url.searchParams.get("closeafter") ?? "no";
  const ping = url.searchParams.get("ping") ?? "0";
  if (
    !TYPE_LIST.test(types) ||
    (closeafter !== "no" && closeafter !== "state") ||
    !PING.test(ping)
  ) {
    return bad;
  }
  return routeFor(method, "GET", {
    closeafter,
    kind: "eventsource",
    ping,
    types,
  });
};

const uploadRoute = (method: string, rest: readonly string[]): RouteMatch => {
  if (rest.length !== 1) {
    return notFound;
  }
  const accountId = jmapId(rest[0]);
  return accountId === undefined
    ? bad
    : routeFor(method, "POST", { accountId, kind: "upload" });
};

/** The routes without parameters, and their method. */
const PLAIN_ROUTES: Readonly<
  Record<string, readonly ["GET" | "POST", ProxyRoute]>
> = {
  api: ["POST", { kind: "api" }],
  grant: ["GET", { kind: "grant" }],
  session: ["GET", { kind: "session" }],
};

/**
 * The route for a request, from its method, path and query. Paths are
 * matched on the raw (still encoded) text, one segment at a time: an
 * encoded `/` (%2F) stays inside its segment and then fails its check.
 * (`new URL` has already removed `.` and `..` segments, encoded or not.)
 */
export const matchRoute = (method: string, url: URL): RouteMatch => {
  if (!url.pathname.startsWith(JMAP_PROXY_PREFIX)) {
    return notFound;
  }
  const [head = "", ...rest] = url.pathname
    .slice(JMAP_PROXY_PREFIX.length)
    .split("/");
  if (head === "upload") {
    return uploadRoute(method, rest);
  }
  if (head === "download") {
    return downloadRoute(method, rest, url);
  }
  if (head === "eventsource") {
    return eventSourceRoute(method, rest, url);
  }
  const plain = Object.hasOwn(PLAIN_ROUTES, head)
    ? PLAIN_ROUTES[head]
    : undefined;
  if (plain === undefined || rest.length !== 0) {
    return notFound;
  }
  return routeFor(method, plain[0], plain[1]);
};

export type CsrfCheck =
  | { readonly ok: true; readonly background: boolean }
  | { readonly ok: false; readonly reason: "header" | "origin" | "mode" };

/**
 * Anti-CSRF, on every proxy request, GETs included:
 *   - the custom header (a cross-site page can't send it without a CORS
 *     preflight, and nothing here answers one);
 *   - `Sec-Fetch-Site: same-origin`, or for browsers without it an
 *     `Origin` equal to the app's (so same-origin GETs need the former);
 *   - never a navigation (`Sec-Fetch-Mode: navigate`): downloads are
 *     fetched and saved from a `blob:` URL (mail/download.ts), so links
 *     and forms can never reach the proxy.
 * The SameSite=Strict proxy cookie is checked apart (hasProxyBinding).
 */
export const checkCsrf = (request: Request, appUrl: URL): CsrfCheck => {
  const marker = request.headers.get(JMAP_PROXY_HEADER);
  if (marker !== JMAP_PROXY_FOREGROUND && marker !== JMAP_PROXY_BACKGROUND) {
    return { ok: false, reason: "header" };
  }
  const site = request.headers.get("sec-fetch-site");
  const sameOrigin =
    site === null
      ? request.headers.get("origin") === appUrl.origin
      : site === "same-origin";
  if (!sameOrigin) {
    return { ok: false, reason: "origin" };
  }
  if (request.headers.get("sec-fetch-mode") === "navigate") {
    return { ok: false, reason: "mode" };
  }
  return { background: marker === JMAP_PROXY_BACKGROUND, ok: true };
};

/**
 * Browser request headers that reach the JMAP server; nothing else (no
 * cookies, no Authorization of the browser's, no forwarding headers). The
 * proxy adds its own Authorization and `Accept-Encoding: identity`.
 */
export const FORWARDED_REQUEST_HEADERS: readonly string[] = [
  "accept",
  "accept-language",
  "content-type",
  "last-event-id",
];

export const upstreamRequestHeaders = (
  request: Request,
  token: string
): Headers => {
  const headers = new Headers();
  for (const name of FORWARDED_REQUEST_HEADERS) {
    const value = request.headers.get(name);
    if (value !== null) {
      headers.set(name, value);
    }
  }
  headers.set("authorization", `Bearer ${token}`);
  // Bytes pass through as they come: a compressed body would be inflated
  // by fetch and no longer match its Content-Length.
  headers.set("accept-encoding", "identity");
  return headers;
};

/**
 * JMAP server response headers that reach the browser; nothing else (no
 * Set-Cookie, no CORS, no Location, no server CSP). Content-Length only
 * when the body wasn't compressed (fetch inflates it).
 */
export const FORWARDED_RESPONSE_HEADERS: readonly string[] = [
  "content-type",
  "content-length",
  "retry-after",
];

export const browserResponseHeaders = (upstream: Response): Headers => {
  const headers = new Headers();
  const encoded = upstream.headers.has("content-encoding");
  for (const name of FORWARDED_RESPONSE_HEADERS) {
    const value = upstream.headers.get(name);
    if (value !== null && !(name === "content-length" && encoded)) {
      headers.set(name, value);
    }
  }
  // Mail data: never in a shared or disk cache (and gone with the session).
  headers.set("cache-control", "no-store");
  // Never rendered on our origin, whatever the JMAP server says it is (a
  // failed download's error body too): navigations get a 403 anyway.
  headers.set("content-security-policy", "sandbox; default-src 'none'");
  headers.set("x-content-type-options", "nosniff");
  return headers;
};

/**
 * `attachment` with the name re-encoded (RFC 6266/8187: `filename*`, plus
 * an ASCII fallback), whatever the JMAP server said.
 */
export const contentDisposition = (name: string): string => {
  const ascii = name.replaceAll(/[^\u0020-\u007E]|["\\%]/gu, "_");
  const encoded = encodeURIComponent(name).replaceAll(
    /['()*]/gu,
    (char) => `%${char.codePointAt(0)?.toString(16).toUpperCase() ?? ""}`
  );
  return `attachment; filename="${ascii}"; filename*=UTF-8''${encoded}`;
};

/**
 * A download's own headers: always a file (`attachment`), never rendered
 * (sandboxed CSP, nosniff), the media type checked or else opaque bytes.
 */
export const downloadHeaders = (upstream: Response, name: string): Headers => {
  const headers = browserResponseHeaders(upstream);
  const type = headers.get("content-type")?.split(";")[0]?.trim() ?? "";
  if (!MEDIA_TYPE.test(type)) {
    headers.set("content-type", DEFAULT_DOWNLOAD_TYPE);
  }
  headers.set("content-disposition", contentDisposition(name));
  return headers;
};
