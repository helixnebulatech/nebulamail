// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Vercel Functions adapter, Node.js or Bun runtime. It uses
// net-guard's pinned fetch (`createGuardedFetch` picks the Bun adapter under
// Bun), so every connection is pinned to the validated IP, exactly like the
// Node server. The Edge runtime (no DNS control, no pinning) is refused at
// startup.
//
// Routes are the core's (`/batch`, `/i/<ticket>`, `/healthz`) plus
// `GET /cron/sweep` for Vercel Cron (Blob retention). `vercel.json` rewrites
// every path to the single function and passes the original path in
// `?__path=`, which this adapter reads back.

import { timingSafeEqual } from "node:crypto";

import { createGuardedFetch } from "@nebulamail/net-guard/node";
import type { GuardedFetch } from "@nebulamail/net-guard/node";

import type {
  ImageUrlExtractor,
  PreloadConfig,
  ProxyLimits,
  ProxyLogEvent,
  RequestContext,
  SweepableStorage,
} from "../config";
import { ProxyConfigError, resolveLimits } from "../config";
import { EnvError } from "../env";
import { fetchImageWith } from "../fetch-image";
import { createProxyHandler } from "../handler";
import { FULL_EMAIL_WARNING, preloadOptions } from "../preload-env";
import type { PreloadStateStore } from "../preload/state";
import { clientKey, createRateLimiter } from "../rate-limit";
import type { RateLimitRules } from "../rate-limit";
import { createS3StateStore, createS3Storage } from "../s3/s3-storage";
import { parseTicketKeys } from "../tickets/ticket";
import { createBlobStateStore } from "./blob-state";
import { createBlobStorage } from "./blob-storage";
import type { VercelBlobApi } from "./blob-storage";
import { parseVercelEnv } from "./env";
import type { VercelEnv } from "./env";

/**
 * Vercel Functions answer at most 4.5 MB per response body. A batch answer
 * carries the images in base64 (+33 %), so 3 MB of images per email keeps it
 * under the limit. A single cached image is at most 3 MB too.
 */
export const VERCEL_LIMITS: Partial<ProxyLimits> = Object.freeze({
  maxBytesPerEmail: 3 * 1024 * 1024,
  maxBytesPerImage: 3 * 1024 * 1024,
});

export interface VercelProxyOptions {
  /** The `@vercel/blob` module; required when `IMAGE_CACHE_STORAGE=vercel-blob`. */
  readonly blob?: VercelBlobApi;
  /**
   * Image URL extraction for the full-email hook
   * (`IMAGE_PRELOAD_FULL_EMAIL=true`): `createWorkerExtractor` from
   * `@nebulamail/mail-render/server-extract`, with `workerUrl` pointing to the
   * bundled worker (README). Parses in a worker thread with a deadline.
   * Not needed with the relay.
   */
  readonly htmlExtractor?: ImageUrlExtractor;
  /** Applied over `VERCEL_LIMITS`; can only tighten the core's limits. */
  readonly limits?: Partial<ProxyLimits>;
  /**
   * `false` disables the per-instance limiter (e.g. with a Vercel Firewall
   * rule). In memory: each instance counts on its own, so with several
   * instances the real limit is approximate.
   */
  readonly rateLimit?: RateLimitRules | false;
  /** Default: non-batch events as JSON on `console.warn`. */
  readonly log?: (event: ProxyLogEvent) => void;
  /** Default: `process.env`. */
  readonly env?: Readonly<Record<string, string | undefined>>;
}

export interface VercelProxy {
  /** Web-standard handler: `export default { fetch: proxy.fetch }`. */
  readonly fetch: (request: Request) => Promise<Response>;
}

interface Built {
  readonly handle: ReturnType<typeof createProxyHandler>;
  readonly storage: SweepableStorage | undefined;
  readonly cronSecret: string | undefined;
}

const CONFIG_ERRORS = new Set(["EnvError", "ProxyConfigError", "TicketError"]);
const FUNCTION_PATH = "/api/proxy";
/** The core routes on the path only; the Host header is not trusted. */
const BASE = "http://image-proxy.invalid";

const errorResponse = (status: number, code: string): Response =>
  Response.json(
    { error: code },
    {
      headers: {
        "Cache-Control": "no-store",
        "Content-Security-Policy": "default-src 'none'; sandbox",
        "X-Content-Type-Options": "nosniff",
      },
      status,
    }
  );

const defaultLog = (event: ProxyLogEvent): void => {
  if (event.event === "preload-full-email") {
    console.warn(FULL_EMAIL_WARNING);
  } else if (event.event !== "batch") {
    console.warn(JSON.stringify(event));
  }
};

/** Refuse runtimes where net-guard's pinned fetch does not work. */
const checkRuntime = (): void => {
  if (Reflect.get(globalThis, "EdgeRuntime") !== undefined) {
    throw new ProxyConfigError(
      "image proxy: the Vercel Edge runtime is not supported, use Node.js"
    );
  }
};

/** The image cache chosen by `IMAGE_CACHE_STORAGE`. */
const storageFor = (
  config: VercelEnv,
  options: VercelProxyOptions,
  limits: ProxyLimits
): SweepableStorage | undefined => {
  if (config.storage === "vercel-blob") {
    if (options.blob === undefined) {
      throw new EnvError(
        "image proxy: IMAGE_CACHE_STORAGE=vercel-blob needs the @vercel/blob module (see README)"
      );
    }
    return createBlobStorage(options.blob, {
      maxAgeMs: limits.cacheTtlMs,
      maxBytes: limits.maxBytesPerImage,
    });
  }
  return config.s3 === undefined
    ? undefined
    : createS3Storage({
        ...config.s3,
        maxAgeMs: limits.cacheTtlMs,
        maxBytes: limits.maxBytesPerImage,
      });
};

/** Preload, with its state next to the cache, outside `img/` (never swept). */
const preloadFor = (
  config: VercelEnv,
  options: VercelProxyOptions
): PreloadConfig | undefined => {
  if (config.preload === undefined) {
    return undefined;
  }
  if (config.preload.fullEmail && options.htmlExtractor === undefined) {
    throw new EnvError(
      "image proxy: IMAGE_PRELOAD_FULL_EMAIL needs the htmlExtractor option (see README)"
    );
  }
  let state: PreloadStateStore | undefined;
  if (config.storage === "vercel-blob" && options.blob !== undefined) {
    state = createBlobStateStore(options.blob);
  } else if (config.s3 !== undefined) {
    state = createS3StateStore(config.s3);
  }
  return state === undefined
    ? undefined
    : {
        ...(config.preload.fullEmail && options.htmlExtractor !== undefined
          ? { extractor: options.htmlExtractor }
          : {}),
        secret: config.preload.secret,
        state,
        ...preloadOptions(config.preload),
      };
};

const build = async (
  options: VercelProxyOptions,
  guarded: GuardedFetch | undefined
): Promise<Built> => {
  checkRuntime();
  const config = parseVercelEnv(options.env ?? process.env);
  const keys = await parseTicketKeys(config.keys);
  const limits = resolveLimits({
    ...VERCEL_LIMITS,
    ...options.limits,
    ...(config.cacheTtlMs === undefined
      ? {}
      : { cacheTtlMs: config.cacheTtlMs }),
  });
  const storage = storageFor(config, options, limits);
  const preload = preloadFor(config, options);
  const fetcher =
    guarded ??
    createGuardedFetch({ limits: { maxBodyBytes: limits.maxBytesPerImage } });
  const rateLimit =
    options.rateLimit === false
      ? undefined
      : createRateLimiter(
          options.rateLimit === undefined ? {} : { rules: options.rateLimit }
        );
  const handle = createProxyHandler({
    appOrigin: config.appOrigin,
    fetchImage: fetchImageWith(fetcher),
    keys,
    limits,
    log: options.log ?? defaultLog,
    ...(storage === undefined ? {} : { storage }),
    ...(rateLimit === undefined ? {} : { rateLimit }),
    ...(preload === undefined ? {} : { preload }),
  });
  return { cronSecret: config.cronSecret, handle, storage };
};

/**
 * Vercel's `waitUntil` (what `@vercel/functions` reads, without the
 * dependency): background work (preload) keeps the function alive after
 * the response. Absent: the core keeps the work in the same invocation
 * only as long as the instance lives, so preload may be cut short.
 */
const vercelWaitUntil = (): RequestContext["waitUntil"] => {
  const holder: unknown = Reflect.get(
    globalThis,
    Symbol.for("@vercel/request-context")
  );
  const get: unknown =
    typeof holder === "object" && holder !== null
      ? Reflect.get(holder, "get")
      : undefined;
  if (typeof get !== "function") {
    return undefined;
  }
  const context: unknown = Reflect.apply(get, holder, []);
  const waitUntil: unknown =
    typeof context === "object" && context !== null
      ? Reflect.get(context, "waitUntil")
      : undefined;
  return typeof waitUntil === "function"
    ? (promise) => {
        Reflect.apply(waitUntil, context, [promise]);
      }
    : undefined;
};

/** The original path: from `?__path=` when rewritten to the function. */
const routedPath = (request: Request): string => {
  const url = new URL(request.url);
  const routed = url.searchParams.get("__path");
  if (url.pathname === FUNCTION_PATH && routed !== null) {
    return `/${routed.replace(/^\/+/u, "")}`;
  }
  return url.pathname;
};

const toCoreRequest = (request: Request, path: string): Request =>
  new Request(`${BASE}${path}`, {
    headers: request.headers,
    method: request.method,
    signal: request.signal,
    ...(request.method === "POST" && request.body !== null
      ? { body: request.body, duplex: "half" }
      : {}),
  });

const sameSecret = (header: string | null, secret: string): boolean => {
  const given = Buffer.from(header ?? "");
  const expected = Buffer.from(`Bearer ${secret}`);
  return given.length === expected.length && timingSafeEqual(given, expected);
};

const sweep = async (built: Built, request: Request): Promise<Response> => {
  if (
    built.cronSecret === undefined ||
    !sameSecret(request.headers.get("authorization"), built.cronSecret)
  ) {
    return errorResponse(401, "unauthorized");
  }
  const deleted =
    ((await built.storage?.sweep()) ?? 0) +
    ((await built.handle.sweepPreload?.()) ?? 0);
  return Response.json(
    { deleted },
    { headers: { "Cache-Control": "no-store" } }
  );
};

/** Internal: the guarded fetch is a parameter only here (see `testing.ts`). */
export const buildVercelProxy = (
  options: VercelProxyOptions,
  guarded: GuardedFetch | undefined
): VercelProxy => {
  let pending: Promise<Built> | undefined;
  return {
    fetch: async (request) => {
      let built: Built;
      try {
        pending ??= build(options, guarded);
        built = await pending;
      } catch (error) {
        console.error(
          error instanceof Error && CONFIG_ERRORS.has(error.name)
            ? error.message
            : "image proxy: failed to start"
        );
        return errorResponse(500, "misconfigured");
      }
      try {
        const path = routedPath(request);
        if (path === "/cron/sweep" && request.method === "GET") {
          return await sweep(built, request);
        }
        // Vercel sets x-real-ip to the client address (not forgeable there).
        const address = clientKey(
          request.headers.get("x-real-ip") ?? undefined
        );
        const waitUntil = vercelWaitUntil();
        return await built.handle(toCoreRequest(request, path), {
          ...(address === undefined ? {} : { clientAddress: address }),
          ...(waitUntil === undefined ? {} : { waitUntil }),
        });
      } catch {
        // Never echo the error: it could contain a URL.
        return errorResponse(500, "internal");
      }
    },
  };
};

/**
 * Production Vercel proxy. Downloads always go through net-guard's pinned
 * Node fetch; there is no option to replace it.
 */
export const createVercelProxy = (
  options: VercelProxyOptions = {}
): VercelProxy => buildVercelProxy(options, undefined);
