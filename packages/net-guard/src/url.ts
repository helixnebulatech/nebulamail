// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// URL parsing. Runtime-agnostic: only WHATWG URL.

import { checkAddress } from "./address";
import type { PublicAddress } from "./address";
import type { BlockReason } from "./types";

export type Scheme = "http:" | "https:";

export interface UrlPolicy {
  /** Allowed schemes. Only http/https exist: other schemes are never fetched. */
  readonly schemes: readonly Scheme[];
  /** Allowed ports, after applying the scheme default (80/443). */
  readonly ports: readonly number[];
}

/** Image proxy and other public fetches: http/https on 80/443. */
export const PROXY_POLICY: UrlPolicy = Object.freeze({
  ports: Object.freeze([80, 443]),
  schemes: Object.freeze<Scheme[]>(["http:", "https:"]),
});

/** BFF talking to a JMAP/OIDC server the user typed: HTTPS on 443 only. */
export const HTTPS_POLICY: UrlPolicy = Object.freeze({
  ports: Object.freeze([443]),
  schemes: Object.freeze<Scheme[]>(["https:"]),
});

export type TargetHost =
  | {
      readonly kind: "ip";
      readonly address: PublicAddress;
      readonly family: 4 | 6;
    }
  | { readonly kind: "name"; readonly name: string };

declare const parsedBrand: unique symbol;

/**
 * A URL that passed the static checks. IP literals are already validated;
 * names still need `resolveAndCheck`. Only `parseTargetUrl` produces it.
 */
export interface ParsedTarget {
  /** Normalized URL (WHATWG serialization), without fragment. */
  readonly href: string;
  readonly scheme: Scheme;
  /** `host` as in the URL: name, dotted IPv4 or bracketed IPv6, plus `:port` if not default. */
  readonly authority: string;
  /** URL hostname without IPv6 brackets: what the adapter must be asked to dial. */
  readonly hostname: string;
  /** Path and query, as sent on the request line. */
  readonly path: string;
  readonly port: number;
  readonly host: TargetHost;
  readonly [parsedBrand]: true;
}

export type ParseResult =
  | { readonly ok: true; readonly target: ParsedTarget }
  | { readonly ok: false; readonly reason: BlockReason };

const DEFAULT_PORT: Readonly<Record<Scheme, number>> = {
  "http:": 80,
  "https:": 443,
};

const isScheme = (value: string, policy: UrlPolicy): value is Scheme =>
  (policy.schemes as readonly string[]).includes(value);

const block = (reason: BlockReason): ParseResult => ({ ok: false, reason });

/**
 * Parse and statically check a URL. The host is never checked as text:
 * WHATWG URL parsing normalizes it first (decimal/octal/hex/short IPv4,
 * `127。0。0。1`, IDN → punycode), then IP literals go through `checkAddress`.
 *
 * Checks run in a fixed order (scheme, userinfo, port, IP literal) so the
 * reason is deterministic and matches the Rust crate.
 */
export const parseTargetUrl = (
  input: string | URL,
  policy: UrlPolicy = PROXY_POLICY
): ParseResult => {
  // Re-parse even URL objects: we must not keep a reference the caller can mutate.
  const url = URL.parse(typeof input === "string" ? input : input.href);
  // Unparseable input is "not an http(s) URL".
  if (url === null || !isScheme(url.protocol, policy)) {
    return block("scheme");
  }
  if (url.username !== "" || url.password !== "") {
    return block("userinfo");
  }
  const scheme = url.protocol;
  const port = url.port === "" ? DEFAULT_PORT[scheme] : Number(url.port);
  if (!policy.ports.includes(port)) {
    return block("port");
  }

  const { hostname } = url;
  let host: TargetHost;
  if (hostname.startsWith("[") && hostname.endsWith("]")) {
    const verdict = checkAddress(hostname.slice(1, -1));
    if (!verdict.ok) {
      return block("private-ip");
    }
    host = { address: verdict.address, family: 6, kind: "ip" };
  } else if (/^[\d.]+$/u.test(hostname)) {
    // For http(s) the URL parser turns any numeric host into dotted IPv4,
    // so an all-digits-and-dots hostname here is always an IPv4 literal.
    const verdict = checkAddress(hostname);
    if (!verdict.ok) {
      return block("private-ip");
    }
    host = { address: verdict.address, family: 4, kind: "ip" };
  } else {
    host = { kind: "name", name: hostname };
  }

  url.hash = "";
  // Minting the brand: only this function creates a ParsedTarget.
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion
  const target = {
    authority: url.host,
    host,
    hostname:
      hostname.startsWith("[") && hostname.endsWith("]")
        ? hostname.slice(1, -1)
        : hostname,
    href: url.href,
    path: `${url.pathname}${url.search}`,
    port,
    scheme,
  } as Omit<ParsedTarget, typeof parsedBrand> as ParsedTarget;
  return { ok: true, target: Object.freeze(target) };
};
