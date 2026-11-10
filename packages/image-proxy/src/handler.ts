// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The proxy's HTTP surface, on standard Request/Response.
//
//   POST /batch      all images of one email at once (batch or cache mode)
//   GET  /i/<ticket> cache mode only: served from storage, NEVER fetched
//   GET  /p/<email token>/<sha256(url)>  preloaded image of an email with
//        an email token (static build, tickets/email-token.ts): storage only
//   GET  /healthz
//   POST /hooks/preload-urls  preload mode: image URLs and hashed recipients
//                         from the relay next to Stalwart (preload/url-hook.ts)
//   POST /hooks/stalwart  preload mode with `fullEmail` only: Stalwart's MTA
//                         Hook with the whole email (preload/hook.ts)
//   POST /preload/preferences  preload mode: the BFF's signed opt-out
//
// Why GET never fetches: if each image were downloaded when the browser asks
// for it, CSS like `:hover { background: url(...) }` in the email would tell
// the sender which rule fired. Downloads happen only in POST /batch, for the
// whole email at once.

import { isTrackerUrl } from "@nebulamail/mail-render/trackers";
import { checkImage, createConcurrencyLimiter } from "@nebulamail/net-guard";
import type { ConcurrencyLimiter } from "@nebulamail/net-guard";
import { z } from "zod";

import { toBase64 } from "./base64";
import { readLimited } from "./body";
import { keepForMs } from "./cache-policy";
import { checkOrigin, ProxyConfigError, resolveLimits } from "./config";
import type {
  ImageStorage,
  ProxyConfig,
  ProxyLimits,
  RequestContext,
  StoredImage,
} from "./config";
import { downloadAll } from "./download";
import type { BatchPlan, DownloadOutcome, ImageError } from "./download";
import { createPreloader, PRELOAD_ACCOUNT, preloadKey } from "./preload/hook";
import type { HookAnswer, Preloader } from "./preload/hook";
import { getRecord } from "./preload/records";
import { URL_HOOK_PATH } from "./preload/url-hook";
import { failOpen } from "./rate-limit";
import { storageKey } from "./storage-key";
import { EMAIL_TOKEN_SOURCE, verifyEmailToken } from "./tickets/email-token";
import { PREFERENCES_PATH } from "./tickets/preload-prefs";
import {
  hashUrl,
  MAX_TICKET_LENGTH,
  TICKET_PATTERN,
  verifyTicket,
} from "./tickets/ticket";
import type { TicketClaims } from "./tickets/ticket";

/** CSP for served images: a directly opened file can do nothing. */
export const IMAGE_CSP =
  "default-src 'none'; style-src 'unsafe-inline'; sandbox";
/** CSP for every other response (JSON, errors). */
const DOCUMENT_CSP = "default-src 'none'; frame-ancestors 'none'; sandbox";

export type BatchImageResult =
  | { readonly type: string; readonly data: string }
  | { readonly cached: true }
  | { readonly error: ImageError };

export interface BatchResponse {
  readonly images: Readonly<Record<string, BatchImageResult>>;
}

const BatchRequest = z
  .object({
    mode: z.enum(["batch", "cache"]).optional(),
    tickets: z.array(z.string().min(1).max(MAX_TICKET_LENGTH)).min(1),
  })
  .strict();

type Mode = "batch" | "cache";

// No parameter properties: Node's type stripping does not support them.
class HttpError extends Error {
  override readonly name = "HttpError";
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string) {
    super(code);
    this.status = status;
    this.code = code;
  }
}

const baseHeaders = (): Headers =>
  new Headers({
    "Content-Security-Policy": DOCUMENT_CSP,
    "Cross-Origin-Resource-Policy": "same-origin",
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
  });

const json = (status: number, body: unknown, extra?: Headers): Response => {
  const headers = baseHeaders();
  headers.set("Cache-Control", "no-store");
  headers.set("Content-Type", "application/json; charset=utf-8");
  for (const [name, value] of extra ?? []) {
    headers.set(name, value);
  }
  return Response.json(body, { headers, status });
};

const mediaType = (header: string | null): string =>
  (header ?? "").split(";", 1)[0]?.trim().toLowerCase() ?? "";

interface Core {
  readonly config: ProxyConfig;
  readonly limits: ProxyLimits;
  readonly appOrigin: string;
  readonly isTracker: (url: string) => boolean;
  readonly now: () => number;
  /** Cache mode: one download per storage key at a time across requests. */
  readonly inflight: Map<string, Promise<BatchImageResult>>;
  /** Downloads in flight across all requests. */
  readonly slots: ConcurrencyLimiter;
  readonly allowed: (key: string) => Promise<boolean>;
  readonly preloader: Preloader | undefined;
}

const limitOrThrow = async (core: Core, key: string): Promise<void> => {
  if (!(await core.allowed(key))) {
    throw new HttpError(429, "rate-limited");
  }
};

const corsHeaders = (core: Core): Headers =>
  new Headers({
    "Access-Control-Allow-Origin": core.appOrigin,
    Vary: "Origin",
  });

const isTrackerSafe = (core: Core, url: string): boolean => {
  try {
    return core.isTracker(url);
  } catch {
    // Fail closed: an error in the check is treated as a tracker.
    return true;
  }
};

/** Download plan for one email: limits, budget, slots and per-hop tracker check. */
const planFor = (core: Core, signal: AbortSignal): BatchPlan => ({
  allowRedirect: (url) => {
    if (isTrackerSafe(core, url)) {
      core.config.log?.({ event: "tracker-redirect" });
      return false;
    }
    return true;
  },
  concurrency: core.limits.concurrency,
  fetchImage: core.config.fetchImage,
  maxBytesPerEmail: core.limits.maxBytesPerEmail,
  maxBytesPerImage: core.limits.maxBytesPerImage,
  maxRequestsPerEmail: core.limits.maxRequestsPerEmail,
  signal,
  slots: core.slots,
  timeoutMs: core.limits.batchTimeoutMs,
});

const verifyAll = async (
  core: Core,
  tickets: readonly string[]
): Promise<Map<string, TicketClaims>> => {
  const unique = [...new Set(tickets)];
  const now = core.now();
  const results = await Promise.all(
    unique.map(
      async (ticket) => await verifyTicket(core.config.keys, ticket, now)
    )
  );
  const claims = new Map<string, TicketClaims>();
  for (const [index, result] of results.entries()) {
    const ticket = unique[index];
    if (!result.ok || ticket === undefined) {
      throw new HttpError(400, "invalid-ticket");
    }
    claims.set(ticket, result.claims);
  }
  const [first] = claims.values();
  for (const claim of claims.values()) {
    if (
      claim.accountId !== first?.accountId ||
      claim.emailId !== first.emailId
    ) {
      throw new HttpError(400, "mixed-email");
    }
  }
  return claims;
};

/**
 * Cache mode: keeps the image if the sender allows it (RFC 9111,
 * cache-policy.ts; the entry is this account's only, a private cache) and
 * answers `cached`; otherwise answers the image itself, like batch mode,
 * so it shows without being stored.
 */
const storeOne = async (
  core: Core,
  storage: ImageStorage,
  key: string,
  outcome: DownloadOutcome
): Promise<BatchImageResult> => {
  if (!outcome.ok) {
    return { error: outcome.error };
  }
  const ttlMs = keepForMs(outcome.image, "private", {
    fixedTtlMs: storage.fixedTtlMs,
    maxMs: core.limits.cacheTtlMs,
  });
  if (ttlMs === 0) {
    return {
      data: toBase64(outcome.image.bytes),
      type: outcome.image.type,
    };
  }
  try {
    await storage.put(key, outcome.image, ttlMs);
    return { cached: true };
  } catch {
    core.config.log?.({ event: "storage-error" });
    return { error: "storage" };
  }
};

const getCached = async (
  core: Core,
  storage: ImageStorage,
  key: string
): Promise<StoredImage | null> => {
  let stored: StoredImage | null;
  try {
    stored = await storage.get(key);
  } catch {
    core.config.log?.({ event: "storage-error" });
    return null;
  }
  if (stored === null) {
    return null;
  }
  const verdict = checkImage(stored.type, stored.bytes);
  if (!verdict.ok || stored.bytes.byteLength > core.limits.maxBytesPerImage) {
    core.config.log?.({ event: "corrupt-cache-entry" });
    return null;
  }
  return {
    bytes: stored.bytes,
    type: verdict.type,
    ...(stored.expiresAt === undefined ? {} : { expiresAt: stored.expiresAt }),
  };
};

/** The account's entry, else the one stored by `preload` for the same URL. */
const getCachedOrPreloaded = async (
  core: Core,
  storage: ImageStorage,
  key: string,
  url: string
): Promise<StoredImage | null> =>
  (await getCached(core, storage, key)) ??
  (core.preloader === undefined
    ? null
    : await getCached(core, storage, await preloadKey(url)));

interface Tally {
  fetched: number;
  fromCache: number;
  bytes: number;
}

/** Results per URL for batch mode: image data or an error. */
const runBatchMode = async (
  core: Core,
  urls: readonly string[],
  signal: AbortSignal,
  tally: Tally
): Promise<Map<string, BatchImageResult>> => {
  const outcomes = await downloadAll(urls, planFor(core, signal));
  const results = new Map<string, BatchImageResult>();
  for (const url of urls) {
    const outcome = outcomes.get(url) ?? { error: "timeout", ok: false };
    if (outcome.ok) {
      tally.fetched += 1;
      tally.bytes += outcome.image.bytes.byteLength;
      results.set(url, {
        data: toBase64(outcome.image.bytes),
        type: outcome.image.type,
      });
    } else {
      results.set(url, { error: outcome.error });
    }
  }
  return results;
};

/**
 * Cache mode: everything already stored is answered from storage without a
 * download; the rest is downloaded together (one batch) and stored. Keys
 * being downloaded by another request are awaited, not downloaded twice.
 */
const runCacheMode = async (
  core: Core,
  storage: ImageStorage,
  accountId: string,
  urls: readonly string[],
  signal: AbortSignal,
  tally: Tally
): Promise<Map<string, BatchImageResult>> => {
  const keyList = await Promise.all(
    urls.map(async (url) => await storageKey(accountId, await hashUrl(url)))
  );
  // Synchronous from here until the first await: claim every free key.
  const pending = new Map<string, Promise<BatchImageResult>>();
  const own: {
    readonly url: string;
    readonly key: string;
    readonly done: PromiseWithResolvers<BatchImageResult>;
  }[] = [];
  for (const [i, url] of urls.entries()) {
    const key = keyList[i] ?? "";
    const running = core.inflight.get(key);
    if (running === undefined) {
      const done = Promise.withResolvers<BatchImageResult>();
      core.inflight.set(key, done.promise);
      own.push({ done, key, url });
      pending.set(url, done.promise);
    } else {
      pending.set(url, running);
    }
  }

  try {
    const cached = await Promise.all(
      own.map(
        async ({ key, url }) =>
          await getCachedOrPreloaded(core, storage, key, url)
      )
    );
    const toFetch = own.filter((entry, i) => {
      if (cached[i] === null) {
        return true;
      }
      tally.fromCache += 1;
      entry.done.resolve({ cached: true });
      return false;
    });
    if (toFetch.length > 0) {
      const outcomes = await downloadAll(
        toFetch.map(({ url }) => url),
        planFor(core, signal)
      );
      await Promise.all(
        toFetch.map(async ({ url, key, done }) => {
          const outcome: DownloadOutcome = outcomes.get(url) ?? {
            error: "timeout",
            ok: false,
          };
          if (outcome.ok) {
            tally.fetched += 1;
            tally.bytes += outcome.image.bytes.byteLength;
          }
          done.resolve(await storeOne(core, storage, key, outcome));
        })
      );
    }
  } finally {
    for (const { key, done } of own) {
      // No-op for entries already resolved; never leaves a waiter hanging.
      done.resolve({ error: "storage" });
      core.inflight.delete(key);
    }
  }

  const results = new Map<string, BatchImageResult>();
  const settled = await Promise.all(
    [...pending].map(async ([url, done]) => [url, await done] as const)
  );
  for (const [url, result] of settled) {
    results.set(url, result);
  }
  return results;
};

interface ParsedBatch {
  readonly mode: Mode;
  readonly claims: ReadonlyMap<string, TicketClaims>;
  readonly accountId: string;
}

/** Everything before any download: body, limits, mode and tickets. */
const parseBatch = async (
  core: Core,
  request: Request,
  context: RequestContext
): Promise<ParsedBatch> => {
  if (mediaType(request.headers.get("content-type")) !== "application/json") {
    throw new HttpError(415, "expected-json");
  }
  if (context.clientAddress !== undefined) {
    await limitOrThrow(core, `batch:ip:${context.clientAddress}`);
  }
  const maxBody =
    core.limits.maxImagesPerEmail * (MAX_TICKET_LENGTH + 8) + 1024;
  const body = await readLimited(request, maxBody);
  if (!body.ok) {
    throw new HttpError(body.code === "body-too-large" ? 413 : 400, body.code);
  }
  let raw: unknown;
  try {
    raw = JSON.parse(body.text);
  } catch {
    throw new HttpError(400, "invalid-body");
  }
  const parsed = BatchRequest.safeParse(raw);
  if (!parsed.success) {
    throw new HttpError(400, "invalid-body");
  }
  const { tickets } = parsed.data;
  if (tickets.length > core.limits.maxImagesPerEmail) {
    throw new HttpError(400, "too-many-images");
  }
  const mode: Mode =
    parsed.data.mode ?? (core.config.storage === undefined ? "batch" : "cache");
  if (mode === "cache" && core.config.storage === undefined) {
    throw new HttpError(400, "cache-unavailable");
  }
  const claims = await verifyAll(core, tickets);
  const [first] = claims.values();
  if (first === undefined) {
    throw new HttpError(400, "invalid-body");
  }
  // Only valid tickets reach the shared buckets, so requests without
  // credentials cannot spend them. The account first: an account over its
  // own limit must not spend everybody's `batch:global`.
  await limitOrThrow(core, `batch:account:${first.accountId}`);
  await limitOrThrow(core, "batch:global");
  return { accountId: first.accountId, claims, mode };
};

/** Whole seconds from `now` until `until` (ms), at least 0. */
const browserMaxAge = (now: number, until: number): number =>
  Math.max(0, Math.floor((until - now) / 1000));

/** Results per URL: trackers refused, the rest downloaded (and stored). */
const resolveUrls = async (
  core: Core,
  batch: ParsedBatch,
  signal: AbortSignal,
  tally: Tally
): Promise<Map<string, BatchImageResult>> => {
  const byUrl = new Map<string, BatchImageResult>();
  const urls: string[] = [];
  for (const url of new Set([...batch.claims.values()].map((c) => c.url))) {
    if (isTrackerSafe(core, url)) {
      // A signed tracker means a leaked key or an outdated BFF list.
      core.config.log?.({ event: "tracker-in-ticket" });
      byUrl.set(url, { error: "tracker" });
    } else {
      urls.push(url);
    }
  }
  const { storage } = core.config;
  const results =
    batch.mode === "cache" && storage !== undefined
      ? await runCacheMode(core, storage, batch.accountId, urls, signal, tally)
      : await runBatchMode(core, urls, signal, tally);
  for (const [url, result] of results) {
    byUrl.set(url, result);
  }
  return byUrl;
};

const handleBatch = async (
  core: Core,
  request: Request,
  context: RequestContext
): Promise<Response> => {
  const cors = corsHeaders(core);
  let batch: ParsedBatch;
  try {
    batch = await parseBatch(core, request, context);
  } catch (error) {
    if (error instanceof HttpError) {
      core.config.log?.({
        event: "rejected",
        reason: error.code,
        status: error.status,
      });
      return json(error.status, { error: error.code }, cors);
    }
    throw error;
  }
  const tally: Tally = { bytes: 0, fetched: 0, fromCache: 0 };
  const byUrl = await resolveUrls(core, batch, request.signal, tally);
  const images: Record<string, BatchImageResult> = {};
  let failed = 0;
  for (const [ticket, claim] of batch.claims) {
    const result = byUrl.get(claim.url) ?? { error: "timeout" };
    if ("error" in result) {
      failed += 1;
    }
    images[ticket] = result;
  }
  core.config.log?.({
    bytes: tally.bytes,
    event: "batch",
    failed,
    fetched: tally.fetched,
    fromCache: tally.fromCache,
    images: batch.claims.size,
    mode: batch.mode,
  });
  return json(200, { images } satisfies BatchResponse, cors);
};

const handlePreflight = (core: Core, request: Request): Response => {
  const method = request.headers.get("access-control-request-method");
  const requested = (
    request.headers.get("access-control-request-headers") ?? ""
  )
    .split(",")
    .map((h) => h.trim().toLowerCase())
    .filter((h) => h !== "");
  if (method !== "POST" || requested.some((h) => h !== "content-type")) {
    return json(403, { error: "preflight-refused" });
  }
  const headers = baseHeaders();
  for (const [name, value] of corsHeaders(core)) {
    headers.set(name, value);
  }
  headers.set("Access-Control-Allow-Methods", "POST");
  headers.set("Access-Control-Allow-Headers", "content-type");
  headers.set("Access-Control-Max-Age", "600");
  headers.set("Cache-Control", "no-store");
  return new Response(null, { headers, status: 204 });
};

const handleImage = async (
  core: Core,
  token: string,
  context: RequestContext
): Promise<Response> => {
  const { storage } = core.config;
  if (storage === undefined) {
    return json(404, { error: "not-found" });
  }
  if (
    context.clientAddress !== undefined &&
    !(await core.allowed(`image:ip:${context.clientAddress}`))
  ) {
    return json(429, { error: "rate-limited" });
  }
  const now = core.now();
  const verdict = await verifyTicket(core.config.keys, token, now);
  if (!verdict.ok) {
    return json(403, { error: "invalid-ticket" });
  }
  const { claims } = verdict;
  if (isTrackerSafe(core, claims.url)) {
    core.config.log?.({ event: "tracker-in-ticket" });
    return json(403, { error: "tracker" });
  }
  // After the ticket: requests without credentials cannot spend it.
  if (!(await core.allowed("image:global"))) {
    return json(429, { error: "rate-limited" });
  }
  // Storage only. There is deliberately no fetch on this path.
  const key = await storageKey(claims.accountId, await hashUrl(claims.url));
  const image = await getCachedOrPreloaded(core, storage, key, claims.url);
  if (image === null) {
    return json(404, { error: "not-found" });
  }
  // The browser keeps it no longer than the ticket, nor than our entry
  // (which follows the sender's Cache-Control, RFC 9111).
  const maxAge = browserMaxAge(
    now,
    Math.min(claims.expiresAt, image.expiresAt ?? Number.POSITIVE_INFINITY)
  );
  const headers = new Headers({
    "Cache-Control": `private, max-age=${maxAge}, immutable`,
    "Content-Length": String(image.bytes.byteLength),
    "Content-Security-Policy": IMAGE_CSP,
    "Content-Type": image.type,
    "Cross-Origin-Resource-Policy": "cross-origin",
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
  });
  return new Response(new Uint8Array(image.bytes), { headers, status: 200 });
};

/** Browsers may keep a preloaded image this long (it never changes). */
const PRELOADED_MAX_AGE_S = 24 * 60 * 60;

/**
 * `GET /p/<token>/<hash>`: one preloaded image of an email with an email
 * token. The token must be authentic and the image one of that email's;
 * served from storage only, never downloaded.
 */
const handlePreloaded = async (
  core: Core,
  token: string,
  urlHash: string,
  context: RequestContext
): Promise<Response> => {
  const { storage } = core.config;
  if (storage === undefined || core.preloader === undefined) {
    return json(404, { error: "not-found" });
  }
  if (
    context.clientAddress !== undefined &&
    !(await core.allowed(`image:ip:${context.clientAddress}`))
  ) {
    return json(429, { error: "rate-limited" });
  }
  const id = await verifyEmailToken(core.config.keys, token);
  if (id === null) {
    return json(403, { error: "invalid-ticket" });
  }
  // After the token: requests without credentials cannot spend it.
  if (!(await core.allowed("image:global"))) {
    return json(429, { error: "rate-limited" });
  }
  let images: ReadonlySet<string> | null;
  try {
    images = await getRecord(storage, id);
  } catch {
    core.config.log?.({ event: "storage-error" });
    images = null;
  }
  if (images?.has(urlHash) !== true) {
    return json(404, { error: "not-found" });
  }
  const image = await getCached(
    core,
    storage,
    await storageKey(PRELOAD_ACCOUNT, urlHash)
  );
  if (image === null) {
    return json(404, { error: "not-found" });
  }
  const maxAge = Math.min(
    PRELOADED_MAX_AGE_S,
    image.expiresAt === undefined
      ? PRELOADED_MAX_AGE_S
      : browserMaxAge(core.now(), image.expiresAt)
  );
  const headers = new Headers({
    "Cache-Control": `private, max-age=${maxAge}, immutable`,
    "Content-Length": String(image.bytes.byteLength),
    "Content-Security-Policy": IMAGE_CSP,
    "Content-Type": image.type,
    "Cross-Origin-Resource-Policy": "cross-origin",
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
  });
  return new Response(new Uint8Array(image.bytes), { headers, status: 200 });
};

const modesOf = (core: Core): string[] => {
  if (core.config.storage === undefined) {
    return ["batch"];
  }
  return core.preloader === undefined
    ? ["batch", "cache"]
    : ["batch", "cache", "preload"];
};

const IMAGE_PATH = /^\/i\/(?<token>[^/]+)$/u;
// Named groups of the token itself are dropped: `token` is the whole of it.
const PRELOADED_PATH = new RegExp(
  `^/p/(?<token>${EMAIL_TOKEN_SOURCE.replaceAll(/\(\?<\w+>/gu, "(?:")})/(?<hash>[0-9a-f]{64})$`,
  "u"
);

/** GET routes: health and cached images. */
const routeGet = async (
  core: Core,
  pathname: string,
  context: RequestContext
): Promise<Response> => {
  if (pathname === "/healthz") {
    return json(200, { modes: modesOf(core), status: "ok" });
  }
  const preloaded = PRELOADED_PATH.exec(pathname)?.groups;
  if (preloaded?.token !== undefined && preloaded.hash !== undefined) {
    return await handlePreloaded(
      core,
      preloaded.token,
      preloaded.hash,
      context
    );
  }
  const token = IMAGE_PATH.exec(pathname)?.groups?.token;
  if (
    token !== undefined &&
    token.length <= MAX_TICKET_LENGTH &&
    TICKET_PATTERN.test(token)
  ) {
    return await handleImage(core, token, context);
  }
  return json(404, { error: "not-found" });
};

/** The preload POST routes; null when `pathname` is none of them. */
const routePreload = async (
  preloader: Preloader,
  pathname: string,
  request: Request,
  context: RequestContext
): Promise<HookAnswer | null> => {
  if (pathname === URL_HOOK_PATH) {
    return await preloader.handleUrls(request, context);
  }
  if (pathname === "/hooks/stalwart" && preloader.fullEmail) {
    return await preloader.handle(request, context);
  }
  if (pathname === PREFERENCES_PATH) {
    // Server to server (the BFF), signed: no CORS, no Origin needed.
    return await preloader.handlePreferences(request, context);
  }
  return null;
};

const route = async (
  core: Core,
  request: Request,
  context: RequestContext
): Promise<Response> => {
  const { pathname } = new URL(request.url);
  const { method } = request;
  if (method !== "GET" && method !== "POST" && method !== "OPTIONS") {
    return json(
      405,
      { error: "method-not-allowed" },
      new Headers({ Allow: "GET, POST, OPTIONS" })
    );
  }
  if (pathname === "/batch" && (method === "POST" || method === "OPTIONS")) {
    // Exact match only. No Origin (not a browser CORS request) is refused too.
    if (request.headers.get("origin") !== core.appOrigin) {
      core.config.log?.({ event: "rejected", reason: "origin", status: 403 });
      return json(403, { error: "origin-not-allowed" });
    }
    return method === "POST"
      ? await handleBatch(core, request, context)
      : handlePreflight(core, request);
  }
  if (method === "POST" && core.preloader !== undefined) {
    const answer = await routePreload(
      core.preloader,
      pathname,
      request,
      context
    );
    if (answer !== null) {
      return json(answer.status, answer.body);
    }
  }
  if (method === "GET") {
    return await routeGet(core, pathname, context);
  }
  return json(404, { error: "not-found" });
};

export interface ProxyHandler {
  (request: Request, context?: RequestContext): Promise<Response>;
  /**
   * With preload: drop expired quota and reference records (never the
   * opt-outs). Run it with the storage's sweep. Returns records deleted.
   */
  readonly sweepPreload: (() => Promise<number>) | undefined;
}

/** Build the proxy's request handler. Throws `ProxyConfigError` on bad config. */
export const createProxyHandler = (config: ProxyConfig): ProxyHandler => {
  const limits = resolveLimits(config.limits);
  const isTracker = config.isTracker ?? ((url) => isTrackerUrl(url) !== null);
  const slots =
    config.downloadSlots ??
    createConcurrencyLimiter({
      max: limits.globalConcurrency,
      maxQueue: limits.globalConcurrency * 32,
    });
  const allowed =
    config.rateLimit === undefined
      ? async () => await Promise.resolve(true)
      : failOpen(config.rateLimit);
  if (config.preload !== undefined && config.storage === undefined) {
    throw new ProxyConfigError("image proxy: preload needs a storage");
  }
  const preloader =
    config.preload === undefined || config.storage === undefined
      ? undefined
      : createPreloader({
          allowed,
          config: config.preload,
          fetchImage: config.fetchImage,
          isTracker: (url) => {
            try {
              return isTracker(url);
            } catch {
              return true;
            }
          },
          keys: config.keys,
          limits,
          log: (event) => {
            config.log?.(event);
          },
          now: config.now ?? Date.now,
          slots,
          storage: config.storage,
        });
  const core: Core = {
    allowed,
    appOrigin: checkOrigin(config.appOrigin),
    config,
    inflight: new Map(),
    isTracker,
    limits,
    now: config.now ?? Date.now,
    preloader,
    slots,
  };

  const handle = async (
    request: Request,
    context: RequestContext = {}
  ): Promise<Response> => await route(core, request, context);
  return Object.assign(handle, { sweepPreload: preloader?.sweep });
};
