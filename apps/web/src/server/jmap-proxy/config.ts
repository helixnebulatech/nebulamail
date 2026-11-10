// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Configuration of the full BFF mode and
// of the internal URL of the JMAP server. Pure functions: server/auth/
// config.ts reads the environment and the server refuses to start with a
// value that doesn't parse (server.ts).

import { checkAddress, isNonPublicName } from "@nebulamail/net-guard";

export class JmapProxyConfigError extends Error {
  override readonly name = "JmapProxyConfigError";
}

/** JMAP_PROXY: `on` or `off`; empty is off (today's behaviour). */
export const parseJmapProxy = (value: string | undefined): boolean => {
  if (value === undefined || value === "" || value === "off") {
    return false;
  }
  if (value === "on") {
    return true;
  }
  throw new JmapProxyConfigError("JMAP_PROXY must be on or off");
};

/** IP ranges where plain http is acceptable: never leaves the host or LAN. */
const LOCAL_RANGES: ReadonlySet<string> = new Set([
  "loopback",
  "private",
  "unique-local",
  "carrier-grade-nat",
]);

/**
 * Plain http only to a host that can't be on the Internet: a loopback or
 * private IP, or a name that never resolves publicly (`stalwart` in Docker,
 * `*.internal`, `*.lan`, `localhost`…). Link-local (cloud metadata) never.
 */
const isInternalHost = (hostname: string): boolean => {
  const bare =
    hostname.startsWith("[") && hostname.endsWith("]")
      ? hostname.slice(1, -1)
      : hostname;
  const address = checkAddress(bare);
  if (address.ok) {
    // A public IP: not internal.
    return false;
  }
  if (address.range !== "invalid") {
    return LOCAL_RANGES.has(address.range);
  }
  return isNonPublicName(bare);
};

/**
 * Cloud metadata services, never the JMAP server: a link-local IP or one of
 * their names (`metadata.google.internal` is a `.internal` name, and
 * `[IP]` answers over https too). A guard against a mistake
 * in the configuration: the bearer token must never go there.
 */
const METADATA_NAMES: ReadonlySet<string> = new Set([
  "metadata",
  "metadata.google.internal",
  "metadata.goog",
  "instance-data",
  "instance-data.ec2.internal",
]);

const isMetadataHost = (hostname: string): boolean => {
  const bare = (
    hostname.startsWith("[") && hostname.endsWith("]")
      ? hostname.slice(1, -1)
      : hostname
  )
    .toLowerCase()
    .replace(/\.$/u, "");
  const address = checkAddress(bare);
  return (
    (!address.ok && address.range === "link-local") || METADATA_NAMES.has(bare)
  );
};

/**
 * JMAP_INTERNAL_URL: an origin (no path, query, fragment or credentials),
 * https anywhere, http only for internal hosts. `undefined` when empty.
 */
export const parseInternalUrl = (
  value: string | undefined
): URL | undefined => {
  if (value === undefined || value === "") {
    return undefined;
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new JmapProxyConfigError("JMAP_INTERNAL_URL is not a URL");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new JmapProxyConfigError("JMAP_INTERNAL_URL must be http(s)");
  }
  if (url.username !== "" || url.password !== "") {
    throw new JmapProxyConfigError(
      "JMAP_INTERNAL_URL must not contain user:password"
    );
  }
  // `new URL` drops an empty `?` or `#`: check the text too.
  if (url.search !== "" || url.hash !== "" || /[?#]/u.test(value)) {
    throw new JmapProxyConfigError(
      "JMAP_INTERNAL_URL must not have a query or fragment"
    );
  }
  if (url.pathname !== "/") {
    throw new JmapProxyConfigError(
      "JMAP_INTERNAL_URL must be an origin, without a path"
    );
  }
  if (isMetadataHost(url.hostname)) {
    throw new JmapProxyConfigError(
      "JMAP_INTERNAL_URL must not be a link-local address or a cloud metadata service"
    );
  }
  if (url.protocol === "http:" && !isInternalHost(url.hostname)) {
    throw new JmapProxyConfigError(
      "JMAP_INTERNAL_URL: plain http only for loopback, private IPs and internal names; use https"
    );
  }
  return new URL(url.origin);
};

/**
 * Where the server reaches the JMAP server: the public origin
 * (JMAP_SERVER) and, if different, the internal one. URLs on the public
 * origin are moved to the internal one (`toUpstream`).
 */
export interface Upstream {
  /** JMAP_SERVER's origin: what Stalwart writes in its session and metadata. */
  readonly publicOrigin: string;
  /** Where requests really go: JMAP_INTERNAL_URL, else JMAP_SERVER. */
  readonly origin: string;
}

export const makeUpstream = (jmapServer: URL, internal: URL | undefined) => ({
  origin: (internal ?? jmapServer).origin,
  publicOrigin: jmapServer.origin,
});

/**
 * The same URL on the internal origin if it is on the public one; any
 * other URL unchanged.
 */
export const toUpstream = (url: URL, upstream: Upstream): URL => {
  if (url.origin !== upstream.publicOrigin || url.origin === upstream.origin) {
    return url;
  }
  const moved = new URL(upstream.origin);
  moved.pathname = url.pathname;
  moved.search = url.search;
  return moved;
};

/**
 * A fetch for this server's own calls to the JMAP/OAuth server (OIDC
 * discovery, tokens, revocation, server-side JMAP): requests to the public
 * origin go to the internal one. With no internal URL, `base` itself.
 */
export const upstreamFetch = (
  base: typeof fetch,
  upstream: Upstream | undefined
): typeof fetch => {
  if (upstream === undefined || upstream.origin === upstream.publicOrigin) {
    return base;
  }
  const current = upstream;
  const wrapped = async (
    input: Parameters<typeof fetch>[0],
    init?: RequestInit
  ): Promise<Response> => {
    if (input instanceof Request) {
      // Not used by our callers; refuse rather than half-rewrite it.
      throw new TypeError("upstreamFetch takes a URL");
    }
    return await base(toUpstream(new URL(input), current), init);
  };
  return wrapped;
};
