// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Vercel environment: the common variables (`../env.ts`, same validation as
// Node) plus the storage choice (Blob or S3), the cron secret and preload.

import { z } from "zod";

import { COMMON_ENV, commonEnv, EnvError, parseEnvWith } from "../env";
import type { CommonEnv } from "../env";
import { PRELOAD_ENV, preloadEnv } from "../preload-env";
import type { PreloadEnv } from "../preload-env";
import { parseS3Env, S3_ENV } from "../s3/env";
import type { S3Env } from "../s3/env";

export interface VercelEnv extends CommonEnv {
  readonly storage: "none" | "vercel-blob" | "s3";
  /** `CRON_SECRET`: Vercel Cron sends it as `Authorization: Bearer …`. */
  readonly cronSecret: string | undefined;
  /** With `storage: "s3"`. */
  readonly s3: S3Env | undefined;
  readonly preload: PreloadEnv | undefined;
}

const Env = z.object({
  ...COMMON_ENV,
  ...S3_ENV,
  ...PRELOAD_ENV,
  BLOB_READ_WRITE_TOKEN: z.string().min(1).optional(),
  BLOB_STORE_ID: z.string().min(1).optional(),
  CRON_SECRET: z.string().min(16).optional(),
  IMAGE_CACHE_STORAGE: z.enum(["none", "vercel-blob", "s3"]).default("none"),
});

export const parseVercelEnv = (
  env: Readonly<Record<string, string | undefined>>
): VercelEnv => {
  const data = parseEnvWith(Env, env);
  const s3 = parseS3Env(env);
  if (data.IMAGE_CACHE_STORAGE === "s3" && s3 === undefined) {
    throw new EnvError(
      "image proxy: IMAGE_CACHE_STORAGE=s3 needs IMAGE_CACHE_S3_BUCKET"
    );
  }
  if (data.IMAGE_CACHE_STORAGE !== "none" && data.CRON_SECRET === undefined) {
    // Without the cron nothing deletes old entries: the retention
    // (IMAGE_CACHE_TTL_DAYS) depends on it, so it is required.
    throw new EnvError(
      `image proxy: ${data.IMAGE_CACHE_STORAGE} storage needs CRON_SECRET`
    );
  }
  const preload = preloadEnv(data);
  if (preload !== undefined && data.IMAGE_CACHE_STORAGE === "none") {
    throw new EnvError(
      "image proxy: IMAGE_PRELOAD_SECRET needs IMAGE_CACHE_STORAGE (vercel-blob or s3)"
    );
  }
  // `@vercel/blob` reads these itself; fail early with a clear message.
  if (
    data.IMAGE_CACHE_STORAGE === "vercel-blob" &&
    data.BLOB_READ_WRITE_TOKEN === undefined &&
    data.BLOB_STORE_ID === undefined
  ) {
    throw new EnvError(
      "image proxy: vercel-blob storage needs BLOB_READ_WRITE_TOKEN or BLOB_STORE_ID"
    );
  }
  return {
    ...commonEnv(data),
    cronSecret: data.CRON_SECRET,
    preload,
    s3: data.IMAGE_CACHE_STORAGE === "s3" ? s3 : undefined,
    storage: data.IMAGE_CACHE_STORAGE,
  };
};
