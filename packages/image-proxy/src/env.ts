// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Environment variables shared by every adapter (Node, Workers, Vercel).
// Errors name the variable, never its value (IMAGE_PROXY_KEYS is a secret).

import { z } from "zod";

export class EnvError extends Error {
  override readonly name = "EnvError";
}

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Largest retention that still fits a safe-integer number of milliseconds
 * (about 285 000 years). Not a policy limit: the admin may set any value.
 */
const MAX_TTL_DAYS = Math.floor(Number.MAX_SAFE_INTEGER / DAY_MS);

/** Variables every adapter reads. Adapters extend this shape. */
export const COMMON_ENV = {
  APP_URL: z.url({ protocol: /^https?$/u }),
  /**
   * Retention of cache and preload modes, in whole days.
   * Default 30; any value from 1 up, no policy maximum.
   */
  IMAGE_CACHE_TTL_DAYS: z.coerce
    .number()
    .int()
    .min(1)
    .max(MAX_TTL_DAYS)
    .optional(),
  IMAGE_PROXY_KEYS: z.string().min(1),
} as const;

export interface CommonEnv {
  /** Raw `IMAGE_PROXY_KEYS`; parse with `parseTicketKeys`. */
  readonly keys: string;
  /** Origin of `APP_URL`. */
  readonly appOrigin: string;
  /** From `IMAGE_CACHE_TTL_DAYS`; undefined = the core's default (30 days). */
  readonly cacheTtlMs: number | undefined;
}

/**
 * Validate `env` with `schema`. Only non-empty strings count as set: empty
 * strings mean "unset" (common in .env files), and Worker bindings (objects)
 * are not variables.
 */
export const parseEnvWith = <T extends z.ZodType>(
  schema: T,
  env: Readonly<Record<string, unknown>>
): z.output<T> => {
  const present = Object.fromEntries(
    Object.entries(env).filter(
      ([, value]) => typeof value === "string" && value !== ""
    )
  );
  const parsed = schema.safeParse(present);
  if (!parsed.success) {
    const names = [
      ...new Set(parsed.error.issues.map((issue) => String(issue.path[0]))),
    ];
    throw new EnvError(`image proxy: invalid or missing ${names.join(", ")}`);
  }
  return parsed.data;
};

export const commonEnv = (data: {
  readonly APP_URL: string;
  readonly IMAGE_PROXY_KEYS: string;
  readonly IMAGE_CACHE_TTL_DAYS?: number | undefined;
}): CommonEnv => ({
  appOrigin: new URL(data.APP_URL).origin,
  cacheTtlMs:
    data.IMAGE_CACHE_TTL_DAYS === undefined
      ? undefined
      : data.IMAGE_CACHE_TTL_DAYS * DAY_MS,
  keys: data.IMAGE_PROXY_KEYS,
});
