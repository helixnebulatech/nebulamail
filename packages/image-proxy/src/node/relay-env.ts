// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Environment of the preload relay (`relay-server.ts`, README "Preload ›
// Relay"). Read by name, like the proxy's (`pickRelayEnv`).

import { z } from "zod";

import { parseEnvWith } from "../env";
import { MIN_PRELOAD_SECRET_LENGTH } from "../preload/hook";

const Env = z.object({
  /** "true": ask the proxy for email token headers (static build). */
  IMAGE_PRELOAD_EMAIL_TOKENS: z
    .enum(["true", "false"])
    .optional()
    .transform((value) => value === "true"),
  /** The proxy's index key, if it has one (default: the proxy's secret). */
  IMAGE_PRELOAD_INDEX_KEY: z.string().min(MIN_PRELOAD_SECRET_LENGTH).optional(),
  IMAGE_PRELOAD_MAX_MESSAGE_BYTES: z.coerce
    .number()
    .int()
    .positive()
    .optional(),
  /** Loopback by default: only Stalwart on this machine should reach it. */
  IMAGE_PRELOAD_RELAY_HOST: z.string().min(1).default("127.0.0.1"),
  IMAGE_PRELOAD_RELAY_PORT: z.coerce
    .number()
    .int()
    .min(0)
    .max(65_535)
    .default(3004),
  /** Bearer token Stalwart's MTA Hook sends to the relay. */
  IMAGE_PRELOAD_RELAY_SECRET: z.string().min(MIN_PRELOAD_SECRET_LENGTH),
  /** The proxy's `IMAGE_PRELOAD_SECRET` (relay → proxy). */
  IMAGE_PRELOAD_SECRET: z.string().min(MIN_PRELOAD_SECRET_LENGTH),
  /** The proxy's base URL, e.g. https://img.example.com. */
  IMAGE_PRELOAD_UPSTREAM: z.url({ protocol: /^https?$/u }),
});

export const RELAY_ENV_NAMES: readonly string[] = Object.freeze(
  Object.keys(Env.shape)
);

export interface RelayEnv {
  readonly host: string;
  readonly port: number;
  readonly hookSecret: string;
  readonly upstream: string;
  readonly upstreamSecret: string;
  readonly indexKey: string | undefined;
  readonly emailTokens: boolean;
  readonly maxMessageBytes: number | undefined;
}

/** Only the relay's variables, read one by one (never enumerating `env`). */
export const pickRelayEnv = (
  env: Readonly<Record<string, string | undefined>>
): Record<string, string | undefined> =>
  Object.fromEntries(RELAY_ENV_NAMES.map((name) => [name, env[name]]));

export const parseRelayEnv = (
  env: Readonly<Record<string, string | undefined>>
): RelayEnv => {
  const data = parseEnvWith(Env, env);
  return {
    emailTokens: data.IMAGE_PRELOAD_EMAIL_TOKENS,
    hookSecret: data.IMAGE_PRELOAD_RELAY_SECRET,
    host: data.IMAGE_PRELOAD_RELAY_HOST,
    indexKey: data.IMAGE_PRELOAD_INDEX_KEY,
    maxMessageBytes: data.IMAGE_PRELOAD_MAX_MESSAGE_BYTES,
    port: data.IMAGE_PRELOAD_RELAY_PORT,
    upstream: data.IMAGE_PRELOAD_UPSTREAM,
    upstreamSecret: data.IMAGE_PRELOAD_SECRET,
  };
};
