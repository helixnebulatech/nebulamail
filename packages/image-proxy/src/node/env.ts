// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Environment of the Node server: the common variables (`../env.ts`) plus
// host, port, the cache storage (disk or S3) and preload.

import { z } from "zod";

import { COMMON_ENV, commonEnv, EnvError, parseEnvWith } from "../env";
import type { CommonEnv } from "../env";
import { PRELOAD_ENV, preloadEnv } from "../preload-env";
import type { PreloadEnv } from "../preload-env";
import { parseS3Env, S3_ENV } from "../s3/env";
import type { S3Env } from "../s3/env";
import { CLIENT_IP_SOURCES } from "./client-ip";
import type { ClientIpSource } from "./client-ip";

export { EnvError } from "../env";

export interface ServerEnv extends CommonEnv {
  readonly host: string;
  readonly port: number;
  /** Where the rate-limit address comes from (`IMAGE_PROXY_CLIENT_IP`). */
  readonly clientIp: ClientIpSource;
  readonly cacheDir: string | undefined;
  /** With `cacheDir`, always set (default `DEFAULT_CACHE_MAX_BYTES`). */
  readonly cacheMaxBytes: number | undefined;
  readonly s3: S3Env | undefined;
  readonly preload: PreloadEnv | undefined;
}

/**
 * Disk cache cap when `IMAGE_CACHE_MAX_BYTES` is not set: there is always
 * one, so a user cannot fill the volume (and with it the preload state).
 */
export const DEFAULT_CACHE_MAX_BYTES = 10 * 2 ** 30;

const Env = z.object({
  ...COMMON_ENV,
  ...S3_ENV,
  ...PRELOAD_ENV,
  IMAGE_CACHE_DIR: z.string().min(1).optional(),
  IMAGE_CACHE_MAX_BYTES: z.coerce.number().int().positive().optional(),
  // Only behind a proxy that sets the header: otherwise anyone fakes any IP.
  IMAGE_PROXY_CLIENT_IP: z.enum(CLIENT_IP_SOURCES).default("socket"),
  IMAGE_PROXY_HOST: z.string().min(1).default("127.0.0.1"),
  IMAGE_PROXY_PORT: z.coerce.number().int().min(0).max(65_535).default(3003),
});

/**
 * Every variable the server reads. `server.ts` reads only these, by name,
 * so Deno can run it with `--allow-env=<this list>` (scripts/start-deno.sh).
 */
export const SERVER_ENV_NAMES: readonly string[] = Object.freeze(
  Object.keys(Env.shape)
);

/** Only the server's variables, read one by one (never enumerating `env`). */
export const pickServerEnv = (
  env: Readonly<Record<string, string | undefined>>
): Record<string, string | undefined> =>
  Object.fromEntries(SERVER_ENV_NAMES.map((name) => [name, env[name]]));

export const parseServerEnv = (
  env: Readonly<Record<string, string | undefined>>
): ServerEnv => {
  const data = parseEnvWith(Env, env);
  const s3 = parseS3Env(env);
  if (s3 !== undefined && data.IMAGE_CACHE_DIR !== undefined) {
    throw new EnvError(
      "image proxy: set IMAGE_CACHE_DIR or IMAGE_CACHE_S3_BUCKET, not both"
    );
  }
  const preload = preloadEnv(data);
  if (
    preload !== undefined &&
    s3 === undefined &&
    data.IMAGE_CACHE_DIR === undefined
  ) {
    throw new EnvError(
      "image proxy: IMAGE_PRELOAD_SECRET needs a cache (IMAGE_CACHE_DIR or IMAGE_CACHE_S3_BUCKET)"
    );
  }
  return {
    ...commonEnv(data),
    cacheDir: data.IMAGE_CACHE_DIR,
    cacheMaxBytes:
      data.IMAGE_CACHE_DIR === undefined
        ? undefined
        : (data.IMAGE_CACHE_MAX_BYTES ?? DEFAULT_CACHE_MAX_BYTES),
    clientIp: data.IMAGE_PROXY_CLIENT_IP,
    host: data.IMAGE_PROXY_HOST,
    port: data.IMAGE_PROXY_PORT,
    preload,
    s3,
  };
};
