// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Worker environment: the common variables (`../env.ts`, same validation as
// Node) plus the DoH endpoint, the optional R2 binding (or S3 variables),
// and the optional Rate Limiting bindings.

import { z } from "zod";

import { COMMON_ENV, commonEnv, EnvError, parseEnvWith } from "../env";
import type { CommonEnv } from "../env";
import { parseS3Env, S3_ENV } from "../s3/env";
import type { S3Env } from "../s3/env";
import { isR2Bucket } from "./r2-storage";
import type { R2BucketLike } from "./r2-storage";
import { rateLimitBindings } from "./rate-limit";
import type { RateLimitBinding } from "./rate-limit";

/** Name of the R2 binding in `wrangler.toml`. Present = cache mode. */
export const R2_BINDING = "IMAGE_CACHE_BUCKET";

export interface WorkersEnv extends CommonEnv {
  /** `IMAGE_PROXY_DOH_URL`; undefined = Cloudflare's DoH. */
  readonly dohUrl: string | undefined;
  readonly bucket: R2BucketLike | undefined;
  /** S3-compatible storage instead of R2 (`IMAGE_CACHE_S3_*`). */
  readonly s3: S3Env | undefined;
  /** Cloudflare Rate Limiting bindings, by rule (`batch:ip`…). */
  readonly rateLimits: ReadonlyMap<string, RateLimitBinding>;
}

const Env = z.object({
  ...COMMON_ENV,
  ...S3_ENV,
  IMAGE_PROXY_DOH_URL: z.url({ protocol: /^https$/u }).optional(),
});

export const parseWorkersEnv = (
  env: Readonly<Record<string, unknown>>
): WorkersEnv => {
  const data = parseEnvWith(Env, env);
  const bucket: unknown = env[R2_BINDING];
  if (bucket !== undefined && !isR2Bucket(bucket)) {
    throw new EnvError(`image proxy: ${R2_BINDING} is not an R2 binding`);
  }
  const s3 = parseS3Env(env);
  if (bucket !== undefined && s3 !== undefined) {
    throw new EnvError(
      `image proxy: set ${R2_BINDING} or IMAGE_CACHE_S3_BUCKET, not both`
    );
  }
  const { bindings, invalid } = rateLimitBindings(env);
  if (invalid.length > 0) {
    throw new EnvError(
      `image proxy: ${invalid.join(", ")} is not a rate limiting binding`
    );
  }
  return {
    ...commonEnv(data),
    bucket,
    dohUrl: data.IMAGE_PROXY_DOH_URL,
    rateLimits: bindings,
    s3,
  };
};
