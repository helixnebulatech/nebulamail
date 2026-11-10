// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `@nebulamail/image-proxy/workers`: Cloudflare Workers adapter.

export {
  createWorkersProxy,
  type ExecutionContextLike,
  type WorkerEnv,
  type WorkersProxy,
  type WorkersProxyOptions,
} from "./proxy";
export {
  createR2Storage,
  type R2BucketLike,
  type R2ObjectLike,
  type R2StorageOptions,
} from "./r2-storage";
export { parseWorkersEnv, R2_BINDING, type WorkersEnv } from "./env";
export {
  createWorkersRateLimiter,
  RATE_LIMIT_BINDINGS,
  type RateLimitBinding,
} from "./rate-limit";
export { EnvError } from "../env";
