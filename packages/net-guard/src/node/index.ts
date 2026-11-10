// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// `@nebulamail/net-guard/node`: guarded fetch for Node, Deno and Bun.
// Under Bun `createGuardedFetch` returns the Bun adapter.

import { createGuardedFetch } from "./fetch";
import type { GuardedFetch } from "./fetch";

export {
  cancelableResolver,
  createCancelableResolver,
  createGuardedFetch,
  MAX_REQUEST_BODY_BYTES,
  systemResolver,
  type CancelableResolverOptions,
  type FetchFailureReason,
  type FetchLimits,
  type FetchRequest,
  type FetchResult,
  type GuardedFetch,
  type GuardedFetchOptions,
  type RedirectMode,
  type RequestBody,
} from "./fetch";

let shared: GuardedFetch | undefined;

/**
 * Locked-down default: c-ares resolver (also under Bun), `PROXY_POLICY`,
 * limits from `LIMITS`.
 * Built on first use, so an unsupported runtime fails there and not on import.
 */
export const guardedFetch: GuardedFetch = async (input, request) => {
  shared ??= createGuardedFetch();
  return await shared(input, request);
};
