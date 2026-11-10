// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Node adapter: the core wired to net-guard's pinned fetch,
// an in-memory rate limiter (per process: with several replicas the limit
// is approximate) and, optionally, disk or S3 storage and preload.

import type { IncomingMessage, RequestListener } from "node:http";

import { createGuardedFetch } from "@nebulamail/net-guard/node";
import type { GuardedFetch } from "@nebulamail/net-guard/node";

import type {
  ImageStorage,
  PreloadConfig,
  ProxyLimits,
  ProxyLogEvent,
} from "../config";
import { isSweepable, resolveLimits } from "../config";
import { fetchImageWith } from "../fetch-image";
import { createProxyHandler } from "../handler";
import { clientKey, createRateLimiter } from "../rate-limit";
import type { RateLimitRules } from "../rate-limit";
import type { TicketKeys } from "../tickets/ticket";
import { createDiskStorage } from "./disk-storage";
import { toNodeListener } from "./http";
import type { WebHandler } from "./http";

export interface NodeProxyOptions {
  readonly keys: TicketKeys;
  readonly appOrigin: string;
  /** Enables cache mode with disk storage in this directory. */
  readonly cacheDir?: string;
  /**
   * Size cap for `cacheDir`: the account using the most is evicted first,
   * shared (preloaded) entries last. The Node server always sets one.
   */
  readonly cacheMaxBytes?: number;
  /**
   * Enables cache mode with another storage (e.g. `createS3Storage`).
   * Exclusive with `cacheDir`. If it has `sweep`, `NodeProxy.sweep` runs it.
   */
  readonly storage?: ImageStorage;
  /**
   * `preload` mode: the relay's `POST /hooks/preload-urls` and, with
   * `fullEmail`, Stalwart's `POST /hooks/stalwart` (the whole email).
   */
  readonly preload?: PreloadConfig;
  readonly limits?: Partial<ProxyLimits>;
  /** `false` disables rate limiting (e.g. when a front proxy does it). */
  readonly rateLimit?: RateLimitRules | false;
  readonly log?: (event: ProxyLogEvent) => void;
  /**
   * Client address for rate limiting. Default: the socket's peer address.
   * Behind a reverse proxy, read the header that proxy sets (and only that).
   */
  readonly clientAddress?: (req: IncomingMessage) => string | undefined;
}

export interface NodeProxy {
  readonly handle: WebHandler;
  readonly listener: RequestListener;
  readonly storage: ImageStorage | undefined;
  /**
   * Delete expired entries (disk, or a storage with `sweep`) and, with
   * preload, expired quota and reference records. Run it periodically.
   */
  readonly sweep: (() => Promise<number>) | undefined;
}

const socketAddress = (req: IncomingMessage): string | undefined =>
  req.socket.remoteAddress;

/** Internal: the guarded fetch is a parameter only here (see `testing.ts`). */
export const buildNodeProxy = (
  options: NodeProxyOptions,
  guarded: GuardedFetch | undefined
): NodeProxy => {
  if (options.cacheDir !== undefined && options.storage !== undefined) {
    throw new Error("image proxy: set cacheDir or storage, not both");
  }
  const limits = resolveLimits(options.limits);
  const disk =
    options.cacheDir === undefined
      ? undefined
      : createDiskStorage(options.cacheDir, {
          ...(options.cacheMaxBytes === undefined
            ? {}
            : { maxBytes: options.cacheMaxBytes }),
          onSharedEvicted: (files) => {
            options.log?.({ event: "shared-evicted", files });
          },
        });
  const storage = options.storage ?? disk;
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
    appOrigin: options.appOrigin,
    fetchImage: fetchImageWith(fetcher),
    keys: options.keys,
    limits,
    ...(storage === undefined ? {} : { storage }),
    ...(rateLimit === undefined ? {} : { rateLimit }),
    ...(options.log === undefined ? {} : { log: options.log }),
    ...(options.preload === undefined ? {} : { preload: options.preload }),
  });
  const external = options.storage;
  const address = options.clientAddress ?? socketAddress;
  const images =
    disk?.sweep ??
    (external !== undefined && isSweepable(external)
      ? external.sweep
      : undefined);
  const { sweepPreload } = handle;
  return {
    handle,
    listener: toNodeListener(handle, (req) => clientKey(address(req))),
    storage,
    sweep:
      images === undefined && sweepPreload === undefined
        ? undefined
        : async () =>
            ((await images?.()) ?? 0) + ((await sweepPreload?.()) ?? 0),
  };
};

/**
 * Production Node proxy. Every download goes through net-guard's
 * `createGuardedFetch` (c-ares resolver, pinned connection, PROXY_POLICY); there
 * is no option to replace it.
 */
export const createNodeProxy = (options: NodeProxyOptions): NodeProxy =>
  buildNodeProxy(options, undefined);
