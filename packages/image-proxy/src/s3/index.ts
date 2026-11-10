// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `@nebulamail/image-proxy/s3`: S3-compatible storage (AWS, MinIO, Garage…),
// any runtime.

export {
  createS3StateStore,
  createS3Storage,
  parseListing,
  S3Error,
  type S3Fetch,
  type S3StorageOptions,
} from "./s3-storage";
export { parseS3Env, S3_ENV, type S3Env } from "./env";
export {
  EMPTY_SHA256,
  sha256Hex,
  signRequest,
  type S3Credentials,
  type SignInput,
  type Signed,
} from "./sigv4";
