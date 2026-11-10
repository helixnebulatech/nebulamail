// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One JMAP connection per browser tab, with the short token from the BFF.
// Only the domain functions in this folder use it: the UI never talks JMAP
// directly.

import type { FetchLike } from "@nebulamail/jmap-client";
import { JmapClient } from "@nebulamail/jmap-client";

import { getGrant, proxySessionUrl } from "../lib/auth";
import {
  JMAP_PROXY_CLIENT_HEADER,
  JMAP_PROXY_FOREGROUND,
  JMAP_PROXY_HEADER,
} from "../lib/jmap-proxy";
import { getLocale } from "../paraglide/runtime.js";

let connection: Promise<JmapClient> | undefined;

/** The server's public name, when the session points at the proxy. */
const publicHostnames = new WeakMap<JmapClient, string>();

/**
 * The JMAP server's public host name: the name Stalwart writes in its
 * Authentication-Results. In full BFF mode the session points at the proxy
 * on this origin, so the grant says it.
 */
export const serverHostname = (client: JmapClient): string =>
  publicHostnames.get(client) ?? client.session.apiUrl.hostname;

/**
 * Every JMAP request says the interface language (Settings, else the
 * browser's): the bridge writes invitation emails in it.
 * Stalwart ignores it.
 */
export const fetchWithLanguage =
  (language: () => string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init.headers);
    headers.set("Accept-Language", language());
    return await globalThis.fetch(input, { ...init, headers });
  };

/**
 * Full BFF mode: no token in the page. JMAP goes to the
 * proxy on this origin with the session cookie and its anti-CSRF header;
 * uploads with progress use XHR there (same origin).
 */
const connectToProxy = async (sessionUrl: string): Promise<JmapClient> =>
  await JmapClient.connect({
    fetch: fetchWithLanguage(getLocale),
    headers: {
      [JMAP_PROXY_CLIENT_HEADER]: crypto.randomUUID(),
      [JMAP_PROXY_HEADER]: JMAP_PROXY_FOREGROUND,
    },
    sameOriginCookies: true,
    sessionUrl,
    xhr: {
      create: () => new XMLHttpRequest(),
      pageOrigin: globalThis.location.origin,
    },
  });

const connect = async (): Promise<JmapClient> => {
  // The page already knows it runs the proxy (router.tsx `hydrate`): its
  // session URL is fixed, so the session comes alongside the grant, not
  // one round trip after it. Both ask with the same cookie; the grant
  // still decides (and its failure, an expired session, wins).
  const guess = proxySessionUrl();
  const early = guess === null ? null : connectToProxy(guess);
  early?.catch(() => null);
  const grant = await getGrant();
  if (grant.jmapProxy === true) {
    const client =
      early !== null && grant.sessionUrl === guess
        ? await early
        : await connectToProxy(grant.sessionUrl);
    if (grant.serverHostname !== undefined) {
      publicHostnames.set(client, grant.serverHostname);
    }
    return client;
  }
  return await JmapClient.connect({
    fetch: fetchWithLanguage(getLocale),
    getAccessToken: async ({ forceRefresh }) => {
      const next = await getGrant({ forceRefresh });
      if (next.jmapProxy === true) {
        // The server switched to the proxy under this page: start over.
        throw new Error("The server now uses the JMAP proxy");
      }
      return next.accessToken;
    },
    sessionUrl: grant.sessionUrl,
    ...(grant.serverAliases === undefined
      ? {}
      : { serverAliases: grant.serverAliases }),
    // Attachment uploads with a progress bar, only to
    // another origin: on the page's own origin XHR would send its cookies.
    xhr: {
      create: () => new XMLHttpRequest(),
      pageOrigin: globalThis.location.origin,
    },
  });
};

/** When each client fetched its session by connecting. */
const connectedAt = new WeakMap<JmapClient, number>();

/** How long ago `client` connected (and fetched its session), in ms. */
export const connectedFor = (client: JmapClient): number =>
  Date.now() - (connectedAt.get(client) ?? 0);

export const getClient = async (): Promise<JmapClient> => {
  connection ??= connect().then((client) => {
    connectedAt.set(client, Date.now());
    return client;
  });
  try {
    return await connection;
  } catch (error) {
    // Let the next call try again instead of keeping the failure.
    connection = undefined;
    throw error;
  }
};

/** After logout: the next user must not reuse this session. */
export const resetClient = (): void => {
  connection = undefined;
};
