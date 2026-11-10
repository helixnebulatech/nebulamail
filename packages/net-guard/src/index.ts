// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// SSRF protection, runtime-agnostic core: works in Node,
// Workers and browsers. The Node adapter with pinned connections lives in
// `@nebulamail/net-guard/node`; the unpinned Workers adapter (DoH) in
// `@nebulamail/net-guard/workers`.

export { LIMITS, type BlockReason } from "./types";
export type {
  FetchFailureReason,
  FetchLimits,
  FetchRequest,
  FetchResult,
  GuardedFetch,
  RedirectMode,
  RequestBody,
} from "./fetch-types";
export { checkAddress, type AddressCheck, type PublicAddress } from "./address";
export { isNonPublicName } from "./host-name";
export {
  HTTPS_POLICY,
  parseTargetUrl,
  PROXY_POLICY,
  type ParsedTarget,
  type ParseResult,
  type Scheme,
  type TargetHost,
  type UrlPolicy,
} from "./url";
export {
  guardUrl,
  resolveAndCheck,
  type DnsAnswer,
  type GuardResult,
  type Resolver,
  type ValidatedTarget,
} from "./resolve";
export {
  allowedImageType,
  checkImage,
  IMAGE_TYPES,
  mediaType,
  sniffImageType,
  type ImageCheck,
  type ImageType,
} from "./image";
export {
  createConcurrencyLimiter,
  createFetchBudget,
  createRateLimiter,
  QueueFullError,
  ruleFor,
  type BucketRule,
  type ConcurrencyLimiter,
  type ConcurrencyLimiterOptions,
  type FetchBudget,
  type FetchBudgetOptions,
  type RateLimiter,
  type RateLimiterOptions,
  type RateLimitRules,
} from "./limits";
