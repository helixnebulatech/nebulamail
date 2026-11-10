// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// `@nebulamail/net-guard/workers`: guarded fetch for runtimes whose `fetch`
// cannot be pinned to an IP (Cloudflare Workers).

export {
  createDohResolver,
  DEFAULT_DOH_ENDPOINT,
  type DohResolverOptions,
} from "./doh";
export { createWorkersFetch, type WorkersFetchOptions } from "./fetch";
export type { WebFetch } from "./stream";
