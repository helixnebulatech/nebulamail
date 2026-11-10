// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// The redirect loop shared by the pinned adapters (Node and Bun): the full
// guard on every hop, the caller's budget and `allowRedirect`,
// redirect modes, POST → GET after 301/302/303, and the total deadline. Each
// adapter only supplies how ONE hop is fetched from a `ValidatedTarget`.

import type { PublicAddress } from "../address";
import {
  hopLimits,
  raceAbort,
  redirectAllowed,
  resolveFetchLimits,
} from "../fetch-types";
import type {
  FetchLimits,
  FetchRequest,
  FetchResult,
  GuardedFetch,
  RequestBody,
} from "../fetch-types";
import { guardUrl } from "../resolve";
import type { Resolver, ValidatedTarget } from "../resolve";
import type { BlockReason } from "../types";
import { PROXY_POLICY } from "../url";
import type { UrlPolicy } from "../url";
import { cancelableResolver, hasCancelableResolver } from "./resolver";

export interface GuardedFetchOptions {
  /**
   * Defaults to c-ares (`cancelableResolver`) in Node, Deno and Bun:
   * cancelable, A and AAAA independent. Every answer is still validated.
   */
  readonly resolver?: Resolver;
  /** Defaults to `PROXY_POLICY` (http/https, ports 80/443). */
  readonly policy?: UrlPolicy;
  /** Each value is clamped to `LIMITS`: options can tighten limits, never loosen them. */
  readonly limits?: Partial<FetchLimits>;
}

/**
 * Test-only: where to really open the TCP connection for a validated
 * address. The guard has already run; tests use it to reach local servers.
 */
export type Dial = (
  address: PublicAddress,
  port: number
) => { readonly host: string; readonly port: number };

/** Our request bodies are tiny (a form field); refuse anything bigger. */
export const MAX_REQUEST_BODY_BYTES = 16 * 1024;

/** `Content-Type` of a request body: a media type and simple parameters, no line breaks. */
const BODY_TYPE = /^[\w!#$&^.+-]+\/[\w!#$&^.+-]+(?:\s*;\s*[\w-]+=[\w-]+)*$/u;

/** Method and body of one hop (a redirect may turn a POST into a GET). */
export interface HopRequest {
  readonly method: "GET" | "POST";
  readonly body: RequestBody | undefined;
}

export type Hop =
  | {
      readonly kind: "redirect";
      readonly location: string;
      readonly status: number;
      readonly headers: Readonly<Record<string, string>>;
    }
  | { readonly kind: "done"; readonly result: FetchResult };

/** Fetch one hop from an already validated target. Throws on failure. */
export type HopFetcher = (
  target: ValidatedTarget,
  request: FetchRequest,
  hopRequest: HopRequest,
  limits: FetchLimits,
  signal: AbortSignal,
  redirects: number
) => Promise<Hop>;

/**
 * Method after a redirect, as browsers do (Fetch standard, "HTTP-redirect
 * fetch"): 303, and 301/302 after a POST, continue as a GET without body;
 * 307/308 repeat the same method and body.
 */
const afterRedirect = (hop: HopRequest, status: number): HopRequest =>
  status === 303 ||
  ((status === 301 || status === 302) && hop.method === "POST")
    ? { body: undefined, method: "GET" }
    : hop;

/** Hostname as `ParsedTarget.hostname` has it (IPv6 without brackets). */
const hostnameOf = (url: URL): string =>
  url.hostname.startsWith("[") && url.hostname.endsWith("]")
    ? url.hostname.slice(1, -1)
    : url.hostname;

/** GET without body, or POST with a small, well-formed one. */
const validRequest = (request: FetchRequest): boolean => {
  const method = request.method ?? "GET";
  const { body } = request;
  if (method === "GET") {
    return body === undefined;
  }
  return (
    method === "POST" &&
    body !== undefined &&
    BODY_TYPE.test(body.type) &&
    new TextEncoder().encode(body.data).byteLength <= MAX_REQUEST_BODY_BYTES
  );
};

interface Context {
  readonly resolver: Resolver;
  readonly policy: UrlPolicy;
  readonly limits: FetchLimits;
  readonly fetchHop: HopFetcher;
  readonly reasonFor: (error: unknown, signal: AbortSignal) => BlockReason;
}

const run = async (
  input: string | URL,
  request: FetchRequest,
  context: Context,
  signal: AbortSignal
): Promise<FetchResult> => {
  // The signal lets the resolver cancel its lookup; the race still cuts
  // off one that ignores it.
  const resolver: Resolver = async (hostname) =>
    await raceAbort(context.resolver(hostname, signal), signal);
  let current: string | URL = input;
  let hopRequest: HopRequest = {
    body: request.body,
    method: request.method ?? "GET",
  };
  let firstHost: string | undefined;
  for (let redirects = 0; ; redirects += 1) {
    // Full guard on every hop.
    // Hops are sequential by nature: each depends on the previous Location.
    // oxlint-disable-next-line no-await-in-loop
    const guard = await guardUrl(current, resolver, context.policy);
    if (!guard.ok) {
      // A resolver cut off by the deadline surfaces as "dns" from the core.
      return signal.aborted ? { ok: false, reason: "timeout" } : guard;
    }
    firstHost ??= guard.target.hostname;
    const limits = hopLimits(context.limits, request);
    if (typeof limits === "string") {
      return { ok: false, reason: limits };
    }
    // The body cap came from the budget: going over it means the email's
    // bytes are spent. The transport's own size cut (undici's
    // `maxResponseSize`) may stop the read before a chunk is charged.
    const { budget } = request;
    const capped =
      budget !== undefined && limits.maxBodyBytes >= budget.remainingBytes();
    let hop: Hop;
    try {
      // oxlint-disable-next-line no-await-in-loop
      hop = await context.fetchHop(
        guard.target,
        request,
        hopRequest,
        limits,
        signal,
        redirects
      );
    } catch (error) {
      if (capped && context.reasonFor(error, signal) === "size") {
        // More than is left never fits: this exhausts the budget.
        budget.takeBytes(Number.MAX_SAFE_INTEGER);
      }
      throw error;
    }
    if (hop.kind === "done") {
      return hop.result;
    }
    // Relative to the URL with the NAME, never to the IP we dialed.
    const next = URL.parse(hop.location, guard.target.href);
    const mode = request.redirects ?? "follow";
    if (
      mode === "none" ||
      (mode === "same-host" &&
        (next === null || hostnameOf(next) !== firstHost))
    ) {
      // Not followed: the caller sees the redirect status, never its target.
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
    hopRequest = afterRedirect(hopRequest, hop.status);
    current = next;
  }
};

/**
 * A `GuardedFetch` from one adapter's hop: deadline, request validation, the
 * redirect loop, and every error reduced to a reason by `reasonFor`.
 *
 * The default resolver is decided here, once for every pinned adapter:
 * c-ares (`cancelableResolver`), so an abort or the deadline cancels the
 * lookup and its retries. Fails closed on a runtime without `Resolver#cancel`
 * unless the caller brings its own resolver.
 */
export const guardedFetchFromHops = (
  options: GuardedFetchOptions,
  fetchHop: HopFetcher,
  reasonFor: (error: unknown, signal: AbortSignal) => BlockReason
): GuardedFetch => {
  if (options.resolver === undefined && !hasCancelableResolver()) {
    throw new Error("net-guard: this runtime has no cancelable DNS resolver");
  }
  const context: Context = {
    fetchHop,
    limits: resolveFetchLimits(options.limits),
    policy: options.policy ?? PROXY_POLICY,
    reasonFor,
    resolver: options.resolver ?? cancelableResolver,
  };
  return async (input, request = {}) => {
    const deadline = AbortSignal.timeout(context.limits.totalTimeoutMs);
    const signal =
      request.signal === undefined
        ? deadline
        : AbortSignal.any([deadline, request.signal]);
    // A programming error, not a policy decision: fail loudly.
    if (!validRequest(request)) {
      throw new TypeError("net-guard: invalid method or request body");
    }
    try {
      return await run(input, request, context, signal);
    } catch (error) {
      return { ok: false, reason: reasonFor(error, signal) };
    }
  };
};
