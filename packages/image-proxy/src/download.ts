// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Downloads all images of one email at once: bounded
// concurrency (per email and for the whole process), net-guard's per-email
// budget (requests and bytes), one deadline, and the tracker check on every
// redirect hop.

import {
  checkImage,
  createFetchBudget,
  QueueFullError,
} from "@nebulamail/net-guard";
import type { ConcurrencyLimiter, FetchBudget } from "@nebulamail/net-guard";

import type {
  FetchedImage,
  FetchFailure,
  FetchImage,
  FetchImageOptions,
} from "./config";

/** Per-image errors returned to the browser. Coarse on purpose. */
export type ImageError =
  /** The URL, or one of its redirect targets, is a known tracker. */
  | "tracker"
  /** Refused by the SSRF guard (scheme, port, private address, DNS, redirects). */
  | "blocked"
  /** Not an allowed raster image, or the bytes do not match the type. */
  | "not-image"
  /** Over the per-image or per-email byte (or request) budget. */
  | "too-large"
  /** Deadline, or the proxy was too busy to start the download. */
  | "timeout"
  /** Connection failure or non-2xx answer. */
  | "upstream"
  /** Cache mode: the image could not be stored. */
  | "storage";

export type DownloadOutcome =
  | { readonly ok: true; readonly image: FetchedImage }
  | { readonly ok: false; readonly error: ImageError };

const errorFor = (failure: FetchFailure["error"]): ImageError => {
  switch (failure) {
    case "content-type": {
      return "not-image";
    }
    case "size":
    case "budget": {
      return "too-large";
    }
    case "timeout": {
      return "timeout";
    }
    case "refused": {
      return "tracker";
    }
    case "network":
    case "upstream": {
      return "upstream";
    }
    case "scheme":
    case "userinfo":
    case "port":
    case "private-ip":
    case "dns":
    case "too-many-redirects": {
      return "blocked";
    }
    default: {
      return "upstream";
    }
  }
};

const callFetcher = async (
  fetchImage: FetchImage,
  url: string,
  signal: AbortSignal,
  options: FetchImageOptions
): Promise<FetchedImage | FetchFailure> => {
  try {
    return await fetchImage(url, signal, options);
  } catch {
    return { error: signal.aborted ? "timeout" : "upstream" };
  }
};

/**
 * One image. `http:` URLs are tried over `https:` first and
 * fall back to `http:` only when https failed at the connection level
 * (`network`: refused, reset, TLS error). Both attempts go through the
 * injected fetcher, so both are guarded. The result is re-checked here, so a
 * buggy fetcher cannot pass SVG, HTML or an oversized body through.
 */
export const downloadImage = async (
  fetchImage: FetchImage,
  url: string,
  maxBytes: number,
  signal: AbortSignal,
  options: FetchImageOptions
): Promise<DownloadOutcome> => {
  let result: FetchedImage | FetchFailure | undefined;
  const parsed = URL.parse(url);
  if (parsed?.protocol === "http:") {
    parsed.protocol = "https:";
    result = await callFetcher(fetchImage, parsed.href, signal, options);
    if ("error" in result && result.error === "network" && !signal.aborted) {
      result = undefined;
    }
  }
  result ??= await callFetcher(fetchImage, url, signal, options);
  if ("error" in result) {
    return { error: errorFor(result.error), ok: false };
  }
  if (result.bytes.byteLength > maxBytes) {
    return { error: "too-large", ok: false };
  }
  const verdict = checkImage(result.type, result.bytes);
  if (!verdict.ok) {
    return { error: "not-image", ok: false };
  }
  return {
    image: {
      bytes: result.bytes,
      type: verdict.type,
      ...(result.cacheHeaders === undefined
        ? {}
        : { cacheHeaders: result.cacheHeaders }),
    },
    ok: true,
  };
};

export interface BatchPlan {
  readonly fetchImage: FetchImage;
  readonly maxBytesPerImage: number;
  readonly maxBytesPerEmail: number;
  readonly maxRequestsPerEmail: number;
  readonly concurrency: number;
  readonly timeoutMs: number;
  /** `false` for redirect targets that must not be requested (trackers). */
  readonly allowRedirect: (url: string) => boolean;
  /** Process-wide download slots, shared by every email. */
  readonly slots?: ConcurrencyLimiter;
  /** Client went away: stop everything. */
  readonly signal?: AbortSignal;
}

/** One download inside a global slot; no slot (queue full) = `timeout`. */
const inSlot = async (
  plan: BatchPlan,
  signal: AbortSignal,
  download: () => Promise<DownloadOutcome>
): Promise<DownloadOutcome> => {
  if (plan.slots === undefined) {
    return await download();
  }
  try {
    return await plan.slots.run(download, signal);
  } catch (error) {
    if (error instanceof QueueFullError || signal.aborted) {
      return { error: "timeout", ok: false };
    }
    throw error;
  }
};

/**
 * The email's budget as one download sees it: every call goes to `shared`,
 * and `charged()` tells how many bytes this download took from it.
 */
const meteredBudget = (
  shared: FetchBudget
): { readonly budget: FetchBudget; readonly charged: () => number } => {
  let charged = 0;
  return {
    budget: {
      exhausted: () => shared.exhausted(),
      remainingBytes: () => shared.remainingBytes(),
      remainingRequests: () => shared.remainingRequests(),
      takeBytes: (bytes) => {
        const taken = shared.takeBytes(bytes);
        if (taken) {
          charged += bytes;
        }
        return taken;
      },
      takeRequest: () => shared.takeRequest(),
    },
    charged: () => charged,
  };
};

/**
 * Download every URL with at most `concurrency` in flight. The fetcher takes
 * the bytes it reads from the email's budget, kept or not (net-guard does);
 * if a kept image was not fully charged (a custom `FetchImage` that drops
 * `options.budget`), the rest is taken here, so the cap holds either way.
 * The first image that does not fit ends the batch: it and every image not
 * finished yet become `too-large`, and in-flight downloads are aborted.
 * Never throws.
 */
export const downloadAll = async (
  urls: readonly string[],
  plan: BatchPlan
): Promise<Map<string, DownloadOutcome>> => {
  const outcomes = new Map<string, DownloadOutcome>();
  const stop = new AbortController();
  const signals = [stop.signal, AbortSignal.timeout(plan.timeoutMs)];
  if (plan.signal !== undefined) {
    signals.push(plan.signal);
  }
  const signal = AbortSignal.any(signals);
  const budget: FetchBudget = createFetchBudget({
    maxBytes: plan.maxBytesPerEmail,
    maxRequests: plan.maxRequestsPerEmail,
  });
  let next = 0;

  const interrupted = (): DownloadOutcome => ({
    error: budget.exhausted() ? "too-large" : "timeout",
    ok: false,
  });

  const worker = async (): Promise<void> => {
    while (next < urls.length) {
      const url = urls[next];
      next += 1;
      if (url === undefined) {
        return;
      }
      if (signal.aborted) {
        outcomes.set(url, interrupted());
        continue;
      }
      const meter = meteredBudget(budget);
      const options: FetchImageOptions = {
        allowRedirect: plan.allowRedirect,
        budget: meter.budget,
      };
      // Each worker downloads one image at a time; the pool bounds concurrency.
      // oxlint-disable-next-line no-await-in-loop
      const outcome = await inSlot(
        plan,
        signal,
        async () =>
          await downloadImage(
            plan.fetchImage,
            url,
            Math.min(plan.maxBytesPerImage, plan.maxBytesPerEmail),
            signal,
            options
          )
      );
      const uncharged = outcome.ok
        ? outcome.image.bytes.byteLength - meter.charged()
        : 0;
      if (budget.exhausted()) {
        stop.abort();
        outcomes.set(url, interrupted());
      } else if (signal.aborted && !outcome.ok) {
        outcomes.set(url, interrupted());
      } else if (uncharged > 0 && !budget.takeBytes(uncharged)) {
        stop.abort();
        outcomes.set(url, { error: "too-large", ok: false });
      } else {
        outcomes.set(url, outcome);
      }
    }
  };

  const workers = Array.from(
    { length: Math.min(plan.concurrency, urls.length) },
    worker
  );
  await Promise.all(workers);
  return outcomes;
};
