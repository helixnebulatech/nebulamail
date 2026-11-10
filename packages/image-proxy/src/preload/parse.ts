// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Reading Stalwart's MTA Hook request, shared by the proxy's
// full-email hook (`hook.ts`) and the relay (`../relay/relay.ts`), so both
// pick exactly the same image URLs. Pure: no storage, no network.

import { z } from "zod";

import type { ImageUrlExtractor } from "../config";
import { EMAIL_TOKEN_HEADER } from "../tickets/email-token";
import { htmlBodies, splitHeaders } from "./mime";

/** Envelope recipients handled per email; more: not preloaded. */
export const MAX_HOOK_RECIPIENTS = 100;

/**
 * Option D: HTML parsed before answering Stalwart (which waits, with a
 * timeout); bigger emails get no token.
 */
export const MAX_TOKEN_HTML_CHARS = 512 * 1024;

/** Never block mail: the only answer Stalwart gets once authenticated. */
export const ACCEPT = Object.freeze({ action: "accept" });

/** The parts of Stalwart's MTA Hook request we read (docs: "MTA Hooks"). */
const HookRequest = z.looseObject({
  context: z.looseObject({ stage: z.string().optional() }).optional(),
  envelope: z
    .looseObject({
      to: z.array(z.looseObject({ address: z.string().max(1024) })).optional(),
    })
    .optional(),
  message: z.looseObject({
    contents: z.string(),
    headers: z.array(z.tuple([z.string(), z.string()])).optional(),
  }),
});

export interface ParsedEmail {
  /** Image URLs of every HTML body: deduplicated, trackers dropped, capped. */
  readonly urls: string[];
  /** HTML bodies that could not be parsed in time (they gave no URLs). */
  readonly unparsable: number;
  /** Envelope recipients, as Stalwart sent them (not normalized). */
  readonly recipients: readonly string[];
  /** The message's own headers, as Stalwart sent them. */
  readonly headers: readonly (readonly [string, string])[];
}

/** What the hook needs from Stalwart's request, before any HTML parsing. */
export interface HookMessage {
  /** Envelope recipients, as Stalwart sent them (not normalized). */
  readonly recipients: readonly string[];
  /** The message's own headers, as Stalwart sent them. */
  readonly headers: readonly (readonly [string, string])[];
  /** Every HTML body, decoded. */
  readonly html: readonly string[];
}

export interface ParseOptions {
  readonly extractor: ImageUrlExtractor;
  readonly isTracker: (url: string) => boolean;
  readonly maxImages: number;
}

/** True for trackers and when the check itself fails (fail closed). */
export const safeIsTracker = (
  isTracker: (url: string) => boolean,
  url: string
): boolean => {
  try {
    return isTracker(url);
  } catch {
    return true;
  }
};

/** Stalwart's request: JSON and MIME only, no HTML parsing yet. */
export const readHookMessage = (text: string): HookMessage | null => {
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch {
    return null;
  }
  const parsed = HookRequest.safeParse(raw);
  if (!parsed.success) {
    return null;
  }
  const stage = parsed.data.context?.stage;
  const recipients = (parsed.data.envelope?.to ?? []).map(
    (rcpt) => rcpt.address
  );
  if (stage !== undefined && stage.toUpperCase() !== "DATA") {
    return { headers: [], html: [], recipients };
  }
  const { contents, headers = [] } = parsed.data.message;
  // Without the header list, `contents` is the whole message.
  const message =
    headers.length === 0 ? splitHeaders(contents) : { body: contents, headers };
  return {
    headers: message.headers,
    html: htmlBodies(message.headers, message.body),
    recipients,
  };
};

/** Too much HTML to parse while Stalwart waits for the answer. */
export const tooMuchHtml = (hook: HookMessage): boolean =>
  hook.html.reduce((sum, html) => sum + html.length, 0) > MAX_TOKEN_HTML_CHARS;

/**
 * The image URLs of an email already read: this is the HTML parsing (in a
 * worker thread on servers). A body that cannot be parsed in time gives no
 * URLs (fail closed). Never rejects.
 */
export const parseHookMessage = async (
  options: ParseOptions,
  hook: HookMessage
): Promise<ParsedEmail> => {
  const urls = new Set<string>();
  let unparsable = 0;
  for (const html of hook.html) {
    let found: readonly string[] | undefined;
    try {
      // One body at a time: the parser's workers are shared.
      // oxlint-disable-next-line no-await-in-loop
      const extracted = await options.extractor.extract(html);
      found = extracted?.urls;
    } catch {
      found = undefined;
    }
    if (found === undefined) {
      unparsable += 1;
    }
    for (const url of found ?? []) {
      if (!safeIsTracker(options.isTracker, url)) {
        urls.add(url);
      }
    }
  }
  return {
    headers: hook.headers,
    recipients: hook.recipients,
    unparsable,
    urls: [...urls].slice(0, options.maxImages),
  };
};

/**
 * Hook modifications: delete every email token header the message already
 * has (anyone can write one into an email), then add ours. As in milter's
 * `smfi_chgheader`, `deleteHeader`'s index is the occurrence of that
 * header name, from 1; the last one first, so the others keep theirs.
 * With `token` null, only the deletions.
 */
export const tokenModifications = (
  headers: readonly (readonly [string, string])[],
  token: string | null
): unknown[] => {
  const count = headers.filter(
    ([name]) => name.toLowerCase() === EMAIL_TOKEN_HEADER.toLowerCase()
  ).length;
  return [
    ...Array.from({ length: count }, (_, i) => ({
      index: count - i,
      name: EMAIL_TOKEN_HEADER,
      type: "deleteHeader",
    })),
    ...(token === null
      ? []
      : [{ name: EMAIL_TOKEN_HEADER, type: "addHeader", value: token }]),
  ];
};

/** `accept`, with the modifications if there are any. */
export const acceptWith = (modifications: readonly unknown[]): unknown =>
  modifications.length === 0 ? ACCEPT : { ...ACCEPT, modifications };
