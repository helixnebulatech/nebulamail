// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// This server's own JMAP calls on behalf of a user (image tickets, preload
// preference, one-click unsubscribe). Each request connects with the
// user's token; the redirect from /.well-known/jmap to the real session
// resource (RFC 8620 §2.2) is the same for everyone, so it is remembered
// server-wide for a while and later connections skip that hop, like the
// proxy's endpoints (jmap-proxy/upstream.ts).
//
// Only the URL is kept: never a token or a user's session (each request
// still fetches its own session with its own token). And only a URL on the
// configured server's origin, so the client's origin policy (built from the
// URL it starts at) is exactly the one it would have had.

import { JmapClient, JmapError } from "@nebulamail/jmap-client";

import type { AuthConfig } from "./auth/handlers";
import { upstreamFetch } from "./jmap-proxy/config";

const SESSION_URL_TTL_MS = 10 * 60 * 1000;

interface Remembered {
  /** The configured /.well-known/jmap it was found from. */
  readonly start: string;
  readonly url: string;
  readonly at: number;
}

let remembered: Remembered | undefined;

/** The remembered URL looks gone (moved, or no longer a session). */
const isMoved = (error: unknown): boolean => {
  if (!(error instanceof JmapError)) {
    return false;
  }
  const { detail } = error;
  return (
    detail.kind === "invalidResponse" ||
    ((detail.kind === "http" || detail.kind === "request") &&
      (detail.status === 404 || detail.status === 410))
  );
};

/** A client for the user whose `accessToken` this is. */
export const connectAs = async (
  auth: Pick<AuthConfig, "jmapSessionUrl" | "upstream">,
  fetchImpl: typeof fetch,
  accessToken: string,
  now: () => number = Date.now
): Promise<JmapClient> => {
  const start = auth.jmapSessionUrl;
  const connect = async (sessionUrl: string): Promise<JmapClient> =>
    await JmapClient.connect({
      // JMAP_INTERNAL_URL: to the internal origin, like the proxy.
      fetch: upstreamFetch(fetchImpl, auth.upstream),
      getAccessToken: async () => await Promise.resolve(accessToken),
      sessionUrl,
    });
  const known = remembered;
  if (
    known !== undefined &&
    known.start === start.href &&
    now() - known.at < SESSION_URL_TTL_MS
  ) {
    try {
      return await connect(known.url);
    } catch (error) {
      if (!isMoved(error)) {
        throw error;
      }
      // The server moved its session resource: find it again, once.
      remembered = undefined;
    }
  }
  const client = await connect(start.href);
  const resolved = client.session.sessionUrl;
  if (resolved.origin === start.origin && resolved.href !== start.href) {
    remembered = { at: now(), start: start.href, url: resolved.href };
  }
  return client;
};

/** For tests: a new server. */
export const forgetSessionUrl = (): void => {
  remembered = undefined;
};
