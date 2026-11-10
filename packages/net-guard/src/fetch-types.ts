// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Fetch contract shared by the adapters (Node with pinned connections,
// Workers without): request and result types, generic headers and limits.

import { GuardError } from "./errors";
import { allowedImageType, mediaType } from "./image";
import type { ImageType } from "./image";
import type { FetchBudget } from "./limits";
import { LIMITS } from "./types";
import type { BlockReason } from "./types";

/**
 * Why a fetch failed: a `BlockReason` (the contract shared with the Rust
 * crate and `ssrf-vectors.json`) or a refusal decided by the caller's own
 * options, which exist only in TypeScript:
 * - `refused`: `allowRedirect` said no to a redirect target.
 * - `budget`: the `budget` had no request left for the next hop.
 */
export type FetchFailureReason = BlockReason | "refused" | "budget";

export interface FetchLimits {
  readonly connectTimeoutMs: number;
  /** Whole operation: DNS, every hop, headers and body. */
  readonly totalTimeoutMs: number;
  /** Applies to the raw body AND to the decompressed body. */
  readonly maxBodyBytes: number;
  readonly maxRedirects: number;
}

/** A small request body (POST). Never anything from the user's session. */
export interface RequestBody {
  /** `Content-Type` of the body, e.g. `application/x-www-form-urlencoded`. */
  readonly type: string;
  readonly data: string;
}

/**
 * How redirects are handled. Every followed hop runs the full guard again.
 * - `"follow"`: up to `maxRedirects`, to any public host.
 * - `"same-host"`: only to the same hostname (any allowed scheme and port);
 *   a redirect elsewhere is not followed and its 3xx status is returned.
 * - `"none"`: never followed; the 3xx status is returned.
 */
export type RedirectMode = "follow" | "same-host" | "none";

export interface FetchRequest {
  /** Defaults to GET. POST needs `body`. */
  readonly method?: "GET" | "POST";
  /** Only with POST; at most `MAX_REQUEST_BODY_BYTES` (UTF-8). */
  readonly body?: RequestBody;
  /** Defaults to `"follow"`. */
  readonly redirects?: RedirectMode;
  /**
   * `false`: the response body is never read (only status and headers), for
   * callers that only need to know whether the server accepted.
   */
  readonly readBody?: boolean;
  /**
   * `"image"`: Content-Type must be an allowed image (no SVG) and match the
   * magic bytes; the `Accept` header asks for images.
   */
  readonly expect?: "image" | "any";
  /** For `expect: "any"`: allowed media types (lowercase, no parameters). */
  readonly allowedContentTypes?: readonly string[];
  /** `Accept` header. Defaults to an image list or `*\/*`. */
  readonly accept?: string;
  /** Abort early. An abort is reported as `timeout`. */
  readonly signal?: AbortSignal;
  /**
   * Called with every redirect target (absolute URL) before the guard runs
   * on it; `false` stops the fetch with `refused`. For caller rules that are
   * not SSRF rules, e.g. the image proxy refusing tracker hosts on every hop.
   * Must not throw (a throw also refuses).
   */
  readonly allowRedirect?: (url: string) => boolean;
  /**
   * Shared budget (e.g. per email). Each hop takes one request (no request
   * left: `budget`). Each body chunk read is taken from `budget` (decoded
   * bytes), kept or not: a body refused afterwards (wrong magic bytes, cut
   * by `size`, reset) still spends what was read. More than is left: `size`,
   * and the budget is exhausted.
   */
  readonly budget?: FetchBudget;
}

export type FetchResult =
  | {
      readonly ok: true;
      readonly status: number;
      /** Final URL after redirects. Do not log it. */
      readonly url: string;
      /** Response headers, lowercase, without cookies, encodings and hop-by-hop fields. */
      readonly headers: Readonly<Record<string, string>>;
      /** Decoded body. Empty for non-2xx responses (their bodies are discarded). */
      readonly body: Uint8Array;
      /** Canonical type when `expect: "image"` and the check passed. */
      readonly imageType?: ImageType;
      readonly redirects: number;
    }
  | { readonly ok: false; readonly reason: FetchFailureReason };

export type GuardedFetch = (
  url: string | URL,
  request?: FetchRequest
) => Promise<FetchResult>;

/** Generic browser-like headers. Nothing from the user, no cookies, no referrer. */
export const USER_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
export const IMAGE_ACCEPT =
  "image/avif,image/webp,image/png,image/jpeg,image/gif,image/*;q=0.8";

export const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308]);

const clampLimit = (value: number | undefined, max: number): number =>
  value !== undefined && Number.isFinite(value) && value >= 0
    ? Math.min(Math.floor(value), max)
    : max;

export const resolveFetchLimits = (
  limits: Partial<FetchLimits> = {}
): FetchLimits => ({
  connectTimeoutMs: clampLimit(
    limits.connectTimeoutMs,
    LIMITS.connectTimeoutMs
  ),
  maxBodyBytes: clampLimit(limits.maxBodyBytes, LIMITS.maxImageBytes),
  maxRedirects: clampLimit(limits.maxRedirects, LIMITS.maxRedirects),
  totalTimeoutMs: clampLimit(limits.totalTimeoutMs, LIMITS.totalTimeoutMs),
});

/** Rejects as soon as `signal` aborts, so a hanging resolver cannot outlive the deadline. */
export const raceAbort = async <T>(
  promise: Promise<T>,
  signal: AbortSignal
): Promise<T> => {
  const aborted = Promise.withResolvers<never>();
  const onAbort = () => {
    aborted.reject(new GuardError("timeout"));
  };
  if (signal.aborted) {
    onAbort();
  } else {
    signal.addEventListener("abort", onAbort, { once: true });
  }
  try {
    return await Promise.race([promise, aborted.promise]);
  } finally {
    signal.removeEventListener("abort", onAbort);
  }
};

export const checkContentType = (
  contentType: string | undefined,
  request: FetchRequest
): void => {
  if (request.expect === "image") {
    if (allowedImageType(contentType) === undefined) {
      throw new GuardError("content-type");
    }
  } else if (
    request.allowedContentTypes !== undefined &&
    !request.allowedContentTypes.includes(mediaType(contentType))
  ) {
    throw new GuardError("content-type");
  }
};

/** `allowRedirect` of the request; a throwing callback refuses. */
export const redirectAllowed = (
  request: FetchRequest,
  href: string
): boolean => {
  if (request.allowRedirect === undefined) {
    return true;
  }
  try {
    return request.allowRedirect(href);
  } catch {
    return false;
  }
};

/** Takes each body chunk read from the request's budget (see `FetchRequest.budget`). */
export const budgetCharge = (
  request: FetchRequest
): ((bytes: number) => boolean) | undefined => {
  const { budget } = request;
  return budget === undefined ? undefined : (bytes) => budget.takeBytes(bytes);
};

/**
 * Limits for the next hop: takes one request from the budget and caps the
 * body to what is left of it. A failure reason when the hop must not run.
 */
export const hopLimits = (
  limits: FetchLimits,
  request: FetchRequest
): FetchLimits | FetchFailureReason => {
  const { budget } = request;
  if (budget === undefined) {
    return limits;
  }
  const remaining = budget.remainingBytes();
  if (remaining < 1 || budget.exhausted()) {
    return "size";
  }
  if (!budget.takeRequest()) {
    return "budget";
  }
  return { ...limits, maxBodyBytes: Math.min(limits.maxBodyBytes, remaining) };
};
