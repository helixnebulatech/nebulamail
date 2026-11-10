// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `@nebulamail/image-proxy/vercel`: Vercel Functions adapter, Node.js runtime
// only.

export {
  createVercelProxy,
  VERCEL_LIMITS,
  type VercelProxy,
  type VercelProxyOptions,
} from "./proxy";
export {
  createBlobStorage,
  type BlobGetResult,
  type BlobStorageOptions,
  type VercelBlobApi,
} from "./blob-storage";
export { createBlobStateStore } from "./blob-state";
export { parseVercelEnv, type VercelEnv } from "./env";
export { EnvError } from "../env";
