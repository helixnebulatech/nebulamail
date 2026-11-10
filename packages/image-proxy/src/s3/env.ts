// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// S3 storage from the environment, shared by every adapter. Setting
// `IMAGE_CACHE_S3_BUCKET` turns it on; then endpoint and keys are required.
// Errors name variables, never values (the secret key is a secret).

import { z } from "zod";

import { EnvError, parseEnvWith } from "../env";
import type { S3StorageOptions } from "./s3-storage";

export const S3_ENV = {
  IMAGE_CACHE_S3_ACCESS_KEY_ID: z.string().min(1).optional(),
  IMAGE_CACHE_S3_BUCKET: z
    .string()
    .regex(/^[\d.a-z-]{3,63}$/u)
    .optional(),
  IMAGE_CACHE_S3_ENDPOINT: z.url({ protocol: /^https?$/u }).optional(),
  IMAGE_CACHE_S3_REGION: z
    .string()
    .regex(/^[\w-]{1,64}$/u)
    .optional(),
  IMAGE_CACHE_S3_SECRET_ACCESS_KEY: z.string().min(1).optional(),
  IMAGE_CACHE_S3_SESSION_TOKEN: z.string().min(1).optional(),
  IMAGE_CACHE_S3_URL_STYLE: z.enum(["path", "virtual"]).optional(),
} as const;

const Env = z.object(S3_ENV);

/** S3 storage options without retention and size (the adapter adds them). */
export type S3Env = Pick<
  S3StorageOptions,
  "bucket" | "credentials" | "endpoint" | "region" | "urlStyle"
>;

/** `undefined` when `IMAGE_CACHE_S3_BUCKET` is not set. */
export const parseS3Env = (
  env: Readonly<Record<string, unknown>>
): S3Env | undefined => {
  const data = parseEnvWith(Env, env);
  if (data.IMAGE_CACHE_S3_BUCKET === undefined) {
    return undefined;
  }
  const missing = (
    [
      "IMAGE_CACHE_S3_ENDPOINT",
      "IMAGE_CACHE_S3_ACCESS_KEY_ID",
      "IMAGE_CACHE_S3_SECRET_ACCESS_KEY",
    ] as const
  ).filter((name) => data[name] === undefined);
  const {
    IMAGE_CACHE_S3_ACCESS_KEY_ID: accessKeyId,
    IMAGE_CACHE_S3_ENDPOINT: endpoint,
    IMAGE_CACHE_S3_SECRET_ACCESS_KEY: secretAccessKey,
  } = data;
  if (
    missing.length > 0 ||
    accessKeyId === undefined ||
    endpoint === undefined ||
    secretAccessKey === undefined
  ) {
    throw new EnvError(`image proxy: S3 storage needs ${missing.join(", ")}`);
  }
  const origin = new URL(endpoint);
  if (
    origin.pathname !== "/" ||
    origin.search !== "" ||
    origin.username !== ""
  ) {
    throw new EnvError(
      "image proxy: invalid IMAGE_CACHE_S3_ENDPOINT (an origin, without path)"
    );
  }
  return {
    bucket: data.IMAGE_CACHE_S3_BUCKET,
    credentials: {
      accessKeyId,
      secretAccessKey,
      ...(data.IMAGE_CACHE_S3_SESSION_TOKEN === undefined
        ? {}
        : { sessionToken: data.IMAGE_CACHE_S3_SESSION_TOKEN }),
    },
    endpoint: origin.origin,
    region: data.IMAGE_CACHE_S3_REGION ?? "us-east-1",
    urlStyle: data.IMAGE_CACHE_S3_URL_STYLE ?? "path",
  };
};
