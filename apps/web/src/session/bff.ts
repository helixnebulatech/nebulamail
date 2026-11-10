// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Browser side of the login with our server: the access token
// lives only in memory and comes from the BFF (/auth/token), which keeps the
// refresh token in an HttpOnly cookie.

// zod-config first: this module can be bundled into a chunk that runs
// before router.tsx (and its schemas would make zod probe `new Function`).
import "../lib/zod-config";
import { createServerFn } from "@tanstack/react-start";
import { getRequest } from "@tanstack/react-start/server";

import {
  JMAP_PROXY_FOREGROUND,
  JMAP_PROXY_HEADER,
  JMAP_PROXY_PREFIX,
  JMAP_PROXY_SESSION_PATH,
  JMAP_PROXY_TOKEN_ERROR,
} from "../lib/jmap-proxy";
import {
  GRANT_MARGIN_MS,
  GrantSchema,
  ProxyGrantSchema,
  SessionExpiredError,
} from "./types";
import type { Grant, LoginCompletion, SessionApi } from "./types";

export { SessionExpiredError } from "./types";
export type { Grant } from "./types";

/** Runs on the server during SSR and as an RPC from the browser. */
export const getAuthState = createServerFn({ method: "GET" }).handler(
  async () => {
    const [{ authConfig }, { hasSession }] = await Promise.all([
      import("../server/auth/config"),
      import("../server/auth/handlers"),
    ]);
    return {
      authenticated: await hasSession(getRequest(), await authConfig()),
    };
  }
);

let current: Grant | undefined;
let pending: Promise<Grant> | undefined;
/**
 * The server runs the full BFF mode (JMAP_PROXY=on): /auth/token said so
 * with a 404, and from then on the grant comes from the proxy.
 */
let viaProxy = false;

/**
 * The page's server render says it runs the full BFF mode (router.tsx
 * `hydrate`): the first grant goes straight to the proxy, without the
 * /auth/token 404 round trip. Only which endpoint is asked first: both
 * check the session cookie.
 */
export const setJmapProxyMode = (on: boolean): void => {
  viaProxy = on;
};

/**
 * In the full BFF mode the JMAP session is always the proxy's, on this
 * origin (the grant says the same): mail/client.ts asks for it alongside
 * the grant instead of after it. Null when the mode isn't known yet.
 */
export const proxySessionUrl = (): string | null =>
  viaProxy
    ? new URL(JMAP_PROXY_SESSION_PATH, globalThis.location.origin).href
    : null;

/** Full BFF mode: no token, the session cookie talks to /jmap-proxy/. */
const requestProxyGrant = async (): Promise<Grant> => {
  const response = await fetch(`${JMAP_PROXY_PREFIX}grant`, {
    credentials: "same-origin",
    headers: { [JMAP_PROXY_HEADER]: JMAP_PROXY_FOREGROUND },
  });
  if (response.status === 401) {
    current = undefined;
    throw new SessionExpiredError("Session expired");
  }
  if (!response.ok) {
    throw new Error(`Grant request failed: HTTP ${response.status}`);
  }
  const grant = ProxyGrantSchema.parse(await response.json());
  // The cookie is the credential: the session must be on this origin.
  if (new URL(grant.sessionUrl).origin !== globalThis.location.origin) {
    throw new Error("The JMAP proxy must be on the page's origin");
  }
  current = grant;
  return current;
};

const isProxyAnswer = async (response: Response): Promise<boolean> => {
  const body: unknown = await response.json().catch(() => null);
  return (
    typeof body === "object" &&
    body !== null &&
    "error" in body &&
    body.error === JMAP_PROXY_TOKEN_ERROR
  );
};

const requestGrant = async (forceRefresh: boolean): Promise<Grant> => {
  if (viaProxy) {
    return await requestProxyGrant();
  }
  const response = await fetch(
    forceRefresh ? "/auth/token?refresh=1" : "/auth/token",
    {
      credentials: "same-origin",
      method: "POST",
    }
  );
  if (response.status === 401) {
    current = undefined;
    throw new SessionExpiredError("Session expired");
  }
  if (response.status === 404 && (await isProxyAnswer(response))) {
    viaProxy = true;
    return await requestProxyGrant();
  }
  if (!response.ok) {
    throw new Error(`Token request failed: HTTP ${response.status}`);
  }
  current = GrantSchema.parse(await response.json());
  return current;
};

/**
 * Returns a valid access token grant. Concurrent callers share one request.
 * `forceRefresh` after JMAP answered 401 with a token we thought was valid.
 */
export const getGrant = async (
  options: { readonly forceRefresh?: boolean } = {}
): Promise<Grant> => {
  const forceRefresh = options.forceRefresh === true;
  if (
    !forceRefresh &&
    current !== undefined &&
    current.expiresAt - Date.now() > GRANT_MARGIN_MS
  ) {
    return current;
  }
  pending ??= requestGrant(forceRefresh).finally(() => {
    pending = undefined;
  });
  return await pending;
};

export const logout = async (): Promise<void> => {
  current = undefined;
  await fetch("/auth/logout", { credentials: "same-origin", method: "POST" });
};

/** The server does the whole login: /auth/login → provider → /auth/callback. */
export const beginLogin = async (
  returnTo: string | undefined
): Promise<void> => {
  const url = new URL("/auth/login", globalThis.location.origin);
  if (returnTo !== undefined) {
    url.searchParams.set("returnTo", returnTo);
  }
  globalThis.location.assign(url);
  await Promise.resolve();
};

/** The callback is /auth/callback on the server: nothing to do here. */
export const completeLogin = async (
  _search: string
): Promise<LoginCompletion> => await Promise.resolve({ kind: "none" });

export const session = {
  beginLogin,
  completeLogin,
  getAuthState,
  getGrant,
  logout,
} satisfies SessionApi;
