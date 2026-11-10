// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The JMAP proxy of the full BFF mode
// (docs/jmap-proxy.md). With JMAP_PROXY=on the browser never gets an access
// token: it talks JMAP to /jmap-proxy/ on the app's own origin with its
// session cookie, and this server adds `Authorization: Bearer` (from the
// cookie, refreshed as /auth/token does) and talks to the JMAP server,
// which can live on an internal network (JMAP_INTERNAL_URL).
//
//   GET  /jmap-proxy/grant      what session/bff.ts needs (never a token)
//   GET  /jmap-proxy/session    the JMAP session, its URLs moved here
//   POST /jmap-proxy/api        API request, up to maxSizeRequest
//   POST /jmap-proxy/upload/{accountId}       streamed, up to maxSizeUpload
//   GET  /jmap-proxy/download/{accountId}/{blobId}/{name}?type=  streamed
//   GET  /jmap-proxy/eventsource?types=&closeafter=&ping=        push (SSE)
//
// SSRF: the only upstream is the configured server; its own
// URLs are accepted only on its origins (session.ts); browser input only
// enters as checked, re-encoded values (request.ts). No redirects are
// followed (upstream.ts). Only listed headers cross, never cookies. CSRF:
// custom header, origin, no navigations, and the SameSite=Strict proxy
// cookie (hasProxyBinding).

import { readLimited } from "@nebulamail/image-proxy/body";

import {
  JMAP_PROXY_CLIENT_HEADER,
  JMAP_PROXY_CLIENT_ID,
  JMAP_PROXY_SESSION_PATH,
} from "../../lib/jmap-proxy";
import { cookieName, parseCookies, PROXY_COOKIE } from "../auth/cookies";
import type { AuthConfig } from "../auth/handlers";
import {
  hasProxyBinding,
  json,
  sessionAccessToken,
  sessionClearingCookies,
} from "../auth/handlers";
import type { Upstream } from "./config";
import { ProxyError } from "./errors";
import { SupersededStreams } from "./push-clients";
import type { ProxyRoute } from "./request";
import {
  browserResponseHeaders,
  checkCsrf,
  downloadHeaders,
  matchRoute,
  upstreamRequestHeaders,
} from "./request";
import type { UpstreamEndpoints } from "./session";
import { browserSession, expandUpstream } from "./session";
import { releasingWhenDone, StreamLimits } from "./streams";
import type { UpstreamDeps } from "./upstream";
import {
  endpointsFor,
  knownMaxSizeRequest,
  readSession,
  send,
} from "./upstream";

export { resetEndpointCache } from "./upstream";

export interface ProxyDeps extends UpstreamDeps {
  /** Longest a push connection stays open; the browser reconnects. */
  readonly maxPushMs?: number;
  /** Caps on open push/download/upload streams (tests pass their own). */
  readonly streams?: StreamLimits;
  /** Push streams by client (tests pass their own). */
  readonly pushClients?: SupersededStreams;
}

/** The process-wide caps on open streams. */
const STREAMS = new StreamLimits();
/** The process-wide push streams by client. */
const PUSH_CLIENTS = new SupersededStreams();

/** Routes that keep a connection open for long. */
const STREAMING: ReadonlySet<ProxyRoute["kind"]> = new Set([
  "download",
  "eventsource",
  "upload",
]);

const SECOND = 1000;
/** Session and API: generous (big queries), then 504. */
const SESSION_TIMEOUT_MS = 15 * SECOND;
const API_TIMEOUT_MS = 60 * SECOND;
/** Uploads stream from the browser: long, but not forever. */
const UPLOAD_TIMEOUT_MS = 10 * 60 * SECOND;
/** Downloads: until the headers; the body then streams as long as it takes. */
const DOWNLOAD_HEADERS_TIMEOUT_MS = 30 * SECOND;
/**
 * Push connections end after this (the browser reconnects at once, with
 * Last-Event-ID): an open stream can't outlive a logout by more.
 */
export const DEFAULT_MAX_PUSH_MS = 5 * 60 * SECOND;
const GRANT_TTL_MS = 5 * 60 * SECOND;
/** Before the endpoints are known: RFC 8620's suggested maxSizeRequest. */
const MAX_API_BODY_BYTES = 10_000_000;

const upstreamOf = (auth: AuthConfig): Upstream =>
  auth.upstream ?? {
    origin: auth.oidc.issuer.origin,
    publicOrigin: auth.oidc.issuer.origin,
  };

const withCookies = (
  response: Response,
  cookies: readonly string[]
): Response => {
  if (cookies.length === 0) {
    return response;
  }
  const headers = new Headers(response.headers);
  for (const cookie of cookies) {
    headers.append("set-cookie", cookie);
  }
  return new Response(response.body, {
    headers,
    status: response.status,
    statusText: response.statusText,
  });
};

const unauthorized = (): Response => json({ error: "unauthorized" }, 401);

/** A JMAP server error status as the browser sees it. */
const fromStatus = (status: number): Response =>
  status === 401
    ? unauthorized()
    : json({ error: "unavailable" }, status >= 500 ? status : 502);

/** The JMAP server's answer, with only the allowed headers. */
const relay = (response: Response, headers?: Headers): Response =>
  new Response(response.body, {
    headers: headers ?? browserResponseHeaders(response),
    status: response.status,
  });

/** Passes bytes through, erroring once more than `max` went by. */
const capped = (
  body: ReadableStream<Uint8Array>,
  max: number
): ReadableStream<Uint8Array> => {
  let seen = 0;
  return body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform: (chunk, controller) => {
        seen += chunk.byteLength;
        if (seen > max) {
          controller.error(new ProxyError("too-large", "upload too large"));
          return;
        }
        controller.enqueue(chunk);
      },
    })
  );
};

const DECIMAL = /^\d+$/u;

/** The Content-Length a body declares: number, missing, or invalid. */
const declaredLength = (request: Request): number | null | "invalid" => {
  const value = request.headers.get("content-length")?.trim();
  if (value === undefined) {
    return null;
  }
  return DECIMAL.test(value) ? Number(value) : "invalid";
};

/**
 * A push stream that ends after `maxMs`, aborting the upstream connection;
 * the browser cancelling it aborts it too.
 */
const timeBoxed = (
  body: ReadableStream<Uint8Array>,
  maxMs: number,
  upstream: AbortController
): ReadableStream<Uint8Array> => {
  const reader = body.getReader();
  let timer: ReturnType<typeof setTimeout> | undefined = undefined;
  const stop = (): void => {
    clearTimeout(timer);
    upstream.abort();
  };
  return new ReadableStream<Uint8Array>({
    cancel: async () => {
      stop();
      await reader.cancel().catch(() => null);
    },
    pull: async (controller) => {
      try {
        const next = await reader.read();
        if (next.done) {
          stop();
          controller.close();
          return;
        }
        controller.enqueue(next.value);
      } catch {
        // Upstream aborted (time is up, or it went away): end cleanly.
        stop();
        controller.close();
      }
    },
    start: (controller) => {
      timer = setTimeout(() => {
        stop();
        try {
          controller.close();
        } catch {
          // Already closed.
        }
      }, maxMs);
    },
  });
};

/** One request through the proxy, for one route. */
interface Attempt {
  readonly route: ProxyRoute;
  readonly request: Request;
  readonly auth: AuthConfig;
  readonly deps: ProxyDeps;
  readonly upstream: Upstream;
  /** The API request body, read once so it can be sent twice. */
  readonly apiBody: string | undefined;
  /** The service worker (a push), not the user: doesn't move `lastSeenAt`. */
  readonly background: boolean;
  /** A push stream: aborted when the same client opens a new one. */
  readonly superseded?: AbortSignal;
}

type RouteOf<K extends ProxyRoute["kind"]> = Extract<ProxyRoute, { kind: K }>;

const timeout = (request: Request, ms: number): AbortSignal =>
  AbortSignal.any([request.signal, AbortSignal.timeout(ms)]);

const grant = (context: Attempt): Response =>
  json({
    expiresAt: context.deps.now() + GRANT_TTL_MS,
    imagePolicy: context.auth.imagePolicy,
    jmapProxy: true,
    serverHostname: new URL(context.upstream.publicOrigin).hostname,
    sessionUrl: new URL(JMAP_PROXY_SESSION_PATH, context.auth.appUrl).href,
    verifiedSenderBadge: context.auth.verifiedSenderBadge,
  });

const session = async (context: Attempt, token: string): Promise<Response> => {
  const read = await readSession(
    context.deps,
    context.upstream,
    token,
    timeout(context.request, SESSION_TIMEOUT_MS)
  );
  return read.ok
    ? json(browserSession(read.session, context.auth.appUrl))
    : fromStatus(read.status);
};

/** The endpoints, or the answer to give when they can't be had. */
const endpoints = async (
  context: Attempt,
  token: string,
  signal: AbortSignal
): Promise<UpstreamEndpoints | Response> => {
  const found = await endpointsFor(
    context.deps,
    context.upstream,
    token,
    signal
  );
  return typeof found === "number" ? fromStatus(found) : found;
};

const api = async (context: Attempt, token: string): Promise<Response> => {
  const signal = timeout(context.request, API_TIMEOUT_MS);
  const found = await endpoints(context, token, signal);
  if (found instanceof Response) {
    return found;
  }
  const body = context.apiBody ?? "";
  if (new TextEncoder().encode(body).byteLength > found.maxSizeRequest) {
    return json({ error: "too-large" }, 413);
  }
  return relay(
    await send(context.deps, found.apiUrl, {
      body,
      headers: upstreamRequestHeaders(context.request, token),
      method: "POST",
      signal,
    })
  );
};

const upload = async (
  context: Attempt,
  route: RouteOf<"upload">,
  token: string
): Promise<Response> => {
  const { request } = context;
  const signal = timeout(request, UPLOAD_TIMEOUT_MS);
  const found = await endpoints(context, token, signal);
  const refuse = async (response: Response): Promise<Response> => {
    await request.body?.cancel();
    return response;
  };
  if (found instanceof Response) {
    return await refuse(found);
  }
  const declared = declaredLength(request);
  if (declared === "invalid") {
    return await refuse(json({ error: "invalid" }, 400));
  }
  if (declared !== null && declared > found.maxSizeUpload) {
    return await refuse(json({ error: "too-large" }, 413));
  }
  const url = expandUpstream(
    found.uploadUrl,
    { accountId: route.accountId },
    context.upstream
  );
  if (url === undefined) {
    return await refuse(json({ error: "unavailable" }, 502));
  }
  return relay(
    await send(context.deps, url, {
      headers: upstreamRequestHeaders(request, token),
      method: "POST",
      signal,
      ...(request.body === null
        ? { body: new Uint8Array() }
        : {
            body: capped(request.body, found.maxSizeUpload),
            // Required by the Fetch spec for streamed request bodies.
            duplex: "half",
          }),
    })
  );
};

const download = async (
  context: Attempt,
  route: RouteOf<"download">,
  token: string
): Promise<Response> => {
  const controller = new AbortController();
  const signal = AbortSignal.any([context.request.signal, controller.signal]);
  const timer = setTimeout(() => {
    controller.abort();
  }, DOWNLOAD_HEADERS_TIMEOUT_MS);
  try {
    const found = await endpoints(context, token, signal);
    if (found instanceof Response) {
      return found;
    }
    const url = expandUpstream(
      found.downloadUrl,
      {
        accountId: route.accountId,
        blobId: route.blobId,
        name: route.name,
        type: route.type,
      },
      context.upstream
    );
    if (url === undefined) {
      return json({ error: "unavailable" }, 502);
    }
    const response = await send(context.deps, url, {
      headers: upstreamRequestHeaders(context.request, token),
      method: "GET",
      signal,
    });
    // The body then streams for as long as it takes.
    return relay(
      response,
      response.ok ? downloadHeaders(response, route.name) : undefined
    );
  } finally {
    clearTimeout(timer);
  }
};

/**
 * The answer to a push stream that a newer one of the same tab replaced
 * before it opened: it no longer holds a slot, so it never opens one.
 */
const supersededResponse = (): Response => json({ error: "superseded" }, 409);

/**
 * Whether a newer stream of the same tab ended this one. A function, read
 * fresh after each await (TypeScript would keep an earlier answer).
 */
const isSuperseded = (context: Attempt): boolean =>
  context.superseded?.aborted === true;

const eventSource = async (
  context: Attempt,
  route: RouteOf<"eventsource">,
  token: string
): Promise<Response> => {
  const controller = new AbortController();
  // AbortSignal.any, not an "abort" listener: by the time this runs (after
  // the token was read or refreshed) a newer stream of the same tab may
  // already have aborted `superseded` and taken over our slot, and a
  // listener added to an aborted signal never fires.
  const signal = AbortSignal.any([
    context.request.signal,
    controller.signal,
    ...(context.superseded === undefined ? [] : [context.superseded]),
  ]);
  const found = await endpoints(context, token, signal);
  if (found instanceof Response) {
    return found;
  }
  if (isSuperseded(context)) {
    return supersededResponse();
  }
  const url = expandUpstream(
    found.eventSourceUrl,
    { closeafter: route.closeafter, ping: route.ping, types: route.types },
    context.upstream
  );
  if (url === undefined) {
    return json({ error: "unavailable" }, 502);
  }
  const response = await send(context.deps, url, {
    cache: "no-store",
    headers: upstreamRequestHeaders(context.request, token),
    method: "GET",
    signal,
  });
  if (isSuperseded(context)) {
    // Its slot is gone: never relay a stream that holds none.
    controller.abort();
    await response.body?.cancel().catch(() => null);
    return supersededResponse();
  }
  const headers = browserResponseHeaders(response);
  // No buffering by reverse proxies in between (nginx).
  headers.set("x-accel-buffering", "no");
  return new Response(
    response.ok && response.body !== null
      ? timeBoxed(
          response.body,
          context.deps.maxPushMs ?? DEFAULT_MAX_PUSH_MS,
          controller
        )
      : response.body,
    { headers, status: response.status }
  );
};

/** One try of the route with this access token. */
const attempt = async (context: Attempt, token: string): Promise<Response> => {
  const { route } = context;
  switch (route.kind) {
    case "grant": {
      return grant(context);
    }
    case "session": {
      return await session(context, token);
    }
    case "api": {
      return await api(context, token);
    }
    case "upload": {
      return await upload(context, route, token);
    }
    case "download": {
      return await download(context, route, token);
    }
    case "eventsource": {
      return await eventSource(context, route, token);
    }
    default: {
      return json({ error: "not-found" }, 404);
    }
  }
};

/** An exception of one try, as the browser sees it: a fixed code. */
const failure = (error: unknown): Response => {
  const cause = error instanceof Error ? error.cause : undefined;
  const tooLarge = [error, cause].some(
    (item) => item instanceof ProxyError && item.kind === "too-large"
  );
  if (tooLarge) {
    return json({ error: "too-large" }, 413);
  }
  if (
    error instanceof DOMException &&
    (error.name === "TimeoutError" || error.name === "AbortError")
  ) {
    return json({ error: "timeout" }, 504);
  }
  return json({ error: "unavailable" }, 502);
};

const safely = async (context: Attempt, token: string): Promise<Response> => {
  try {
    const response = await attempt(context, token);
    if (response.status !== 401) {
      return response;
    }
    // Never the JMAP server's own 401 text.
    await response.body?.cancel().catch(() => null);
    return unauthorized();
  } catch (error) {
    return failure(error);
  }
};

/**
 * Runs one try; on a 401 from the JMAP server, refreshes the token once
 * (unless it was just refreshed) and tries again if the body allows it. A
 * second 401 (or an upload's first) goes to the browser as 401, with the
 * session cookie of the refresh: the browser may try once more.
 */
const withRefresh = async (context: Attempt): Promise<Response> => {
  const { auth, request } = context;
  const touch = !context.background;
  const first = await sessionAccessToken(request, auth, { touch });
  if (!first.ok) {
    await request.body?.cancel().catch(() => null);
    return first.response;
  }
  // Replaced (and its slot released) while the token was read: stop.
  if (isSuperseded(context)) {
    return withCookies(supersededResponse(), first.cookies);
  }
  const response = await safely(context, first.accessToken);
  if (response.status !== 401 || first.refreshed) {
    return withCookies(response, first.cookies);
  }
  const again = await sessionAccessToken(request, auth, { force: true, touch });
  if (!again.ok) {
    return again.response;
  }
  if (isSuperseded(context)) {
    return withCookies(supersededResponse(), again.cookies);
  }
  // An upload's body is gone: the browser sends it again (jmap-client).
  const retried =
    context.route.kind === "upload"
      ? response
      : await safely(context, again.accessToken);
  return withCookies(retried, again.cookies);
};

/** The API body, read once (it may be sent twice), or the refusal. */
const readApiBody = async (request: Request): Promise<string | Response> => {
  const declared = declaredLength(request);
  if (declared === "invalid") {
    return json({ error: "invalid" }, 400);
  }
  // At most the biggest request the JMAP server takes (once known).
  const cap = knownMaxSizeRequest() ?? MAX_API_BODY_BYTES;
  if (declared !== null && declared > cap) {
    return json({ error: "too-large" }, 413);
  }
  const body = await readLimited(request, cap);
  if (body.ok) {
    return body.text;
  }
  return body.code === "body-too-large"
    ? json({ error: "too-large" }, 413)
    : json({ error: "invalid" }, 400);
};

/** Every /jmap-proxy/ request (routes/jmap-proxy/$.ts). */
export const handleJmapProxy = async (
  request: Request,
  auth: AuthConfig,
  deps: ProxyDeps
): Promise<Response> => {
  const refuse = async (response: Response): Promise<Response> => {
    await request.body?.cancel().catch(() => null);
    return response;
  };
  if (auth.jmapProxy !== true) {
    return await refuse(json({ error: "not-found" }, 404));
  }
  const matched = matchRoute(request.method, new URL(request.url));
  if (!matched.ok) {
    const error = matched.status === 400 ? "invalid" : "not-found";
    return await refuse(json({ error }, matched.status));
  }
  const csrf = checkCsrf(request, auth.appUrl);
  if (!csrf.ok) {
    return await refuse(json({ error: "forbidden" }, 403));
  }
  if (!(await hasProxyBinding(request, auth))) {
    // The CSRF check passed, so this is the app itself: end the session
    // here too, or /login would see it and send the user back (a loop).
    return await refuse(
      json({ error: "unauthenticated" }, 401, sessionClearingCookies(auth))
    );
  }
  const { route } = matched;
  const apiBody = route.kind === "api" ? await readApiBody(request) : undefined;
  if (apiBody instanceof Response) {
    return await refuse(apiBody);
  }
  const context: Attempt = {
    apiBody,
    auth,
    background: csrf.background,
    deps,
    request,
    route,
    upstream: upstreamOf(auth),
  };
  if (!STREAMING.has(route.kind)) {
    return await withRefresh(context);
  }
  // The proxy cookie was just checked: it names this session.
  const key =
    parseCookies(request.headers.get("cookie")).get(
      cookieName(PROXY_COOKIE, auth.appUrl)
    ) ?? "";
  // A tab's new push stream ends its previous one with the same types
  // first, so a reconnect never waits for a slot the old one still holds.
  const client = request.headers.get(JMAP_PROXY_CLIENT_HEADER);
  const superseded = new AbortController();
  let forget: (() => void) | undefined = undefined;
  let slot: (() => void) | undefined = undefined;
  const release = (): void => {
    forget?.();
    slot?.();
  };
  if (
    route.kind === "eventsource" &&
    client !== null &&
    JMAP_PROXY_CLIENT_ID.test(client)
  ) {
    forget = (deps.pushClients ?? PUSH_CLIENTS).claim(
      `${key}\u0000${client}\u0000${route.types}`,
      () => {
        superseded.abort();
        release();
      }
    );
  }
  slot = (deps.streams ?? STREAMS).acquire(key);
  if (slot === undefined) {
    forget?.();
    const busy = json({ error: "too-many-streams" }, 429);
    busy.headers.set("retry-after", "5");
    return await refuse(busy);
  }
  try {
    return releasingWhenDone(
      await withRefresh({ ...context, superseded: superseded.signal }),
      release
    );
  } catch (error) {
    release();
    throw error;
  }
};
