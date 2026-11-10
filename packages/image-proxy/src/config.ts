// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Configuration contract of the runtime-agnostic core: the
// fetcher, storage, tracker check, clock and rate limiter are injected.

import type { ImageUrlExtractor } from "@nebulamail/mail-render/extract";
import type {
  ConcurrencyLimiter,
  FetchBudget,
  FetchFailureReason,
  RateLimiter,
} from "@nebulamail/net-guard";

import type { CacheHeaders } from "./cache-policy";
import type { PreloadStateStore } from "./preload/state";
import type { TicketKeys } from "./tickets/ticket";

export type {
  HtmlParser,
  ImageUrlExtractor,
} from "@nebulamail/mail-render/extract";

/** An image as stored in cache mode. `type` is a sniffed raster MIME type (never SVG). */
export interface StoredImage {
  readonly type: string;
  readonly bytes: Uint8Array;
  /**
   * From `get` only, when the storage knows it: when the entry expires
   * (ms). The proxy never tells browsers to keep it longer.
   */
  readonly expiresAt?: number;
}

/** What a fetcher returns on success. The core re-checks type and magic bytes. */
export interface FetchedImage {
  readonly type: string;
  readonly bytes: Uint8Array;
  /**
   * The response's caching fields (cache-policy.ts, RFC 9111). Missing:
   * none known, so only heuristic freshness (our own maximum).
   */
  readonly cacheHeaders?: CacheHeaders;
}

/**
 * `upstream`: the server answered with a non-2xx status. `refused`: a
 * redirect target was refused by `allowRedirect` (a tracker).
 */
export interface FetchFailure {
  readonly error: FetchFailureReason | "upstream";
}

/** What the core asks of every download, besides the URL. */
export interface FetchImageOptions {
  /**
   * Every redirect target must pass this before it is requested (the core
   * refuses trackers on every hop).
   */
  readonly allowRedirect: (url: string) => boolean;
  /**
   * Per-email budget: each hop takes a request, and every body byte read is
   * taken from it, kept or not (net-guard's `FetchRequest.budget`).
   */
  readonly budget: FetchBudget;
}

/**
 * Downloads one image. MUST enforce the SSRF rules (the Node adapter wires
 * net-guard's `guardedFetch` with `expect: "image"`) and MUST pass
 * `options.budget` (and `allowRedirect`) on to it (`fetchImageWith` does),
 * so that bodies read and then refused are counted. A kept image the
 * fetcher did not charge is still taken from the budget by the core, so
 * forgetting it cannot lift the per-email cap. Should not throw; a throw is
 * treated as `upstream`.
 */
export type FetchImage = (
  url: string,
  signal: AbortSignal,
  options?: FetchImageOptions
) => Promise<FetchedImage | FetchFailure>;

/** Cache-mode storage. Keys are opaque `[0-9a-z:]` strings, never URLs. */
export interface ImageStorage {
  readonly get: (key: string) => Promise<StoredImage | null>;
  readonly put: (
    key: string,
    image: StoredImage,
    ttlMs: number
  ) => Promise<void>;
  /**
   * Delete one entry (missing is fine). Used when a user opts out of
   * preload; without it their preloaded images just expire.
   */
  readonly delete?: (key: string) => Promise<void>;
  /**
   * Set by storages whose entries live a fixed time from upload whatever
   * `ttlMs` says (R2, S3, Vercel Blob). The core then stores only what may
   * be kept at least that long (RFC 9111: never longer than the sender
   * allows).
   */
  readonly fixedTtlMs?: number;
}

/** A storage that can delete its expired entries (run it periodically). */
export interface SweepableStorage extends ImageStorage {
  /** Delete expired entries. Returns how many were deleted. */
  readonly sweep: () => Promise<number>;
}

export const isSweepable = (
  storage: ImageStorage
): storage is SweepableStorage =>
  typeof Reflect.get(storage, "sweep") === "function";

export interface ProxyLimits {
  readonly maxImagesPerEmail: number;
  readonly maxBytesPerEmail: number;
  readonly maxBytesPerImage: number;
  /**
   * HTTP requests per email, redirects and the https→http retry included
   * (net-guard's per-email budget).
   */
  readonly maxRequestsPerEmail: number;
  /** Simultaneous downloads per batch request. */
  readonly concurrency: number;
  /** Simultaneous downloads in the whole process / isolate, all emails together. */
  readonly globalConcurrency: number;
  /**
   * Cache retention (cache and preload modes). The default (30 days) is NOT
   * an upper bound: the admin may set any value.
   */
  readonly cacheTtlMs: number;
  /** Whole `POST /batch` download phase; unfinished images become `timeout`. */
  readonly batchTimeoutMs: number;
}

export const DAY_MS = 24 * 60 * 60 * 1000;

/** Default retention: 30 days. */
export const DEFAULT_CACHE_TTL_MS = 30 * DAY_MS;

/**
 * Defaults. They are also upper bounds (options can only tighten them),
 * except `cacheTtlMs`, which the admin may raise.
 */
export const DEFAULT_LIMITS: ProxyLimits = Object.freeze({
  batchTimeoutMs: 30_000,
  cacheTtlMs: DEFAULT_CACHE_TTL_MS,
  concurrency: 6,
  globalConcurrency: 32,
  maxBytesPerEmail: 10 * 1024 * 1024,
  maxBytesPerImage: 5 * 1024 * 1024,
  maxImagesPerEmail: 60,
  // 60 images × (https try + http fallback) plus some redirects.
  maxRequestsPerEmail: 180,
});

/** Log events. They never contain URLs, tickets, ids or addresses. */
export type ProxyLogEvent =
  | {
      /** The disk cap had to evict shared (preloaded) entries. */
      readonly event: "shared-evicted";
      readonly files: number;
    }
  | {
      readonly event: "batch";
      readonly mode: "batch" | "cache";
      readonly images: number;
      readonly fetched: number;
      readonly fromCache: number;
      readonly failed: number;
      readonly bytes: number;
    }
  | {
      readonly event: "preload";
      /** Recipients preloaded for, opted out, and over their quota. */
      readonly recipients: number;
      readonly optedOut: number;
      readonly overQuota: number;
      readonly images: number;
      readonly fetched: number;
      readonly fromCache: number;
      readonly failed: number;
      readonly bytes: number;
    }
  | {
      readonly event: "preload-skipped";
      readonly reason:
        | "busy"
        | "rate-limited"
        | "too-large"
        | "invalid"
        | "no-recipient"
        | "too-many-recipients"
        | "opted-out"
        | "quota"
        | "unparsable";
    }
  | {
      /** Option D: the email gets no token (its preload still runs). */
      readonly event: "preload-token-skipped";
      readonly reason: "too-large";
    }
  | {
      readonly event: "preload-preferences";
      readonly addresses: number;
      readonly optOut: boolean;
      /** The stored state is the one asked for (false: a newer choice won). */
      readonly applied: boolean;
    }
  | { readonly event: "preload-purge"; readonly deleted: number }
  /**
   * At start, when `POST /hooks/stalwart` is open: every incoming email
   * (headers, body, attachments) reaches this proxy.
   */
  | { readonly event: "preload-full-email" }
  | { readonly event: "tracker-in-ticket" }
  | { readonly event: "tracker-redirect" }
  | { readonly event: "storage-error" }
  | { readonly event: "corrupt-cache-entry" }
  | {
      readonly event: "rejected";
      readonly status: number;
      readonly reason: string;
    };

/**
 * `preload` mode: images fetched when an email arrives. The relay
 * (`POST /hooks/preload-urls`) sends only image URLs and hashed recipients;
 * with `fullEmail`, Stalwart's MTA Hook may also send the whole email.
 */
export interface PreloadConfig {
  /**
   * Bearer token of the relay (and of Stalwart's MTA Hook with
   * `fullEmail`). At least 32 characters; compared in constant time.
   */
  readonly secret: string;
  /**
   * Open `POST /hooks/stalwart`: Stalwart sends the WHOLE email here, so
   * whoever runs the proxy (a host like Vercel included) can read every
   * incoming email. Default off: use the relay instead. Logs
   * `preload-full-email` at start when on.
   */
  readonly fullEmail?: boolean;
  /**
   * Image URL extraction from the email's HTML: on a server,
   * `createWorkerExtractor` from `@nebulamail/mail-render/server-extract` (the
   * same parser and code as the BFF's tickets, so both pick exactly the
   * same URLs; parsed in a worker thread with a deadline).
   * Needed only with `fullEmail` (with the relay, the relay parses).
   */
  readonly extractor?: ImageUrlExtractor;
  /** Hook request bodies over this are accepted without preloading. Default 25 MB. */
  readonly maxMessageBytes?: number;
  /** Emails preloaded at the same time. Default 2. */
  readonly concurrency?: number;
  /** Emails waiting beyond `concurrency`; more are skipped. Default 50. */
  readonly maxQueue?: number;
  /**
   * Characters of whole emails (`fullEmail`) held by queued and running
   * jobs; more are skipped (`busy`). Default 4 × `maxMessageBytes`.
   */
  readonly maxQueuedChars?: number;
  /**
   * Per-recipient usage, references and opt-outs (`preload/state.ts`).
   * Must persist (disk, S3, Blob): an opt-out kept only in memory would be
   * lost on restart.
   */
  readonly state: PreloadStateStore;
  /**
   * Key of the recipient hashes (32+ characters). Default: `secret`. Set
   * it to rotate `secret` without losing quotas and opt-outs.
   */
  readonly indexKey?: string;
  /** Preloaded bytes per recipient address. Default 256 MiB. */
  readonly maxBytesPerRecipient?: number;
  /** Preloaded images per recipient address. Default 10 000. */
  readonly maxImagesPerRecipient?: number;
  /**
   * Add an email token header to each preloaded email
   * (`tickets/email-token.ts`), so the static build can show its
   * images without a server of its own. Default off.
   */
  readonly emailTokens?: boolean;
}

export interface ProxyConfig {
  readonly keys: TicketKeys;
  /** Exact CORS origin of the app for `POST /batch`, e.g. `https://mail.example.com`. */
  readonly appOrigin: string;
  readonly fetchImage: FetchImage;
  /** Present = cache mode available; absent = batch mode only. */
  readonly storage?: ImageStorage;
  /** Re-check before downloading. Default: mail-render's `isTrackerUrl`. */
  readonly isTracker?: (url: string) => boolean;
  readonly limits?: Partial<ProxyLimits>;
  readonly now?: () => number;
  /**
   * `true` = allowed; may be async (Cloudflare's Rate Limiting binding).
   * Keys: `batch:ip:<ip>`, `batch:account:<accountId>`, `batch:global`,
   * `image:ip:<ip>`, `image:global`, `preload:global` (ip keys only when
   * the adapter passes `clientAddress`). A throwing limiter allows: the
   * in-memory one never throws, and a remote one must not take the proxy down.
   */
  readonly rateLimit?: RateLimiter;
  /**
   * Downloads in flight across all requests. Default: one limiter per
   * handler with `limits.globalConcurrency`.
   */
  readonly downloadSlots?: ConcurrencyLimiter;
  /** `preload` mode: the Stalwart MTA Hook endpoint. Needs `storage`. */
  readonly preload?: PreloadConfig;
  readonly log?: (event: ProxyLogEvent) => void;
}

/** Per-request data only the runtime adapter knows. */
export interface RequestContext {
  readonly clientAddress?: string;
  /**
   * Keeps background work alive after the response (Workers'
   * `ctx.waitUntil`, Vercel's request context). Absent on Node, where the
   * process keeps running anyway.
   */
  readonly waitUntil?: (promise: Promise<unknown>) => void;
}

export class ProxyConfigError extends Error {
  override readonly name = "ProxyConfigError";
}

/**
 * Clamp each option to its default (except `cacheTtlMs`: any positive
 * value); invalid values are a configuration error.
 */
export const resolveLimits = (
  limits: Partial<ProxyLimits> = {}
): ProxyLimits => {
  const value = (name: keyof ProxyLimits): number => {
    const given = limits[name];
    if (given === undefined) {
      return DEFAULT_LIMITS[name];
    }
    if (!Number.isSafeInteger(given) || given <= 0) {
      throw new ProxyConfigError(
        `image proxy: limit ${name} must be a positive integer`
      );
    }
    return given;
  };
  const clamp = (name: keyof ProxyLimits): number =>
    Math.min(value(name), DEFAULT_LIMITS[name]);
  return {
    batchTimeoutMs: clamp("batchTimeoutMs"),
    cacheTtlMs: value("cacheTtlMs"),
    concurrency: clamp("concurrency"),
    globalConcurrency: clamp("globalConcurrency"),
    maxBytesPerEmail: clamp("maxBytesPerEmail"),
    maxBytesPerImage: clamp("maxBytesPerImage"),
    maxImagesPerEmail: clamp("maxImagesPerEmail"),
    maxRequestsPerEmail: clamp("maxRequestsPerEmail"),
  };
};

/** `https://mail.example.com` style origin, exactly as browsers send it. */
export const checkOrigin = (origin: string): string => {
  const parsed = URL.parse(origin);
  if (
    parsed === null ||
    parsed.origin !== origin ||
    (parsed.protocol !== "https:" && parsed.protocol !== "http:")
  ) {
    throw new ProxyConfigError(
      "image proxy: appOrigin must be an origin like https://mail.example.com"
    );
  }
  return origin;
};
