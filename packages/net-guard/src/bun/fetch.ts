// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Bun adapter: fetch a public URL with the
// connection pinned to the validated IP, using Bun's own `fetch`.
//
// Bun replaces `undici` with a stub, so the Node adapter's pinned connector
// does not work there (every fetch fails closed with `network`). Bun's
// `fetch` has a documented way to connect to a given address instead
// ("Connecting to a specific address" in its docs), checked:
//
// - The full guard runs on every hop (`guardUrl`: one resolution, every
//   A/AAAA answer must be public; IP literals validated by the parser).
// - We ask for `http(s)://<validated IP>:<port>/path`, so Bun never resolves
//   the name itself: no DNS rebinding window. IPv6 goes in brackets.
// - `Host` carries the original authority, and `tls.serverName` the original
//   name: it is the SNI AND the name the certificate is checked against.
// - `tls.rejectUnauthorized: true` always: Bun would otherwise honour
//   `NODE_TLS_REJECT_UNAUTHORIZED=0` from the environment (checked).
// - `proxy: false`: Bun otherwise sends every fetch through `HTTP_PROXY`,
//   `HTTPS_PROXY` or `ALL_PROXY` (`proxy: ""` does not work).
// - `redirect: "manual"`: every Location goes back through the guard,
//   resolved against the URL with the name, never against the IP.
// - `decompress: false`: we decode ourselves with a limit (bombs).
// - `keepalive: false`: no connection is reused, so a socket verified for
//   one name can never carry a request for another.
// - Bun does not try other addresses: we walk the validated list ourselves.
//   A GET moves on after any connection or TLS error; a POST only after
//   errors that happen before the request is sent, so it is never repeated.
// - DNS: c-ares (`cancelableResolver`), the same as Node: an abort or the
//   deadline cancels the lookup; nothing is left running.
// - No separate connect timeout: the 10 s total deadline covers it.
// - Never `protocol: "http3"` (it cannot use `serverName`).
//
// `Bun.fetch` is used, not `globalThis.fetch`, so a library that patches the
// global (and might drop Bun's options) cannot weaken this.
//
// Fails closed: outside Bun, or on a Bun older than the one the test suite
// covers in CI, creating the fetch throws.
//
// This module never logs. Errors are reduced to a `BlockReason` so no URL,
// address or header leaks into logs through exception messages.

import { GuardError } from "../errors";
import {
  budgetCharge,
  checkContentType,
  IMAGE_ACCEPT,
  REDIRECT_STATUSES,
  USER_AGENT,
} from "../fetch-types";
import type { FetchLimits, FetchRequest, GuardedFetch } from "../fetch-types";
import { DROPPED_HEADERS, decoderFor, readBody } from "../http/body";
import { guardedFetchFromHops } from "../http/hops";
import type { Dial, GuardedFetchOptions, Hop, HopRequest } from "../http/hops";
import { checkImage } from "../image";
import type { ImageType } from "../image";
import type { ValidatedTarget } from "../resolve";
import type { BlockReason } from "../types";

/**
 * Oldest Bun this adapter runs on: the version CI tests it with
 * (`packageManager` in the root package.json). Older ones are refused.
 */
export const MIN_BUN_VERSION = "1.4.3";

/** The Bun-only `fetch` options we rely on (no Bun types in this package). */
interface BunRequestInit {
  readonly method: "GET" | "POST";
  readonly headers: Readonly<Record<string, string>>;
  readonly body?: string;
  readonly signal: AbortSignal;
  readonly redirect: "manual";
  readonly proxy: false;
  readonly decompress: false;
  readonly keepalive: false;
  readonly tls: {
    readonly rejectUnauthorized: true;
    readonly serverName?: string;
    readonly ca?: string;
  };
}

type BunFetch = (url: string, init: BunRequestInit) => Promise<Response>;

/** Test-only hooks (see `testing.ts`): where to connect, and a test CA. */
export interface BunTestHooks {
  readonly dial: Dial;
  /** PEM of a test CA, for local TLS servers. */
  readonly ca?: string;
}

export const isBun = (): boolean =>
  typeof process !== "undefined" && process.versions.bun !== undefined;

const parseVersion = (v: string): number[] | undefined => {
  const groups = /^(?<major>\d+)\.(?<minor>\d+)\.(?<patch>\d+)/u.exec(
    v
  )?.groups;
  return groups === undefined
    ? undefined
    : [groups.major, groups.minor, groups.patch].map(Number);
};

/** `a.b.c` ≥ `MIN_BUN_VERSION`; anything unparseable is too old. */
export const supportedBunVersion = (version: string | undefined): boolean => {
  const have = version === undefined ? undefined : parseVersion(version);
  const need = parseVersion(MIN_BUN_VERSION) ?? [];
  if (have === undefined) {
    return false;
  }
  for (const [i, wanted] of need.entries()) {
    const got = have[i] ?? 0;
    if (got !== wanted) {
      return got > wanted;
    }
  }
  return true;
};

/** `Bun.fetch`, looked up without Bun's types. */
const bunFetch = (): BunFetch => {
  const bun: unknown = Reflect.get(globalThis, "Bun");
  const candidate: unknown =
    typeof bun === "object" && bun !== null
      ? Reflect.get(bun, "fetch")
      : undefined;
  if (typeof candidate !== "function") {
    throw new TypeError("net-guard: Bun.fetch is not available");
  }
  return async (url, init) => {
    // `Bun.fetch` is the platform fetch; its result is a Response.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    const result = (await Reflect.apply(candidate, bun, [
      url,
      init,
    ])) as unknown;
    if (!(result instanceof Response)) {
      throw new GuardError("network");
    }
    return result;
  };
};

const errorCode = (error: unknown): string =>
  typeof error === "object" && error !== null && "code" in error
    ? String(error.code)
    : "";

/** Failures before anything is sent: refused, unreachable, or the TLS handshake. */
const CONNECT_FAILURES = new Set([
  "ConnectionRefused",
  "EADDRNOTAVAIL",
  "ECONNREFUSED",
  "EHOSTUNREACH",
  "ENETUNREACH",
  "FailedToOpenSocket",
]);
const TLS_FAILURE =
  /^(?:ERR_TLS_|ERR_SSL_|UNABLE_TO_|CERT_|DEPTH_ZERO_|SELF_SIGNED_|HOSTNAME_MISMATCH)/u;

/** Whether the next validated address may be tried after `error`. */
const canTryNext = (error: unknown, method: "GET" | "POST"): boolean => {
  if (error instanceof GuardError) {
    return false;
  }
  if (method === "GET") {
    return true;
  }
  const code = errorCode(error);
  return CONNECT_FAILURES.has(code) || TLS_FAILURE.test(code);
};

const reasonFor = (error: unknown, signal: AbortSignal): BlockReason => {
  if (error instanceof GuardError) {
    return error.reason;
  }
  return signal.aborted ? "timeout" : "network";
};

const cleanHeaders = (headers: Headers): Record<string, string> => {
  const out: Record<string, string> = {};
  for (const [name, value] of headers) {
    if (!DROPPED_HEADERS.has(name)) {
      out[name] = value;
    }
  }
  return out;
};

/** Close a body we will not read (and its connection). */
const discard = async (response: Response): Promise<void> => {
  try {
    await response.body?.cancel();
  } catch {
    // Already closed or errored: nothing to release.
  }
};

// A generator cannot be an arrow function.
// oxlint-disable-next-line func-style
async function* chunksOf(
  stream: ReadableStream<Uint8Array>
): AsyncGenerator<Uint8Array> {
  const reader = stream.getReader();
  try {
    for (;;) {
      // Chunks arrive in order; each read waits for the previous one.
      // oxlint-disable-next-line no-await-in-loop
      const { done, value } = await reader.read();
      if (done) {
        return;
      }
      yield value;
    }
  } finally {
    // Stopped early (size limit, abort): close the connection too.
    try {
      await reader.cancel();
    } catch {
      // Nothing left to release.
    }
  }
}

/** IPv6 addresses go in brackets in a URL. */
const urlHost = (host: string): string =>
  host.includes(":") ? `[${host}]` : host;

const readHop = async (
  response: Response,
  target: ValidatedTarget,
  request: FetchRequest,
  limits: FetchLimits,
  redirects: number
): Promise<Hop> => {
  const headers = cleanHeaders(response.headers);
  const location = response.headers.get("location");
  if (REDIRECT_STATUSES.has(response.status) && location !== null) {
    await discard(response);
    return { headers, kind: "redirect", location, status: response.status };
  }
  const empty = {
    body: new Uint8Array(0),
    headers,
    ok: true,
    redirects,
    status: response.status,
    url: target.href,
  } as const;
  if (
    response.status < 200 ||
    response.status > 299 ||
    request.readBody === false
  ) {
    await discard(response);
    return { kind: "done", result: empty };
  }
  const contentType = response.headers.get("content-type") ?? undefined;
  let decoder: ReturnType<typeof decoderFor>;
  try {
    checkContentType(contentType, request);
    const declared = Number(response.headers.get("content-length") ?? "");
    if (Number.isFinite(declared) && declared > limits.maxBodyBytes) {
      throw new GuardError("size");
    }
    decoder = decoderFor(response.headers.get("content-encoding") ?? undefined);
    if (decoder === "unsupported") {
      throw new GuardError("content-type");
    }
  } catch (error) {
    await discard(response);
    throw error;
  }
  const body =
    response.body === null
      ? new Uint8Array(0)
      : await readBody(
          chunksOf(response.body),
          decoder,
          limits.maxBodyBytes,
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

const fetchHop = async (
  fetch: BunFetch,
  hooks: BunTestHooks | undefined,
  target: ValidatedTarget,
  request: FetchRequest,
  hopRequest: HopRequest,
  limits: FetchLimits,
  signal: AbortSignal,
  redirects: number
): Promise<Hop> => {
  const { body: payload, method } = hopRequest;
  const serverName =
    target.scheme === "https:" && target.host.kind === "name"
      ? target.host.name
      : undefined;
  const init: BunRequestInit = {
    decompress: false,
    // Only generic headers: no cookies, no referrer, nothing from the user.
    headers: {
      accept:
        request.accept ?? (request.expect === "image" ? IMAGE_ACCEPT : "*/*"),
      "accept-encoding": "identity",
      host: target.authority,
      "user-agent": USER_AGENT,
      ...(payload === undefined ? {} : { "content-type": payload.type }),
    },
    keepalive: false,
    method,
    proxy: false,
    redirect: "manual",
    signal,
    tls: {
      rejectUnauthorized: true,
      ...(serverName === undefined ? {} : { serverName }),
      ...(hooks?.ca === undefined ? {} : { ca: hooks.ca }),
    },
    ...(payload === undefined ? {} : { body: payload.data }),
  };
  let lastError: unknown = new GuardError("network");
  // Try each validated address until one answers (all passed the guard).
  for (const address of target.addresses) {
    const where = hooks?.dial(address, target.port) ?? {
      host: address,
      port: target.port,
    };
    let response: Response;
    try {
      // One address at a time, in the order the guard validated them.
      // oxlint-disable-next-line no-await-in-loop
      response = await fetch(
        `${target.scheme}//${urlHost(where.host)}:${where.port}${target.path}`,
        init
      );
    } catch (error) {
      if (signal.aborted || !canTryNext(error, method)) {
        throw error;
      }
      lastError = error;
      continue;
    }
    // oxlint-disable-next-line no-await-in-loop
    return await readHop(response, target, request, limits, redirects);
  }
  throw lastError;
};

/**
 * Internal factory. `hooks` exist only for tests (see `testing.ts`) and are
 * refused outside Vitest, so even a deep import cannot use them in production.
 */
export const createBunFetchInternal = (
  options: GuardedFetchOptions,
  hooks: BunTestHooks | undefined
): GuardedFetch => {
  if (!isBun()) {
    throw new Error("net-guard: the Bun adapter only runs on Bun");
  }
  if (!supportedBunVersion(process.versions.bun)) {
    throw new Error(`net-guard: needs Bun ${MIN_BUN_VERSION} or newer`);
  }
  if (hooks !== undefined && process.env.VITEST !== "true") {
    throw new Error("net-guard: test hooks are only available in tests");
  }
  const fetch = bunFetch();
  return guardedFetchFromHops(
    options,
    async (...args) => await fetchHop(fetch, hooks, ...args),
    reasonFor
  );
};

/** Build a guarded fetch for Bun with a custom resolver, policy or tighter limits. */
export const createBunFetch = (
  options: GuardedFetchOptions = {}
): GuardedFetch => createBunFetchInternal(options, undefined);
