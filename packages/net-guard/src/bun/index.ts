// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// `@nebulamail/net-guard/bun`: guarded fetch for Bun.
// `@nebulamail/net-guard/node` already picks it under Bun; import this one only
// to require Bun explicitly.

export {
  createBunFetch,
  isBun,
  MIN_BUN_VERSION,
  supportedBunVersion,
} from "./fetch";
export type {
  FetchFailureReason,
  FetchLimits,
  FetchRequest,
  FetchResult,
  GuardedFetch,
  RedirectMode,
  RequestBody,
} from "../fetch-types";
export { MAX_REQUEST_BODY_BYTES } from "../http/hops";
export type { GuardedFetchOptions } from "../http/hops";
export {
  cancelableResolver,
  createCancelableResolver,
  systemResolver,
} from "../http/resolver";
export type { CancelableResolverOptions } from "../http/resolver";
