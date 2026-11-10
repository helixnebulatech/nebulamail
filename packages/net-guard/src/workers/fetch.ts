// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Guarded fetch for Cloudflare Workers. NOT PINNED.
//
// Workers' `fetch` takes a URL and resolves the name itself; there is no way
// to choose the IP it connects to. So each hop runs the full guard with a DoH
// resolver (A and AAAA, every address must be public) and only then calls
// `fetch` with the same URL. Between our lookup and Cloudflare's own there is
// a DNS rebinding window: an attacker's name can answer a public IP to us and
// another IP to Cloudflare. What limits the damage is the platform: Workers
// subrequests leave from Cloudflare's network, not from your servers, and
// Cloudflare refuses to connect to private or reserved addresses. This is the
// residual risk of this adapter. When it is not acceptable,
// run the proxy on Node (pinned connections).
//
// Everything else matches the Node adapter: schemes, ports, userinfo, IP
// literals, manual redirects with the guard on every hop (max 5), 5 MB body
// limit while streaming (decoded bytes, so compression bombs are cut too),
// 10 s total deadline, image type and magic bytes. GET only.

import { GuardError } from "../errors";
import {
  budgetCharge,
  checkContentType,
  hopLimits,
  IMAGE_ACCEPT,
  raceAbort,
  redirectAllowed,
  REDIRECT_STATUSES,
  resolveFetchLimits,
  USER_AGENT,
} from "../fetch-types";
import type {
  FetchLimits,
  FetchRequest,
  FetchResult,
  GuardedFetch,
} from "../fetch-types";
import { checkImage } from "../image";
import type { ImageType } from "../image";
import { guardUrl } from "../resolve";
import type { Resolver, ValidatedTarget } from "../resolve";
import type { BlockReason } from "../types";
import { PROXY_POLICY } from "../url";
import type { UrlPolicy } from "../url";
import { discard, readLimited } from "./stream";
import type { WebFetch } from "./stream";

export interface WorkersFetchOptions {
  /** Required: there is no system resolver in Workers. Use `createDohResolver`. */
  readonly resolver: Resolver;
  /** Defaults to `PROXY_POLICY` (http/https, ports 80/443). */
  readonly policy?: UrlPolicy;
  /**
   * Clamped to `LIMITS`. `connectTimeoutMs` cannot be applied (the platform
   * connects); the total deadline covers it.
   */
  readonly limits?: Partial<FetchLimits>;
  /** Default: the global `fetch`. */
  readonly fetch?: WebFetch;
}

/** Response headers we never pass on. */
const DROPPED_HEADERS = new Set([
  "connection",
  "content-encoding",
  "content-length",
  "keep-alive",
  "set-cookie",
  "set-cookie2",
  "transfer-encoding",
]);

const cleanHeaders = (headers: Headers): Record<string, string> => {
  const out: Record<string, string> = {};
  for (const [name, value] of headers) {
    if (!DROPPED_HEADERS.has(name)) {
      out[name] = value;
    }
  }
  return out;
};

const reasonFor = (error: unknown, signal: AbortSignal): BlockReason => {
  if (error instanceof GuardError) {
    return error.reason;
  }
  // `fetch` throws a TypeError for refused connections, TLS errors and
  // destinations Cloudflare will not connect to.
  return signal.aborted ? "timeout" : "network";
};

interface Context {
  readonly fetch: WebFetch;
  readonly limits: FetchLimits;
}

type Hop =
  | {
      readonly kind: "redirect";
      readonly location: string;
      readonly status: number;
      readonly headers: Readonly<Record<string, string>>;
    }
  | { readonly kind: "done"; readonly result: FetchResult };

const fetchHop = async (
  target: ValidatedTarget,
  request: FetchRequest,
  context: Context,
  signal: AbortSignal,
  redirects: number
): Promise<Hop> => {
  const response = await context.fetch(target.href, {
    // Only generic headers: no cookies, no referrer, nothing from the user.
    headers: {
      accept:
        request.accept ?? (request.expect === "image" ? IMAGE_ACCEPT : "*/*"),
      "accept-encoding": "identity",
      "user-agent": USER_AGENT,
    },
    method: "GET",
    redirect: "manual",
    signal,
  });
  const headers = cleanHeaders(response.headers);
  const location = response.headers.get("location");
  if (REDIRECT_STATUSES.has(response.status) && location !== null) {
    await discard(response);
    return { headers, kind: "redirect", location, status: response.status };
  }
  const empty: FetchResult = {
    body: new Uint8Array(0),
    headers,
    ok: true,
    redirects,
    status: response.status,
    url: target.href,
  };
  if (
    response.status < 200 ||
    response.status > 299 ||
    request.readBody === false
  ) {
    await discard(response);
    return { kind: "done", result: empty };
  }
  const contentType = response.headers.get("content-type") ?? undefined;
  try {
    checkContentType(contentType, request);
    const declared = Number(response.headers.get("content-length") ?? "");
    if (Number.isFinite(declared) && declared > context.limits.maxBodyBytes) {
      throw new GuardError("size");
    }
  } catch (error) {
    await discard(response);
    throw error;
  }
  const body = await readLimited(
    response,
    context.limits.maxBodyBytes,
    budgetCharge(request)
  );
  let imageType: ImageType | undefined;
  if (request.expect === "image") {
    const verdict = checkImage(contentType, body);
    if (!verdict.ok) {
      throw new GuardError(verdict.reason);
    }
    imageType = verdict.type;
  }
  return {
    kind: "done",
    result: {
      ...empty,
      body,
      ...(imageType === undefined ? {} : { imageType }),
    },
  };
};

const hostnameOf = (url: URL): string =>
  url.hostname.startsWith("[") && url.hostname.endsWith("]")
    ? url.hostname.slice(1, -1)
    : url.hostname;

const run = async (
  input: string | URL,
  request: FetchRequest,
  options: WorkersFetchOptions,
  context: Context,
  signal: AbortSignal
): Promise<FetchResult> => {
  const resolver: Resolver = async (hostname) =>
    await raceAbort(options.resolver(hostname), signal);
  const policy = options.policy ?? PROXY_POLICY;
  let current: string | URL = input;
  let firstHost: string | undefined;
  for (let redirects = 0; ; redirects += 1) {
    // Full guard on every hop. Hops are sequential by nature.
    // oxlint-disable-next-line no-await-in-loop
    const guard = await guardUrl(current, resolver, policy);
    if (!guard.ok) {
      return signal.aborted ? { ok: false, reason: "timeout" } : guard;
    }
    firstHost ??= guard.target.hostname;
    const limits = hopLimits(context.limits, request);
    if (typeof limits === "string") {
      return { ok: false, reason: limits };
    }
    // oxlint-disable-next-line no-await-in-loop
    const hop = await fetchHop(
      guard.target,
      request,
      { ...context, limits },
      signal,
      redirects
    );
    if (hop.kind === "done") {
      return hop.result;
    }
    const next = URL.parse(hop.location, guard.target.href);
    const mode = request.redirects ?? "follow";
    if (
      mode === "none" ||
      (mode === "same-host" &&
        (next === null || hostnameOf(next) !== firstHost))
    ) {
      return {
        body: new Uint8Array(0),
        headers: hop.headers,
        ok: true,
        redirects,
        status: hop.status,
        url: guard.target.href,
      };
    }
    if (redirects >= context.limits.maxRedirects) {
      return { ok: false, reason: "too-many-redirects" };
    }
    if (next === null) {
      return { ok: false, reason: "scheme" };
    }
    if (!redirectAllowed(request, next.href)) {
      return { ok: false, reason: "refused" };
    }
    current = next;
  }
};

/**
 * Guarded fetch for Workers. GET only: a POST (or a body) throws `TypeError`
 * before any lookup. See the file comment for what it cannot guarantee.
 */
export const createWorkersFetch = (
  options: WorkersFetchOptions
): GuardedFetch => {
  const context: Context = {
    fetch:
      options.fetch ??
      (async (input, init) => await globalThis.fetch(input, init)),
    limits: resolveFetchLimits(options.limits),
  };
  return async (input, request = {}) => {
    if ((request.method ?? "GET") !== "GET" || request.body !== undefined) {
      throw new TypeError("net-guard: the Workers fetch only supports GET");
    }
    const deadline = AbortSignal.timeout(context.limits.totalTimeoutMs);
    const signal =
      request.signal === undefined
        ? deadline
        : AbortSignal.any([deadline, request.signal]);
    try {
      return await run(input, request, options, context, signal);
    } catch (error) {
      return { ok: false, reason: reasonFor(error, signal) };
    }
  };
};
