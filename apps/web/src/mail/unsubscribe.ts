// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// List-Unsubscribe (RFC 2369) and one-click (RFC 8058), §8.4.
// Pure and DOM-free: the browser uses it to choose the button, and the BFF
// runs it again on the headers it fetched itself before POSTing anything.
//
// One-click only when all of these hold:
// - an https URL in List-Unsubscribe and exactly
//   `List-Unsubscribe-Post: List-Unsubscribe=One-Click`;
// - our server's topmost Authentication-Results (auth-results.ts) has a
//   `dkim=pass` whose `header.d` is the URL's host or the From domain, or a
//   parent of them (`d=shop.com` covers `news.shop.com`);
// - every DKIM-Signature of that domain (and selector/`b=` prefix, when the
//   server wrote them) signs both headers in `h=` (RFC 8058 §4). "Every"
//   because an unverified copy with the same `d=` could be added by anyone.
// Otherwise: `mailto:` opens the composer, and an https page can be opened in
// a new tab after a warning.

import { z } from "zod";

import { dkimPasses } from "./auth-results";
import type { DkimPass } from "./auth-results";

export interface UnsubscribeOptions {
  /** https URL the BFF may POST to (RFC 8058). */
  readonly oneClick: string | null;
  readonly mailto: string | null;
  /** https page that can be opened in a new tab (the fallback without one-click). */
  readonly web: string | null;
}

export interface UnsubscribeHeaders {
  /** `header:List-Unsubscribe:asURLs`. */
  readonly urls: readonly string[];
  /** `header:List-Unsubscribe-Post:asText`. */
  readonly post: string | null;
  /** Topmost `Authentication-Results` (`:asText:all`, first). */
  readonly authResults: string | null;
  /** Every `DKIM-Signature` (`:asText:all`). */
  readonly dkimSignatures: readonly string[];
  /** From addresses. */
  readonly from: readonly string[];
  /** Our server's hostname (the authserv-id it writes). */
  readonly serverHostname: string;
}

export const ONE_CLICK_BODY = "List-Unsubscribe=One-Click";

/**
 * Email/get properties for the sender check and List-Unsubscribe, read the
 * same way by the browser (api.ts) and the BFF (server/unsubscribe.ts).
 * `:all` returns every instance, top first. The topmost
 * Authentication-Results is the one our own server prepended; without
 * `:all` JMAP returns the last instance, which the sender could have written.
 * The other headers without `:all` are the last instance, the one DKIM signs.
 */
export const UNSUBSCRIBE_PROPERTIES = [
  "from",
  "header:Authentication-Results:asText:all",
  "header:DKIM-Signature:asText:all",
  "header:List-Unsubscribe:asURLs",
  "header:List-Unsubscribe-Post:asText",
] as const;

const Texts = z.array(z.string()).nullable().catch(null);
const Text = z.string().nullable().catch(null);
const From = z
  .array(z.object({ email: z.string() }))
  .nullable()
  .catch(null);

/** Reads UNSUBSCRIBE_PROPERTIES from an Email/get result. */
export const unsubscribeHeaders = (
  email: Readonly<Record<string, unknown>>,
  serverHostname: string
): UnsubscribeHeaders => ({
  authResults:
    Texts.parse(email["header:Authentication-Results:asText:all"])?.[0] ?? null,
  dkimSignatures: Texts.parse(email["header:DKIM-Signature:asText:all"]) ?? [],
  from: (From.parse(email.from) ?? []).map((address) => address.email),
  post: Text.parse(email["header:List-Unsubscribe-Post:asText"]),
  serverHostname,
  urls: Texts.parse(email["header:List-Unsubscribe:asURLs"]) ?? [],
});

const SIGNED_HEADERS = ["list-unsubscribe", "list-unsubscribe-post"];

const parseUrl = (value: string): URL | null => URL.parse(value.trim());

/** Signature tags (RFC 6376 §3.2): `d`, `s`, `b` without whitespace, `h` lowercase. */
export const parseDkimSignature = (
  header: string
): {
  readonly domain: string;
  readonly selector: string;
  readonly signature: string;
  readonly headers: readonly string[];
} | null => {
  const tags = new Map<string, string>();
  for (const part of header.split(";")) {
    const index = part.indexOf("=");
    if (index > 0) {
      tags.set(
        part.slice(0, index).trim().toLowerCase(),
        part.slice(index + 1).replaceAll(/\s+/gu, "")
      );
    }
  }
  const domain = tags.get("d")?.toLowerCase();
  const signed = tags.get("h");
  if (domain === undefined || domain === "" || signed === undefined) {
    return null;
  }
  return {
    domain,
    headers: signed.split(":").map((name) => name.toLowerCase()),
    selector: tags.get("s")?.toLowerCase() ?? "",
    signature: tags.get("b") ?? "",
  };
};

/** `d` is `domain` or a parent of it; never a bare label like `com`. */
export const isAligned = (d: string, domain: string): boolean =>
  d.includes(".") &&
  !d.startsWith(".") &&
  (domain === d || domain.endsWith(`.${d}`));

const domainOf = (address: string): string | null => {
  const at = address.lastIndexOf("@");
  return at === -1 ? null : address.slice(at + 1).toLowerCase();
};

const coversHeaders = (
  pass: DkimPass,
  signatures: readonly string[]
): boolean => {
  const matching = signatures.flatMap((raw) => {
    const parsed = parseDkimSignature(raw);
    return parsed !== null &&
      parsed.domain === pass.domain &&
      (pass.selector === null || parsed.selector === pass.selector) &&
      (pass.signature === null || parsed.signature.startsWith(pass.signature))
      ? [parsed]
      : [];
  });
  return (
    matching.length > 0 &&
    matching.every((signature) =>
      SIGNED_HEADERS.every((name) => signature.headers.includes(name))
    )
  );
};

/** Whether this email may be unsubscribed with a one-click POST to `url`. */
export const oneClickAllowed = (
  url: URL,
  headers: UnsubscribeHeaders
): boolean => {
  if (url.protocol !== "https:" || headers.post?.trim() !== ONE_CLICK_BODY) {
    return false;
  }
  const domains = [
    url.hostname.toLowerCase(),
    ...headers.from.flatMap((address) => domainOf(address) ?? []),
  ];
  return dkimPasses(headers.authResults, headers.serverHostname).some(
    (pass) =>
      domains.some((domain) => isAligned(pass.domain, domain)) &&
      coversHeaders(pass, headers.dkimSignatures)
  );
};

export const unsubscribeOptions = (
  headers: UnsubscribeHeaders
): UnsubscribeOptions => {
  const parsed = headers.urls.flatMap((value) => parseUrl(value) ?? []);
  const https = parsed.find((url) => url.protocol === "https:");
  const mailto = parsed.find((url) => url.protocol === "mailto:");
  const oneClick =
    https !== undefined && oneClickAllowed(https, headers) ? https.href : null;
  return {
    mailto: mailto?.href ?? null,
    oneClick,
    web: https?.href ?? null,
  };
};
