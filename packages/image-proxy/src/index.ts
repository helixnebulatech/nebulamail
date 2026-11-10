// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Hosting-agnostic image proxy. The core uses only
// Request/Response and WebCrypto; the fetcher (which must enforce the SSRF rules) and the
// storage are injected by an adapter (`@nebulamail/image-proxy/node`).

/**
 * How remote images are cached, from least to most private:
 * - `batch`: no storage; all images of an email in one request, kept in the browser.
 * - `cache`: stored once on the server (like Gmail).
 * - `preload`: fetched when the email arrives, via a Stalwart MTA Hook (like
 *   Proton). See `PreloadConfig` and `preload/hook.ts`.
 */
export type ImageCacheMode = "batch" | "cache" | "preload";

export type RuntimeAdapter = "node" | "workers" | "external" | "off";

export type StorageAdapter = "r2" | "vercel-blob" | "s3" | "disk" | "none";

export {
  DAY_MS,
  DEFAULT_CACHE_TTL_MS,
  DEFAULT_LIMITS,
  ProxyConfigError,
  type FetchedImage,
  type FetchFailure,
  type FetchImage,
  type FetchImageOptions,
  type HtmlParser,
  type ImageUrlExtractor,
  type ImageStorage,
  type PreloadConfig,
  type ProxyConfig,
  type ProxyLimits,
  type ProxyLogEvent,
  type RequestContext,
  type StoredImage,
  type SweepableStorage,
} from "./config";
export type { ImageError } from "./download";
export {
  createProxyHandler,
  IMAGE_CSP,
  type ProxyHandler,
  type BatchImageResult,
  type BatchResponse,
} from "./handler";
export {
  MIN_PRELOAD_SECRET_LENGTH,
  PRELOAD_ACCOUNT,
  preloadKey,
} from "./preload/hook";
export {
  createMemoryStateStore,
  MAX_STATE_BYTES,
  STATE_KEY_PATTERN,
  type PreloadStateStore,
  type SweptStateKind,
} from "./preload/state";
export {
  createRecipientHasher,
  normalizeAddress,
  type RecipientHasher,
} from "./preload/recipients";
export {
  DEFAULT_MAX_BYTES_PER_RECIPIENT,
  DEFAULT_MAX_IMAGES_PER_RECIPIENT,
} from "./preload/usage";
export { STORAGE_KEY_PATTERN } from "./storage-key";
export { fetchImageWith } from "./fetch-image";
export {
  clientKey,
  createRateLimiter,
  DEFAULT_RATE_LIMITS,
  failOpen,
  type BucketRule,
  type RateLimiter,
  type RateLimiterOptions,
  type RateLimitRules,
} from "./rate-limit";
export {
  COMMON_ENV,
  commonEnv,
  EnvError,
  parseEnvWith,
  type CommonEnv,
} from "./env";
export {
  createMemoryStorage,
  type MemoryStorageOptions,
} from "./memory-storage";
