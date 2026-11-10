// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The JMAP session through the proxy:
// where the JMAP server says its API, downloads, uploads and push live
// (only accepted on the configured server), and the copy the browser gets,
// with every one of those URLs pointing at the proxy on the app's origin.

import { expandTemplate } from "@nebulamail/jmap-client";

import { JMAP_PROXY_PREFIX } from "../../lib/jmap-proxy";
import type { Upstream } from "./config";
import { toUpstream } from "./config";
import { ProxyError } from "./errors";

/** The JMAP server's endpoints, all on `Upstream.origin`. */
export interface UpstreamEndpoints {
  readonly apiUrl: URL;
  /** RFC 6570 level-1 templates, on the upstream origin. */
  readonly downloadUrl: string;
  readonly uploadUrl: string;
  readonly eventSourceUrl: string;
  /** From the core capability (RFC 8620 §2), for the proxy's own limits. */
  readonly maxSizeUpload: number;
  readonly maxSizeRequest: number;
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** The two origins a URL from the JMAP server may be on. */
const allowedOrigins = (upstream: Upstream): readonly string[] => [
  upstream.origin,
  upstream.publicOrigin,
];

/** An absolute URL of the JMAP server, moved to the upstream origin. */
const checkedUrl = (raw: unknown, base: URL, upstream: Upstream): URL => {
  if (typeof raw !== "string") {
    throw new ProxyError("session", "session: URL missing");
  }
  let url: URL;
  try {
    url = new URL(raw, base);
  } catch {
    throw new ProxyError("session", "session: invalid URL");
  }
  if (
    (url.protocol !== "https:" && url.protocol !== "http:") ||
    url.username !== "" ||
    url.password !== "" ||
    !allowedOrigins(upstream).includes(url.origin)
  ) {
    throw new ProxyError("session", "session: URL on another server");
  }
  return toUpstream(url, upstream);
};

const SAMPLE = "x";

/**
 * Expands a template with the (already checked) values, each encoded by
 * RFC 6570; `undefined` unless the result is a URL on the upstream origin.
 */
export const expandUpstream = (
  template: string,
  values: Readonly<Record<string, string>>,
  upstream: Upstream
): URL | undefined => {
  try {
    const sample = template.replaceAll(/\{\w+\}/gu, SAMPLE);
    if (/[{}]/u.test(sample)) {
      return undefined;
    }
    const url = new URL(expandTemplate(template, values));
    return url.origin === upstream.origin &&
      url.username === "" &&
      url.password === ""
      ? url
      : undefined;
  } catch {
    return undefined;
  }
};

/**
 * A URL template of the JMAP server, on the upstream origin. The origin
 * must be written out literally (one of the two allowed) or the template
 * must be an absolute path: no variable can be part of the host.
 */
export const checkedTemplate = (
  raw: unknown,
  required: readonly string[],
  upstream: Upstream
): string => {
  if (typeof raw !== "string") {
    throw new ProxyError("session", "session: template missing");
  }
  let template: string | undefined = undefined;
  if (raw.startsWith("/") && !raw.startsWith("//")) {
    template = `${upstream.origin}${raw}`;
  } else {
    const origin = allowedOrigins(upstream).find((candidate) =>
      raw.startsWith(`${candidate}/`)
    );
    template =
      origin === undefined
        ? undefined
        : `${upstream.origin}${raw.slice(origin.length)}`;
  }
  if (
    template === undefined ||
    required.some((name) => !template.includes(`{${name}}`))
  ) {
    throw new ProxyError("session", "session: template on another server");
  }
  const expanded = expandUpstream(template, {}, upstream);
  if (expanded === undefined) {
    throw new ProxyError("session", "session: malformed template");
  }
  return template;
};

const positive = (value: unknown, fallback: number): number =>
  typeof value === "number" && Number.isSafeInteger(value) && value > 0
    ? value
    : fallback;

/**
 * The API body is read whole before it goes on: whatever the JMAP server
 * announces, never more than this in memory per request.
 */
export const MAX_PROXIED_REQUEST = 50_000_000;

/** RFC 8620's suggested minimums, if the server says nothing. */
const DEFAULT_MAX_UPLOAD = 50_000_000;
const DEFAULT_MAX_REQUEST = 10_000_000;

/**
 * The endpoints of a session document from the JMAP server. Throws
 * `ProxyError` (`session`) if any URL is not on the configured server.
 */
export const upstreamEndpoints = (
  session: unknown,
  sessionUrl: URL,
  upstream: Upstream
): UpstreamEndpoints => {
  if (!isObject(session) || !isObject(session.capabilities)) {
    throw new ProxyError("session", "session: not a JMAP session");
  }
  const core = session.capabilities["urn:ietf:params:jmap:core"];
  return {
    apiUrl: checkedUrl(session.apiUrl, sessionUrl, upstream),
    downloadUrl: checkedTemplate(
      session.downloadUrl,
      ["accountId", "blobId"],
      upstream
    ),
    eventSourceUrl: checkedTemplate(session.eventSourceUrl, [], upstream),
    maxSizeRequest: Math.min(
      positive(
        isObject(core) ? core.maxSizeRequest : undefined,
        DEFAULT_MAX_REQUEST
      ),
      MAX_PROXIED_REQUEST
    ),
    maxSizeUpload: positive(
      isObject(core) ? core.maxSizeUpload : undefined,
      DEFAULT_MAX_UPLOAD
    ),
    uploadUrl: checkedTemplate(session.uploadUrl, ["accountId"], upstream),
  };
};

/** Capabilities with a URL of their own, which the proxy doesn't carry. */
const UNPROXIED_CAPABILITIES: ReadonlySet<string> = new Set([
  // RFC 8887: the WebSocket URL would go straight to the JMAP server.
  "urn:ietf:params:jmap:websocket",
]);

/**
 * The session the browser gets: the JMAP server's, with its four URLs on
 * the proxy and without the capabilities the proxy doesn't carry. Only
 * call it after `upstreamEndpoints` accepted the same document.
 */
export const browserSession = (
  session: Record<string, unknown>,
  appUrl: URL
): Record<string, unknown> => {
  const base = `${appUrl.origin}${JMAP_PROXY_PREFIX}`;
  const capabilities: Record<string, unknown> = {};
  if (isObject(session.capabilities)) {
    for (const [name, value] of Object.entries(session.capabilities)) {
      if (!UNPROXIED_CAPABILITIES.has(name)) {
        capabilities[name] = value;
      }
    }
  }
  return {
    ...session,
    apiUrl: `${base}api`,
    capabilities,
    downloadUrl: `${base}download/{accountId}/{blobId}/{name}?type={type}`,
    eventSourceUrl: `${base}eventsource?types={types}&closeafter={closeafter}&ping={ping}`,
    uploadUrl: `${base}upload/{accountId}`,
  };
};
