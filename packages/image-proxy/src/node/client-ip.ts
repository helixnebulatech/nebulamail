// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Client address for rate limiting, like the bridge's
// `server.client_ip`. Behind a TLS terminator every request comes from the
// proxy's socket, so `batch:ip`/`image:ip` would become one bucket shared by
// everybody; the admin names the header that proxy sets.

import type { IncomingMessage } from "node:http";
import { isIP } from "node:net";

/** Where the client address comes from (`IMAGE_PROXY_CLIENT_IP`). */
export type ClientIpSource = "socket" | "x-forwarded-for" | "x-real-ip";

export const CLIENT_IP_SOURCES: readonly ClientIpSource[] = Object.freeze([
  "socket",
  "x-forwarded-for",
  "x-real-ip",
]);

/** One header value; repeated headers are joined in order, as Node does for XFF. */
const headerText = (
  value: string | string[] | undefined
): string | undefined => (Array.isArray(value) ? value.join(", ") : value);

/** Bracketed IPv6 (`[::1]`) without brackets; anything else as is. */
const bare = (text: string): string =>
  text.startsWith("[") && text.endsWith("]") ? text.slice(1, -1) : text;

const validIp = (text: string | undefined): string | undefined => {
  if (text === undefined) {
    return undefined;
  }
  const candidate = bare(text.trim());
  return isIP(candidate) === 0 ? undefined : candidate;
};

/**
 * The client address of a request. `x-forwarded-for`: the LAST entry of the
 * last header (the one the trusted proxy appended; earlier entries come from
 * the client). `x-real-ip`: the value, if it is one IP. A missing or invalid
 * header falls back to the socket's peer, never `undefined`: a misconfigured
 * proxy must not turn the per-IP limit off.
 */
export const clientAddressFrom =
  (source: ClientIpSource): ((req: IncomingMessage) => string | undefined) =>
  (req) => {
    const socket = req.socket.remoteAddress;
    if (source === "socket") {
      return socket;
    }
    const text = headerText(req.headers[source]);
    const value = source === "x-forwarded-for" ? text?.split(",").at(-1) : text;
    return validIp(value) ?? socket;
  };
