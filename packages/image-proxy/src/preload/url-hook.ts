// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `POST /hooks/preload-urls`: what the relay sends to the
// proxy instead of the whole email. The relay runs next to Stalwart, reads
// the email there and sends only:
//
//   { "v": 1, "urls": [...], "recipients": [...], "token": true? }
//
// - `urls`: the email's image URLs (trackers already dropped; the proxy
//   checks them again).
// - `recipients`: HMAC-SHA256 hashes of the normalized envelope recipients
//   (`recipients.ts`, the same key as the proxy: `IMAGE_PRELOAD_INDEX_KEY`,
//   default `IMAGE_PRELOAD_SECRET`). The proxy never sees an address here.
// - `token`: email tokens on (static build): the proxy answers with one.
//
// Answer: `{ "token": "1.…" }` when one was asked for and the email is
// preloaded, `{}` otherwise. Auth: `Authorization: Bearer
// <IMAGE_PRELOAD_SECRET>`.

import { z } from "zod";

import { MAX_HOOK_RECIPIENTS } from "./parse";

export const URL_HOOK_PATH = "/hooks/preload-urls";

/** URLs in one request; the proxy keeps the first `maxImagesPerEmail`. */
export const MAX_HOOK_URLS = 500;
export const MAX_HOOK_URL_LENGTH = 8192;
/** Plenty for 500 long URLs and 100 hashes. */
export const MAX_URL_HOOK_BODY_BYTES = 5 * 1024 * 1024;

export const UrlHookBody = z.strictObject({
  recipients: z
    .array(z.string().regex(/^[0-9a-f]{64}$/u))
    .min(1)
    .max(MAX_HOOK_RECIPIENTS),
  token: z.boolean().optional(),
  urls: z.array(z.string().min(1).max(MAX_HOOK_URL_LENGTH)).max(MAX_HOOK_URLS),
  v: z.literal(1),
});
export type UrlHookRequest = z.infer<typeof UrlHookBody>;

export const UrlHookAnswer = z.looseObject({
  token: z.string().max(512).optional(),
});

/** An absolute http(s) URL without credentials, as the extractor gives. */
export const isImageUrl = (value: string): boolean => {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  return (
    (url.protocol === "https:" || url.protocol === "http:") &&
    url.username === "" &&
    url.password === ""
  );
};
