// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Images of an open email, in the browser:
// - `cid:` images: downloaded with the token, given to mail-render as
//   `data:` URLs (never fetched by the iframe).
// - Remote images, only after "Mostrar imágenes": the BFF signs tickets for
//   the email's own images, then the proxy fetches all of them at once
//   (cache mode) or returns them in one response (batch mode → `blob:`).

import { normalizeContentId } from "@nebulamail/mail-render";
import { z } from "zod";

import type { MailAccount } from "./account-scope";
import type { InlinePart } from "./api";
import { getClient } from "./client";
import { RASTER } from "./inline";

/** Per image and per email, like the proxy's budgets. */
export const MAX_INLINE_BYTES = 5 * 1024 * 1024;
const MAX_INLINE_TOTAL = 10 * 1024 * 1024;

/**
 * Images the user can put in the body of a message: the
 * types every mail client shows, within what the editor and the reader
 * show inline (MAX_INLINE_BYTES).
 */
export const BODY_IMAGE_TYPES = [
  "image/png",
  "image/jpeg",
  "image/gif",
  "image/webp",
] as const;

export const canInsertImage = (file: {
  readonly type: string;
  readonly size: number;
}): boolean =>
  BODY_IMAGE_TYPES.some((type) => type === file.type) &&
  file.size > 0 &&
  file.size <= MAX_INLINE_BYTES;

/** A `data:` URL of an image (labelled with the blob's own type). */
export const toDataUrl = async (blob: Blob): Promise<string> => {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  const chunk = 0x80_00;
  for (let index = 0; index < bytes.length; index += chunk) {
    binary += String.fromCodePoint(...bytes.subarray(index, index + chunk));
  }
  return `data:${blob.type};base64,${btoa(binary)}`;
};

/**
 * The inline images to download for `needed` (Content-IDs from
 * mail-render): raster types only (never SVG), each under the per-image
 * limit, in order while they fit the email's total. One that doesn't fit
 * is skipped and a smaller one after it may still fit.
 */
export const inlineToLoad = (
  parts: readonly InlinePart[],
  needed: readonly string[]
): InlinePart[] => {
  // Content-IDs match like mail-render's (lower case, no <>).
  const wanted = new Set(needed.map(normalizeContentId));
  const chosen: InlinePart[] = [];
  let total = 0;
  for (const part of parts) {
    if (
      wanted.has(normalizeContentId(part.cid)) &&
      RASTER.test(part.type.toLowerCase()) &&
      part.size <= MAX_INLINE_BYTES &&
      total + part.size <= MAX_INLINE_TOTAL
    ) {
      total += part.size;
      chosen.push(part);
    }
  }
  return chosen;
};

/**
 * Downloads the inline images the HTML needs (`neededCids` from
 * mail-render) as `data:` URLs, keyed by normalized Content-ID. What
 * `inlineToLoad` leaves out stays hidden.
 */
export const loadInlineImages = async (
  parts: readonly InlinePart[],
  needed: readonly string[]
): Promise<ReadonlyMap<string, string>> => {
  const client = await getClient();
  const primary = client.primaryAccountId();
  const result = new Map<string, string>();
  for (const part of inlineToLoad(parts, needed)) {
    const type = part.type.toLowerCase();
    // oxlint-disable-next-line no-await-in-loop -- one at a time keeps memory and the total budget in check.
    const blob = await client.downloadBlob({
      // Emails of shared folders are in another account.
      accountId: part.accountId ?? primary,
      blobId: part.blobId,
      maxBytes: MAX_INLINE_BYTES,
      name: part.name === "" ? "image" : part.name,
      type,
    });
    // The server's Content-Type isn't trusted: label it with the part's type.
    // oxlint-disable-next-line no-await-in-loop -- see above.
    const url = await toDataUrl(new Blob([blob], { type }));
    result.set(normalizeContentId(part.cid), url);
  }
  return result;
};

const TicketsResponse = z.object({
  images: z.array(
    z.object({ src: z.url(), ticket: z.string(), url: z.string() })
  ),
  proxyOrigin: z.url(),
});

/** What the proxy sends inline: a raster type and plain base64. */
const INLINE_TYPE = /^image\/(?:png|jpeg|gif|webp|avif|bmp|x-icon)$/u;
const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/u;

const BatchResponse = z.object({
  images: z.record(
    z.string(),
    z.union([
      z.object({ cached: z.literal(true) }),
      z.object({ data: z.string(), type: z.string() }),
      z.object({ error: z.string() }),
    ])
  ),
});

export type RemoteImages =
  | {
      readonly kind: "proxy";
      readonly origin: string;
      /** Original URL in the email → proxy URL. */
      readonly urls: ReadonlyMap<string, string>;
    }
  | {
      readonly kind: "batch";
      /** Original URL → `blob:` URL (revoke when the email closes). */
      readonly urls: ReadonlyMap<string, string>;
      readonly revoke: () => void;
    }
  /**
   * Straight from the sender (static build):
   * the email is shown in the page of `DIRECT_FRAME_PAGE`.
   */
  | { readonly kind: "direct" }
  /** The proxy isn't configured: remote images can't be shown. */
  | { readonly kind: "unavailable" };

const fromBase64 = (data: string, type: string): Blob =>
  new Blob([Uint8Array.from(atob(data), (char) => char.codePointAt(0) ?? 0)], {
    type,
  });

/**
 * Proxy origins that answered 400 to `mode: "cache"` and then served the
 * batch: they have no storage, for the life of the page.
 */
const withoutStorage = new Set<string>();

/** For tests: forget what the proxies said. */
export const forgetProxyModes = (): void => {
  withoutStorage.clear();
};

const postBatch = async (
  origin: string,
  tickets: readonly string[],
  mode: "cache" | "batch"
): Promise<Response> =>
  await fetch(`${origin}/batch`, {
    body: JSON.stringify({ mode, tickets }),
    credentials: "omit",
    headers: { "content-type": "application/json" },
    method: "POST",
    referrerPolicy: "no-referrer",
  });

const loadThroughBff = async (
  item: Readonly<Record<string, string>>
): Promise<RemoteImages> => {
  const ticketResponse = await fetch("/api/images/tickets", {
    body: JSON.stringify(item),
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  if (ticketResponse.status === 404) {
    return { kind: "unavailable" };
  }
  if (!ticketResponse.ok) {
    throw new Error(`Image tickets failed: HTTP ${ticketResponse.status}`);
  }
  const { images, proxyOrigin } = TicketsResponse.parse(
    await ticketResponse.json()
  );
  const { origin } = new URL(proxyOrigin);
  if (images.length === 0) {
    return { kind: "proxy", origin, urls: new Map() };
  }
  const tickets = images.map((image) => image.ticket);
  let mode: "cache" | "batch" = withoutStorage.has(origin) ? "batch" : "cache";
  let response = await postBatch(origin, tickets, mode);
  if (mode === "cache" && response.status === 400) {
    // The proxy has no storage: everything comes back in one response.
    response = await postBatch(origin, tickets, "batch");
    mode = "batch";
    if (response.ok) {
      // Don't ask it for "cache" again (one wasted request per email).
      // Batch goes through the same proxy and stores nothing, so it is
      // never less private than cache.
      withoutStorage.add(origin);
    }
  }
  if (!response.ok) {
    throw new Error(`Image proxy failed: HTTP ${response.status}`);
  }
  const { images: results } = BatchResponse.parse(await response.json());
  const urls = new Map<string, string>();
  const created: string[] = [];
  for (const image of images) {
    const result = results[image.ticket];
    if (result !== undefined && "cached" in result) {
      urls.set(image.url, image.src);
    } else if (result !== undefined && "data" in result && mode === "cache") {
      // Not kept by the proxy (the sender said no-store and the like, RFC
      // 9111): it comes inline. A `data:` URL, since the proxy mode's frame
      // only allows the proxy origin and `data:` (mail-render accepts
      // raster `data:` images only). Anything else is dropped.
      if (INLINE_TYPE.test(result.type) && BASE64.test(result.data)) {
        urls.set(image.url, `data:${result.type};base64,${result.data}`);
      }
    } else if (result !== undefined && "data" in result) {
      const blobUrl = URL.createObjectURL(fromBase64(result.data, result.type));
      created.push(blobUrl);
      urls.set(image.url, blobUrl);
    }
  }
  return mode === "cache"
    ? { kind: "proxy", origin, urls }
    : {
        kind: "batch",
        revoke: () => {
          for (const url of created) {
            URL.revokeObjectURL(url);
          }
        },
        urls,
      };
};

/**
 * The static build has no BFF to sign tickets: no proxy
 * there (preloaded images come next); the reader loads images straight
 * from the sender when the user chose so (mail/remote-images.ts).
 */
const loadTicketed: typeof loadThroughBff = import.meta.env.NEBULA_STATIC
  ? async () => await Promise.resolve({ kind: "unavailable" })
  : loadThroughBff;

export const loadRemoteImages = async (
  emailId: string,
  account: MailAccount = null
): Promise<RemoteImages> =>
  await loadTicketed(
    account === null ? { emailId } : { accountId: account, emailId }
  );

/**
 * A contact's remote photo, the same way: the BFF reads the
 * card and signs only its own photo URLs; the proxy fetches them.
 */
export const loadContactPhotos = async (
  contactId: string,
  /** A shared address book's account; null: ours. */
  accountId: string | null = null
): Promise<RemoteImages> =>
  await loadTicketed(
    accountId === null ? { contactId } : { accountId, contactId }
  );
