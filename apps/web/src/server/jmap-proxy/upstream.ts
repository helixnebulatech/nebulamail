// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The proxy's requests to the JMAP server
// (§8): only to the configured server, never following a
// redirect (the session discovery's only within the server), never with
// cookies, and the session read with a size limit.

import type { Upstream } from "./config";
import { ProxyError } from "./errors";
import type { UpstreamEndpoints } from "./session";
import { upstreamEndpoints } from "./session";

export interface UpstreamDeps {
  /** The plain fetch; the proxy always asks it not to follow redirects. */
  readonly fetch: typeof fetch;
  readonly now: () => number;
}

const MAX_SESSION_BYTES = 1024 * 1024;
const MAX_SESSION_REDIRECTS = 3;
/** The endpoints are the server's, not the user's: kept a while. */
const ENDPOINTS_TTL_MS = 10 * 60 * 1000;

const isRedirect = (status: number): boolean => status >= 300 && status < 400;

/** One upstream request: never follows redirects, never sends cookies. */
export const send = async (
  deps: UpstreamDeps,
  url: URL,
  init: RequestInit
): Promise<Response> => {
  const response = await deps.fetch(url, {
    ...init,
    credentials: "omit",
    redirect: "manual",
  });
  if (isRedirect(response.status) || response.type === "opaqueredirect") {
    await response.body?.cancel();
    throw new ProxyError("redirect", "the JMAP server answered a redirect");
  }
  return response;
};

/** A body as UTF-8 text, refusing more than `max` bytes or bad UTF-8. */
const readText = async (
  body: ReadableStream<Uint8Array> | null,
  max: number
): Promise<string> => {
  if (body === null) {
    return "";
  }
  const chunks: Uint8Array[] = [];
  let total = 0;
  for await (const chunk of body) {
    total += chunk.byteLength;
    if (total > max) {
      throw new ProxyError("session", "session: too large");
    }
    chunks.push(chunk);
  }
  const joined = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    joined.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder("utf-8", { fatal: true }).decode(joined);
};

type Fetched =
  | { readonly ok: true; readonly session: unknown; readonly url: URL }
  | { readonly ok: false; readonly status: number };

/** Where a discovery redirect points, if it stays on the configured server. */
const redirectTarget = (
  location: string,
  from: URL,
  upstream: Upstream
): URL => {
  const next = new URL(location, from);
  if (
    ![upstream.origin, upstream.publicOrigin].includes(next.origin) ||
    next.username !== "" ||
    next.password !== ""
  ) {
    throw new ProxyError("redirect", "session redirect to another server");
  }
  return new URL(next.pathname + next.search, upstream.origin);
};

/**
 * The session document from the configured server. Its discovery may
 * redirect (`/.well-known/jmap` → the session resource), only within it.
 */
const fetchSession = async (
  deps: UpstreamDeps,
  upstream: Upstream,
  token: string,
  signal: AbortSignal
): Promise<Fetched> => {
  let url = new URL("/.well-known/jmap", upstream.origin);
  for (let hop = 0; hop <= MAX_SESSION_REDIRECTS; hop += 1) {
    // Each hop depends on the one before.
    // oxlint-disable-next-line no-await-in-loop
    const response = await deps.fetch(url, {
      credentials: "omit",
      headers: {
        accept: "application/json",
        "accept-encoding": "identity",
        authorization: `Bearer ${token}`,
      },
      redirect: "manual",
      signal,
    });
    const location = response.headers.get("location");
    if (isRedirect(response.status) && location !== null) {
      // oxlint-disable-next-line no-await-in-loop
      await response.body?.cancel();
      url = redirectTarget(location, url, upstream);
      continue;
    }
    if (!response.ok) {
      // oxlint-disable-next-line no-await-in-loop
      await response.body?.cancel();
      return { ok: false, status: response.status };
    }
    // oxlint-disable-next-line no-await-in-loop
    const text = await readText(response.body, MAX_SESSION_BYTES);
    return { ok: true, session: JSON.parse(text) as unknown, url };
  }
  throw new ProxyError("redirect", "session: too many redirects");
};

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** Per process: the JMAP server's endpoints, from the last session read. */
const cache: { value: UpstreamEndpoints | undefined; at: number } = {
  at: 0,
  value: undefined,
};

/** For tests: forget the endpoints. */
export const resetEndpointCache = (): void => {
  cache.value = undefined;
  cache.at = 0;
};

/** The biggest API request, once known (else undefined). */
export const knownMaxSizeRequest = (): number | undefined =>
  cache.value?.maxSizeRequest;

export type SessionRead =
  | {
      readonly ok: true;
      readonly session: Record<string, unknown>;
      readonly endpoints: UpstreamEndpoints;
    }
  | { readonly ok: false; readonly status: number };

/** The session, checked (session.ts); also refreshes the endpoint cache. */
export const readSession = async (
  deps: UpstreamDeps,
  upstream: Upstream,
  token: string,
  signal: AbortSignal
): Promise<SessionRead> => {
  const fetched = await fetchSession(deps, upstream, token, signal);
  if (!fetched.ok) {
    return fetched;
  }
  const { session, url } = fetched;
  const endpoints = upstreamEndpoints(session, url, upstream);
  if (!isObject(session)) {
    throw new ProxyError("session", "session: not an object");
  }
  cache.value = endpoints;
  cache.at = deps.now();
  return { endpoints, ok: true, session };
};

/** The endpoints (cached, or from a fresh session), or the error status. */
export const endpointsFor = async (
  deps: UpstreamDeps,
  upstream: Upstream,
  token: string,
  signal: AbortSignal
): Promise<UpstreamEndpoints | number> => {
  if (cache.value !== undefined && deps.now() - cache.at < ENDPOINTS_TTL_MS) {
    return cache.value;
  }
  const read = await readSession(deps, upstream, token, signal);
  return read.ok ? read.endpoints : read.status;
};
