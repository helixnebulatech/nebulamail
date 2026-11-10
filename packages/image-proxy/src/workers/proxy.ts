// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Cloudflare Workers adapter: the core wired to net-guard's
// Workers fetch (DoH check on every hop, NOT pinned.1c),
// Cloudflare's Rate Limiting bindings (in-memory per isolate for rules
// without one) and, with an R2 binding or S3 variables, cache mode.
// No preload here: it needs a spec-compliant HTML parser (jsdom), which
// does not run on Workers (HTMLRewriter is a stream rewriter, not a DOM).
//
//   export default createWorkersProxy();   // see worker.ts
//
// Configuration comes from the Worker's env (vars, secrets, bindings) on the
// first request and is validated like the Node env. A bad env answers 500
// on every request and logs which variable is wrong (never its value).

import {
  createDohResolver,
  createWorkersFetch,
} from "@nebulamail/net-guard/workers";
import type { WebFetch } from "@nebulamail/net-guard/workers";

import type {
  ProxyLimits,
  ProxyLogEvent,
  RequestContext,
  SweepableStorage,
} from "../config";
import { resolveLimits } from "../config";
import { fetchImageWith } from "../fetch-image";
import { createProxyHandler } from "../handler";
import { clientKey, DEFAULT_RATE_LIMITS } from "../rate-limit";
import type { RateLimitRules } from "../rate-limit";
import { createS3Storage } from "../s3/s3-storage";
import { parseTicketKeys } from "../tickets/ticket";
import { parseWorkersEnv } from "./env";
import { createR2Storage } from "./r2-storage";
import { createWorkersRateLimiter } from "./rate-limit";

export interface WorkersProxyOptions {
  readonly limits?: Partial<ProxyLimits>;
  /**
   * `false` disables rate limiting (e.g. with a WAF rate rule). Rules are
   * for the in-memory fallback; bindings carry their own limits.
   */
  readonly rateLimit?: RateLimitRules | false;
  /** Default: non-batch events as JSON on `console.warn`. */
  readonly log?: (event: ProxyLogEvent) => void;
}

/** `ExecutionContext`, only what we use. */
export interface ExecutionContextLike {
  readonly waitUntil: (promise: Promise<unknown>) => void;
}

export type WorkerEnv = Readonly<Record<string, unknown>>;

/** The Worker's default export: `fetch` and the cron `scheduled` handler. */
export interface WorkersProxy {
  readonly fetch: (
    request: Request,
    env: WorkerEnv,
    ctx?: ExecutionContextLike
  ) => Promise<Response>;
  /** Cron trigger: deletes expired R2 entries (no-op without R2). */
  readonly scheduled: (
    controller: unknown,
    env: WorkerEnv,
    ctx?: ExecutionContextLike
  ) => Promise<void>;
}

interface Built {
  readonly handle: (
    request: Request,
    context?: RequestContext
  ) => Promise<Response>;
  readonly storage: SweepableStorage | undefined;
}

const CONFIG_ERRORS = new Set(["EnvError", "ProxyConfigError", "TicketError"]);

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

const report = (error: unknown): void => {
  console.error(
    error instanceof Error && CONFIG_ERRORS.has(error.name)
      ? error.message
      : "image proxy: failed to start"
  );
};

const defaultLog = (event: ProxyLogEvent): void => {
  if (event.event !== "batch") {
    console.warn(JSON.stringify(event));
  }
};

const build = async (
  env: WorkerEnv,
  options: WorkersProxyOptions,
  net: WebFetch | undefined
): Promise<Built> => {
  const config = parseWorkersEnv(env);
  const keys = await parseTicketKeys(config.keys);
  const limits = resolveLimits({
    ...options.limits,
    ...(config.cacheTtlMs === undefined
      ? {}
      : { cacheTtlMs: config.cacheTtlMs }),
  });
  const resolver = createDohResolver({
    ...(config.dohUrl === undefined ? {} : { endpoint: config.dohUrl }),
    ...(net === undefined ? {} : { fetch: net }),
  });
  const guarded = createWorkersFetch({
    limits: { maxBodyBytes: limits.maxBytesPerImage },
    resolver,
    ...(net === undefined ? {} : { fetch: net }),
  });
  let storage: SweepableStorage | undefined;
  if (config.bucket !== undefined) {
    storage = createR2Storage(config.bucket, {
      maxAgeMs: limits.cacheTtlMs,
      maxBytes: limits.maxBytesPerImage,
    });
  } else if (config.s3 !== undefined) {
    storage = createS3Storage({
      ...config.s3,
      maxAgeMs: limits.cacheTtlMs,
      maxBytes: limits.maxBytesPerImage,
      ...(net === undefined ? {} : { fetch: net }),
    });
  }
  const rateLimit =
    options.rateLimit === false
      ? undefined
      : createWorkersRateLimiter(
          config.rateLimits,
          options.rateLimit ?? DEFAULT_RATE_LIMITS
        );
  const handle = createProxyHandler({
    appOrigin: config.appOrigin,
    fetchImage: fetchImageWith(guarded),
    keys,
    limits,
    log: options.log ?? defaultLog,
    ...(storage === undefined ? {} : { storage }),
    ...(rateLimit === undefined ? {} : { rateLimit }),
  });
  return { handle, storage };
};

/** Internal: `net` (the raw network `fetch`) is a parameter only here (see `testing.ts`). */
export const buildWorkersProxy = (
  options: WorkersProxyOptions,
  net: WebFetch | undefined
): WorkersProxy => {
  // One configuration per env object (stable within an isolate).
  const built = new WeakMap<object, Promise<Built>>();
  const get = async (env: WorkerEnv): Promise<Built> => {
    let pending = built.get(env);
    if (pending === undefined) {
      pending = build(env, options, net);
      built.set(env, pending);
    }
    return await pending;
  };

  return {
    fetch: async (request, env, ctx) => {
      let proxy: Built;
      try {
        proxy = await get(env);
      } catch (error) {
        report(error);
        return errorResponse(500, "misconfigured");
      }
      // Set by Cloudflare's edge; clients cannot forge it.
      const address = clientKey(
        request.headers.get("cf-connecting-ip") ?? undefined
      );
      try {
        const work = proxy.handle(request, {
          ...(address === undefined ? {} : { clientAddress: address }),
          ...(ctx === undefined
            ? {}
            : {
                waitUntil: (promise: Promise<unknown>) => {
                  ctx.waitUntil(promise);
                },
              }),
        });
        // If the client disconnects, Workers cancels pending work that is
        // not registered with `waitUntil`, and the `finally` blocks that
        // free download slots and `inflight` entries would never run. The
        // work is bounded by `batchTimeoutMs` and Workers' CPU limit. We do
        // not abort on disconnect (`enable_request_signal`): a cache-mode
        // download another tab waits for must still be stored.
        ctx?.waitUntil(work.catch(() => null));
        return await work;
      } catch {
        // Never echo the error: it could contain a URL.
        return errorResponse(500, "internal");
      }
    },
    scheduled: async (_controller, env) => {
      const proxy = await get(env);
      await proxy.storage?.sweep();
    },
  };
};

/**
 * Production Worker. Downloads always go through net-guard's Workers fetch
 * (DoH + guard on every hop); there is no option to replace it.
 */
export const createWorkersProxy = (
  options: WorkersProxyOptions = {}
): WorkersProxy => buildWorkersProxy(options, undefined);
