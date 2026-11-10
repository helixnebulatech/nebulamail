// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// POST /api/images/tickets { emailId } or { contactId }
//
// The browser asks to show the images of one email. The BFF downloads that
// email itself with the user's token (so it only ever sees the user's own
// mail), extracts the image URLs with the same rules the browser uses,
// drops trackers, and signs one short-lived ticket per URL. The browser's
// own list is never trusted, so even a logged-in user can't make the proxy
// fetch arbitrary URLs.
//
// `contactId`: the photo of a contact card (remote contact
// photos only through the proxy). Same idea: the BFF reads the card itself
// and signs only its own photo URLs. The ticket's email field is then
// `card-<id>` (the proxy only uses it to keep one batch to one item).
// `accountId` (optional): an email of a folder shared with the user.

import {
  AccountId,
  CAPABILITIES,
  ContactCardId,
  EmailId,
  hasCapability,
} from "@nebulamail/jmap-client";
import type { ContactCard, EmailBodyPart } from "@nebulamail/jmap-client";
import type { ImageUrlExtractor } from "@nebulamail/mail-render/extract";
import { z } from "zod";

import type { AuthConfig } from "../auth/handlers";
import { isSameOrigin, json, withSessionToken } from "../auth/handlers";
import { connectAs } from "../jmap-connect";
import type { RateLimiter } from "../rate-limit";
import { bodyError, readJsonBody } from "../request-body";

export interface TicketClaims {
  readonly accountId: string;
  readonly emailId: string;
  readonly url: string;
  readonly expiresAt: number;
}

export interface ImageTicketConfig {
  /** Origin of the image proxy, e.g. https://img.example.com. */
  readonly proxyOrigin: string;
  readonly sign: (claims: TicketClaims) => Promise<string>;
  readonly imageUrl: (proxyOrigin: string, ticket: string) => string;
  readonly ttlMs: number;
  /** Most images ticketed per email (the proxy enforces it too). */
  readonly maxImages: number;
}

export interface TicketDeps {
  /**
   * `createWorkerExtractor` (`@nebulamail/mail-render/server-extract`): the
   * email's HTML is parsed in a worker thread with a deadline.
   */
  readonly extractor: ImageUrlExtractor;
  readonly fetch: typeof fetch;
  /** Per user (each call downloads and parses one email). */
  readonly rateLimit: RateLimiter;
}

/** Same cap as the browser (mail/api.ts MAX_BODY_BYTES). */
const MAX_BODY_BYTES = 2 * 1024 * 1024;
const MAX_REQUEST_BYTES = 1024;

const Body = z.union([
  z.strictObject({ accountId: AccountId.optional(), emailId: EmailId }),
  z.strictObject({ accountId: AccountId.optional(), contactId: ContactCardId }),
]);

/** Longest URL a ticket carries (the signer refuses longer ones too). */
const MAX_PHOTO_URL = 2048;

/** Remote photo URLs of a card (http/https only; data: needs no proxy). */
export const remotePhotoUrls = (media: ContactCard["media"]): string[] =>
  Object.values(media ?? {}).flatMap((item) => {
    const uri = item.uri?.trim() ?? "";
    if (item.kind.toLowerCase() !== "photo" || uri.length > MAX_PHOTO_URL) {
      return [];
    }
    try {
      const url = new URL(uri);
      return (url.protocol === "https:" || url.protocol === "http:") &&
        url.username === "" &&
        url.password === ""
        ? [url.href]
        : [];
    } catch {
      return [];
    }
  });

export interface TicketedImage {
  /** The URL as it appears (normalized) in the email. */
  readonly url: string;
  readonly ticket: string;
  /** Ready-to-use proxy URL (cache mode). */
  readonly src: string;
}

export interface TicketsResponse {
  readonly proxyOrigin: string;
  readonly expiresAt: number;
  readonly images: readonly TicketedImage[];
  /** Images left out because the email has more than `maxImages`. */
  readonly omitted: number;
  readonly trackersBlocked: number;
}

const joinHtml = (
  parts: readonly EmailBodyPart[],
  values: Readonly<Record<string, { readonly value: string }>>
): string =>
  parts
    .filter((part) => part.type?.toLowerCase() === "text/html")
    .map((part) =>
      part.partId === undefined || part.partId === null
        ? ""
        : (values[part.partId]?.value ?? "")
    )
    .join("");

export const handleImageTickets = async (
  request: Request,
  auth: AuthConfig,
  images: ImageTicketConfig | undefined,
  deps: TicketDeps
): Promise<Response> => {
  if (request.method !== "POST" || !isSameOrigin(request, auth.appUrl)) {
    return json({ error: "forbidden" }, 403);
  }
  if (images === undefined) {
    // No proxy configured: remote images can't be loaded.
    return json({ error: "disabled" }, 404);
  }
  const read = await readJsonBody(request, MAX_REQUEST_BYTES);
  if (!read.ok) {
    return json(bodyError(read.status), read.status);
  }
  const body = Body.safeParse(read.value);
  if (!body.success) {
    return json({ error: "invalid" }, 400);
  }
  return await withSessionToken(request, auth, async (token) => {
    const client = await connectAs(auth, deps.fetch, token.accessToken);
    const { data } = body;
    const accountId =
      data.accountId ??
      client.primaryAccountId(
        "contactId" in data ? CAPABILITIES.contacts : CAPABILITIES.mail
      );
    if (client.session.accounts[accountId] === undefined) {
      return json({ error: "not-found" }, 404);
    }
    if (!deps.rateLimit.take(client.session.username, auth.oidc.now())) {
      return json({ error: "rate-limited" }, 429);
    }
    const found = await (async (): Promise<
      | {
          readonly item: string;
          readonly urls: readonly string[];
          readonly trackers: number;
        }
      | "unparsable"
      | null
    > => {
      if ("contactId" in data) {
        if (!hasCapability(client.session, CAPABILITIES.contacts, accountId)) {
          return null;
        }
        const [cards] = await client.request((b) => [
          b.call("ContactCard/get", {
            accountId,
            ids: [data.contactId],
            properties: ["media"],
          }),
        ]);
        const [card] = cards.list;
        return card === undefined
          ? null
          : {
              item: `card-${data.contactId}`,
              trackers: 0,
              urls: remotePhotoUrls(card.media),
            };
      }
      const [result] = await client.request((b) => [
        b.call("Email/get", {
          accountId,
          bodyProperties: ["partId", "type"],
          fetchHTMLBodyValues: true,
          ids: [data.emailId],
          maxBodyValueBytes: MAX_BODY_BYTES,
          properties: ["htmlBody", "bodyValues"],
        }),
      ]);
      const [email] = result.list;
      if (email === undefined) {
        return null;
      }
      const fromHtml = await deps.extractor.extract(
        joinHtml(email.htmlBody, email.bodyValues)
      );
      if (fromHtml === null) {
        // Not parsed in time or within the limits: no tickets (fail closed).
        return "unparsable";
      }
      return {
        item: data.emailId,
        trackers: fromHtml.trackers.length,
        urls: fromHtml.urls,
      };
    })();
    if (found === null) {
      return json({ error: "not-found" }, 404);
    }
    if (found === "unparsable") {
      return json({ error: "unparsable" }, 422);
    }

    const extracted = found;
    const urls = extracted.urls.slice(0, images.maxImages);
    const expiresAt = auth.oidc.now() + images.ttlMs;
    const signed = await Promise.all(
      urls.map(async (url): Promise<TicketedImage | null> => {
        try {
          const ticket = await images.sign({
            accountId,
            emailId: extracted.item,
            expiresAt,
            url,
          });
          return {
            src: images.imageUrl(images.proxyOrigin, ticket),
            ticket,
            url,
          };
        } catch {
          // e.g. a URL too long for a ticket: that image just stays hidden.
          return null;
        }
      })
    );
    const ticketed = signed.filter((image) => image !== null);
    const response: TicketsResponse = {
      expiresAt,
      images: ticketed,
      omitted: extracted.urls.length - ticketed.length,
      proxyOrigin: images.proxyOrigin,
      trackersBlocked: extracted.trackers,
    };
    return json(response, 200);
  });
};
