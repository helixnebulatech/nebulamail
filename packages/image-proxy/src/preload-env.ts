// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `preload` mode from the environment, shared by the Node
// and Vercel adapters. `IMAGE_PRELOAD_SECRET` turns it on (the relay's
// `POST /hooks/preload-urls`); `IMAGE_PRELOAD_FULL_EMAIL=true` also opens
// `POST /hooks/stalwart`, where Stalwart sends the WHOLE email.

import { z } from "zod";

import { MIN_PRELOAD_SECRET_LENGTH } from "./preload/hook";

export const PRELOAD_ENV = {
  /** Bearer token of the relay (and of Stalwart's hook). 32+ characters. */
  IMAGE_PRELOAD_SECRET: z.string().min(MIN_PRELOAD_SECRET_LENGTH).optional(),
  /**
   * "true": accept the whole email from Stalwart's MTA Hook
   * (`POST /hooks/stalwart`). PRIVACY RISK: whoever runs this proxy can
   * read every incoming email. Anything else: off (use the relay).
   */
  IMAGE_PRELOAD_FULL_EMAIL: z
    .enum(["true", "false"])
    .optional()
    .transform((value) => value === "true"),
  /** Hook bodies (the whole email as JSON) over this are not preloaded. */
  IMAGE_PRELOAD_MAX_MESSAGE_BYTES: z.coerce
    .number()
    .int()
    .positive()
    .optional(),
  /**
   * Key of the recipient hashes (quota and opt-out). Default: the secret.
   * Set it before rotating the secret, or quotas and opt-outs are lost.
   */
  IMAGE_PRELOAD_INDEX_KEY: z.string().min(MIN_PRELOAD_SECRET_LENGTH).optional(),
  /** Preloaded bytes per recipient address (default 256 MiB). */
  IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT: z.coerce
    .number()
    .int()
    .positive()
    .max(Number.MAX_SAFE_INTEGER)
    .optional(),
  /** Preloaded images per recipient address (default 10 000). */
  IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT: z.coerce
    .number()
    .int()
    .positive()
    .max(1_000_000)
    .optional(),
  /**
   * "true": add an email token header to preloaded emails, for the static
   * build. Anything else: off.
   */
  IMAGE_PRELOAD_EMAIL_TOKENS: z
    .enum(["true", "false"])
    .optional()
    .transform((value) => value === "true"),
} as const;

export interface PreloadEnv {
  readonly secret: string;
  readonly fullEmail: boolean;
  readonly maxMessageBytes: number | undefined;
  readonly indexKey: string | undefined;
  readonly maxBytesPerRecipient: number | undefined;
  readonly maxImagesPerRecipient: number | undefined;
  readonly emailTokens: boolean;
}

export const preloadEnv = (data: {
  readonly IMAGE_PRELOAD_SECRET?: string | undefined;
  readonly IMAGE_PRELOAD_FULL_EMAIL?: boolean | undefined;
  readonly IMAGE_PRELOAD_MAX_MESSAGE_BYTES?: number | undefined;
  readonly IMAGE_PRELOAD_INDEX_KEY?: string | undefined;
  readonly IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT?: number | undefined;
  readonly IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT?: number | undefined;
  readonly IMAGE_PRELOAD_EMAIL_TOKENS?: boolean | undefined;
}): PreloadEnv | undefined =>
  data.IMAGE_PRELOAD_SECRET === undefined
    ? undefined
    : {
        emailTokens: data.IMAGE_PRELOAD_EMAIL_TOKENS === true,
        fullEmail: data.IMAGE_PRELOAD_FULL_EMAIL === true,
        indexKey: data.IMAGE_PRELOAD_INDEX_KEY,
        maxBytesPerRecipient: data.IMAGE_PRELOAD_MAX_BYTES_PER_RECIPIENT,
        maxImagesPerRecipient: data.IMAGE_PRELOAD_MAX_IMAGES_PER_RECIPIENT,
        maxMessageBytes: data.IMAGE_PRELOAD_MAX_MESSAGE_BYTES,
        secret: data.IMAGE_PRELOAD_SECRET,
      };

/** The optional `PreloadConfig` fields from the environment. */
export const preloadOptions = (
  env: PreloadEnv
): {
  readonly maxMessageBytes?: number;
  readonly indexKey?: string;
  readonly maxBytesPerRecipient?: number;
  readonly maxImagesPerRecipient?: number;
  readonly emailTokens?: boolean;
  readonly fullEmail?: boolean;
} => ({
  ...(env.emailTokens ? { emailTokens: true } : {}),
  ...(env.fullEmail ? { fullEmail: true } : {}),
  ...(env.maxMessageBytes === undefined
    ? {}
    : { maxMessageBytes: env.maxMessageBytes }),
  ...(env.indexKey === undefined ? {} : { indexKey: env.indexKey }),
  ...(env.maxBytesPerRecipient === undefined
    ? {}
    : { maxBytesPerRecipient: env.maxBytesPerRecipient }),
  ...(env.maxImagesPerRecipient === undefined
    ? {}
    : { maxImagesPerRecipient: env.maxImagesPerRecipient }),
});

/** Printed at start by the Node and Vercel adapters when the full-email hook is on. */
export const FULL_EMAIL_WARNING =
  'image proxy: WARNING: IMAGE_PRELOAD_FULL_EMAIL=true. Stalwart sends every incoming email (headers, body, attachments) to this proxy: whoever runs it, the hosting provider included, can read all your mail. Use the preload relay instead (README, "Preload").';
