// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Image ticket config from the environment. Without IMAGE_PROXY_ORIGIN and
// IMAGE_PROXY_KEYS remote images are never loaded.

import {
  MAX_TICKET_TTL_MS,
  imageUrl,
  parseTicketKeys,
  signPreferenceRequest,
  signTicket,
} from "@nebulamail/image-proxy/tickets";
import type { ImageUrlExtractor } from "@nebulamail/mail-render/extract";
import { createWorkerExtractor } from "@nebulamail/mail-render/server-extract";

import { ENV } from "../../env.server";
import { envSetting } from "../env-setting";
import { createRateLimiter } from "../rate-limit";
import type { PreloadPrefsConfig } from "./preload";
import type { ImageTicketConfig, TicketDeps } from "./tickets";

/** Same as the proxy's default `maxImagesPerEmail`. */
const MAX_IMAGES = 60;

const build = async (): Promise<ImageTicketConfig | undefined> => {
  const origin = envSetting(ENV.IMAGE_PROXY_ORIGIN);
  const keysValue = envSetting(ENV.IMAGE_PROXY_KEYS);
  if (origin === undefined || keysValue === undefined) {
    return undefined;
  }
  const proxyOrigin = new URL(origin).origin;
  if (proxyOrigin === new URL(ENV.APP_URL).origin) {
    throw new Error(
      "IMAGE_PROXY_ORIGIN must be a different origin than APP_URL"
    );
  }
  const keys = await parseTicketKeys(keysValue);
  return {
    imageUrl,
    maxImages: MAX_IMAGES,
    proxyOrigin,
    sign: async (claims) => await signTicket(keys, claims),
    ttlMs: MAX_TICKET_TTL_MS,
  };
};

let config: Promise<ImageTicketConfig | undefined> | undefined;

export const imageTicketConfig = async (): Promise<
  ImageTicketConfig | undefined
> => {
  config ??= build();
  return await config;
};

// The email's HTML is parsed with parse5 (the HTML spec, no scripts, loads
// nothing) in worker threads with a deadline: some inputs are quadratic in
// parse5, and on the main thread one email would stop the server.
let extractor: ImageUrlExtractor | undefined;
/**
 * Per user: one call per email opened with images (or contact photo), so 60
 * a minute is plenty, and it caps how much parsing one account can cause.
 */
const ticketLimit = createRateLimiter(60, 60_000);

export const ticketDeps = (): TicketDeps => {
  extractor ??= createWorkerExtractor();
  return {
    extractor,
    fetch: globalThis.fetch.bind(globalThis),
    rateLimit: ticketLimit,
  };
};

let preloadConfig: Promise<PreloadPrefsConfig | undefined> | undefined;

/** Preload opt-out: the same proxy and keys as the tickets. */
export const preloadPrefsConfig = async (): Promise<
  PreloadPrefsConfig | undefined
> => {
  preloadConfig ??= (async () => {
    const images = await imageTicketConfig();
    const keysValue = envSetting(ENV.IMAGE_PROXY_KEYS);
    const keys =
      images === undefined || keysValue === undefined
        ? undefined
        : await parseTicketKeys(keysValue);
    return images === undefined || keys === undefined
      ? undefined
      : {
          proxyOrigin: images.proxyOrigin,
          sign: async (input) => await signPreferenceRequest(keys, input),
        };
  })();
  return await preloadConfig;
};
