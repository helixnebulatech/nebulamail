// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The BFF login. Plain `Request` → `Response` functions, so they
// run on any hosting and are tested without a server.
//
//   GET  /auth/login?returnTo=/inbox  → redirect to the OIDC provider
//   GET  /auth/callback               → exchange the code, set the session cookie
//   POST /auth/token                  → short access token for JMAP (memory only)
//   POST /auth/logout                 → clear the session cookie
//
// The refresh token never leaves the server: it lives encrypted in an
// HttpOnly cookie. Since 1.0, Stalwart rotates it on
// every refresh and ends the whole grant when a superseded one comes back
// (reuse detection): that is why the cookie is only rewritten when something
// changed (`sessionAccessToken`), never with a copy read before a refresh.

import { z } from "zod";

import { endsSession } from "../../lib/oauth-errors";
import { safeReturnTo } from "../../lib/return-to";
import type { AdminImagePolicy } from "../../mail/image-policy";
import type { Upstream } from "../jmap-proxy/config";
import {
  LOGIN_COOKIE,
  PROXY_COOKIE,
  SESSION_COOKIE,
  cookieName,
  clearCookie,
  parseCookies,
  serializeCookie,
} from "./cookies";
import type { OidcClient, ProviderMetadata, TokenResult } from "./oidc";
import { authorizationUrl, exchangeCode, refresh, revoke } from "./oidc";
import type { SealKeys } from "./seal";
import { pkceChallenge, randomToken, safeEqual, seal, unseal } from "./seal";

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;

export interface SessionLimits {
  /** Log in again after this long, whatever happens. */
  readonly maxAgeMs: number;
  /** Log in again after this long without using the app. */
  readonly idleMs: number;
}

export const DEFAULT_LIMITS: SessionLimits = {
  idleMs: 7 * DAY,
  maxAgeMs: 30 * DAY,
};

/** Time to complete the login form at the provider. */
const LOGIN_TTL_MS = 10 * MINUTE;
/** Refresh the access token when less than this is left. */
const REFRESH_MARGIN_MS = 2 * MINUTE;
/**
 * Without a refresh, the cookie is only rewritten (to move `lastSeenAt`, for
 * the idle limit) when it is older than this: each rewrite carries the
 * refresh token read at the start of the request, and a slow response could
 * put back one a parallel request already replaced (sessionAccessToken).
 */
const LAST_SEEN_STEP_MS = 60 * MINUTE;

export interface AuthConfig {
  readonly appUrl: URL;
  readonly jmapSessionUrl: URL;
  readonly oidc: OidcClient;
  readonly metadata: () => Promise<ProviderMetadata>;
  readonly keys: SealKeys;
  readonly limits: SessionLimits;
  /**
   * VERIFIED_SENDER_BADGE forced by the admin (null: each user decides).
   * Sent with the token so the browser knows.
   */
  readonly verifiedSenderBadge: boolean | null;
  /** The admin's remote images settings (server/images/policy.ts). */
  readonly imagePolicy: AdminImagePolicy;
  /**
   * JMAP_PROXY=on: the browser never gets
   * an access token; JMAP goes through /jmap-proxy/ (server/jmap-proxy/).
   * Missing or false: off, the browser gets short tokens (/auth/token).
   */
  readonly jmapProxy?: boolean;
  /**
   * Where this server reaches the JMAP server (JMAP_INTERNAL_URL). Missing:
   * the issuer's own origin.
   */
  readonly upstream?: Upstream;
}

const LoginStateSchema = z.object({
  codeVerifier: z.string(),
  createdAt: z.number(),
  returnTo: z.string(),
  state: z.string(),
  v: z.literal(1),
});

const SessionSchema = z.object({
  accessToken: z.string(),
  accessTokenExpiresAt: z.number(),
  createdAt: z.number(),
  lastSeenAt: z.number(),
  refreshToken: z.string(),
  /**
   * Only with JMAP_PROXY=on: the value of the SameSite=Strict proxy cookie
   * (PROXY_COOKIE) that must come with every /jmap-proxy/ request.
   */
  proxyKey: z.string().optional(),
  v: z.literal(1),
});

type Session = z.infer<typeof SessionSchema>;

/** Login errors shown to the user; fixed codes, never text from the provider. */
export type LoginError = "denied" | "expired" | "invalid" | "unavailable";

const NO_STORE = { "cache-control": "no-store", pragma: "no-cache" } as const;

export const json = (
  body: unknown,
  status = 200,
  cookies: readonly string[] = []
): Response => {
  const headers = new Headers({
    ...NO_STORE,
    "content-type": "application/json",
  });
  for (const cookie of cookies) {
    headers.append("set-cookie", cookie);
  }
  return Response.json(body, { headers, status });
};

const redirect = (location: URL, cookies: readonly string[] = []): Response => {
  const headers = new Headers({ ...NO_STORE, location: location.href });
  for (const cookie of cookies) {
    headers.append("set-cookie", cookie);
  }
  // 303: the browser follows with a GET whatever the original method was.
  return new Response(null, { headers, status: 303 });
};

/**
 * Anti-CSRF for state-changing endpoints, on top of
 * SameSite=Lax. Modern browsers send `Sec-Fetch-Site`; older ones `Origin`.
 */
export const isSameOrigin = (request: Request, appUrl: URL): boolean => {
  const site = request.headers.get("sec-fetch-site");
  if (site !== null) {
    return site === "same-origin";
  }
  return request.headers.get("origin") === appUrl.origin;
};

const loginErrorUrl = (config: AuthConfig, error: LoginError): URL => {
  const url = new URL("/login", config.appUrl);
  url.searchParams.set("error", error);
  return url;
};

const readSession = async (
  request: Request,
  config: AuthConfig
): Promise<Session | undefined> => {
  const value = parseCookies(request.headers.get("cookie")).get(
    cookieName(SESSION_COOKIE, config.appUrl)
  );
  if (value === undefined) {
    return undefined;
  }
  const parsed = SessionSchema.safeParse(
    await unseal(config.keys, "session", value)
  );
  if (!parsed.success) {
    return undefined;
  }
  const now = config.oidc.now();
  const session = parsed.data;
  if (
    now - session.createdAt > config.limits.maxAgeMs ||
    now - session.lastSeenAt > config.limits.idleMs
  ) {
    return undefined;
  }
  return session;
};

const sessionCookie = async (
  config: AuthConfig,
  session: Session
): Promise<string> =>
  serializeCookie(
    cookieName(SESSION_COOKIE, config.appUrl),
    await seal(config.keys, "session", session),
    (session.createdAt + config.limits.maxAgeMs - config.oidc.now()) / 1000
  );

export const handleLogin = async (
  request: Request,
  config: AuthConfig
): Promise<Response> => {
  const metadata = await config.metadata();
  const codeVerifier = randomToken();
  const state = randomToken();
  const login = {
    codeVerifier,
    createdAt: config.oidc.now(),
    returnTo: safeReturnTo(new URL(request.url).searchParams.get("returnTo")),
    state,
    v: 1,
  } satisfies z.infer<typeof LoginStateSchema>;
  const cookie = serializeCookie(
    cookieName(LOGIN_COOKIE, config.appUrl),
    await seal(config.keys, "login", login),
    LOGIN_TTL_MS / 1000
  );
  return redirect(
    authorizationUrl(config.oidc, metadata, {
      codeChallenge: await pkceChallenge(codeVerifier),
      state,
    }),
    [cookie]
  );
};

const callbackError = (
  result: Extract<TokenResult, { ok: false }>
): LoginError => (result.kind === "unavailable" ? "unavailable" : "invalid");

export const handleCallback = async (
  request: Request,
  config: AuthConfig
): Promise<Response> => {
  const params = new URL(request.url).searchParams;
  const clearLogin = clearCookie(cookieName(LOGIN_COOKIE, config.appUrl));
  const fail = (error: LoginError): Response =>
    redirect(loginErrorUrl(config, error), [clearLogin]);

  const loginValue = parseCookies(request.headers.get("cookie")).get(
    cookieName(LOGIN_COOKIE, config.appUrl)
  );
  const login = LoginStateSchema.safeParse(
    loginValue === undefined
      ? undefined
      : await unseal(config.keys, "login", loginValue)
  );
  if (
    !login.success ||
    config.oidc.now() - login.data.createdAt > LOGIN_TTL_MS
  ) {
    return fail("expired");
  }
  if (!safeEqual(params.get("state") ?? "", login.data.state)) {
    return fail("invalid");
  }
  const metadata = await config.metadata();
  // RFC 9207: reject responses from another issuer (mix-up attacks).
  const iss = params.get("iss");
  if (
    (iss !== null ||
      metadata.authorization_response_iss_parameter_supported === true) &&
    iss !== metadata.issuer
  ) {
    return fail("invalid");
  }
  if (params.has("error")) {
    return fail(params.get("error") === "access_denied" ? "denied" : "invalid");
  }
  const code = params.get("code");
  if (code === null || code === "") {
    return fail("invalid");
  }

  const result = await exchangeCode(
    config.oidc,
    metadata,
    code,
    login.data.codeVerifier
  );
  if (!result.ok) {
    return fail(callbackError(result));
  }
  if (result.tokens.refreshToken === undefined) {
    // Without a refresh token the session would end with the access token.
    return fail("invalid");
  }
  const now = config.oidc.now();
  const proxyKey = config.jmapProxy === true ? randomToken() : undefined;
  const session: Session = {
    accessToken: result.tokens.accessToken,
    accessTokenExpiresAt: result.tokens.expiresAt,
    createdAt: now,
    lastSeenAt: now,
    refreshToken: result.tokens.refreshToken,
    v: 1,
    ...(proxyKey === undefined ? {} : { proxyKey }),
  };
  // Proxy mode: a second cookie, SameSite=Strict, that the proxy also
  // requires (server/jmap-proxy/proxy.ts). The session cookie stays Lax so
  // a link from another site still opens the app signed in; JMAP through
  // the proxy needs both. A top-level navigation may set a Strict cookie
  // even when the provider is on another site.
  const proxyCookies =
    proxyKey === undefined
      ? []
      : [
          serializeCookie(
            cookieName(PROXY_COOKIE, config.appUrl),
            proxyKey,
            config.limits.maxAgeMs / 1000,
            "Strict"
          ),
        ];
  // The cookie's returnTo is ours (sealed), but check it again anyway:
  // whatever it holds, the redirect stays on the app's origin.
  const target = new URL(safeReturnTo(login.data.returnTo), config.appUrl);
  return redirect(
    target.origin === config.appUrl.origin
      ? target
      : new URL("/", config.appUrl),
    [clearLogin, await sessionCookie(config, session), ...proxyCookies]
  );
};

/** What the browser gets: a short access token, kept in memory only. */
export interface TokenGrant {
  readonly accessToken: string;
  readonly expiresAt: number;
  readonly sessionUrl: string;
  readonly verifiedSenderBadge: boolean | null;
  readonly imagePolicy: AdminImagePolicy;
}

/** A valid access token for this request's session, or why there is none. */
export type SessionToken =
  | {
      readonly ok: true;
      readonly accessToken: string;
      readonly expiresAt: number;
      /**
       * The updated session cookie, when it changed (none or one): send it
       * with the response.
       */
      readonly cookies: readonly string[];
      /** A refresh just happened (the token is brand new). */
      readonly refreshed: boolean;
    }
  | {
      readonly ok: false;
      readonly response: Response;
    };

export interface SessionTokenOptions {
  /** Refresh even if the access token is still good. */
  readonly force?: boolean;
  /**
   * Counts as use of the app (moves `lastSeenAt`, for the idle limit).
   * `false` for the service worker's background requests (a push), so they
   * never keep an unused session alive.
   */
  readonly touch?: boolean;
}

/**
 * Refreshes of this process, by refresh token, for a minute. Parallel
 * requests of one session (the JMAP proxy sends many) share one refresh
 * instead of each presenting the same refresh token; and a request that
 * still carries a cookie from before a refresh never writes that cookie
 * back (it would put back the superseded refresh token, which Stalwart 1.0
 * takes as stolen and ends the session). Per OIDC client, so separate
 * configurations (tests) never share.
 */
interface SharedRefresh {
  readonly result: Promise<TokenResult>;
  readonly startedAt: number;
  /** Set once it gave new tokens: the refresh token is [SECRET] */
  rotated: boolean;
}

const REFRESH_SHARE_MS = 60_000;
const refreshesByClient = new WeakMap<OidcClient, Map<string, SharedRefresh>>();

const recentRefreshes = (
  client: OidcClient,
  now: number
): Map<string, SharedRefresh> => {
  let refreshes = refreshesByClient.get(client);
  if (refreshes === undefined) {
    refreshes = new Map();
    refreshesByClient.set(client, refreshes);
  }
  for (const [token, entry] of refreshes) {
    if (now - entry.startedAt > REFRESH_SHARE_MS) {
      refreshes.delete(token);
    }
  }
  return refreshes;
};

const sharedRefresh = async (
  config: AuthConfig,
  refreshToken: string,
  now: number,
  force: boolean
): Promise<TokenResult> => {
  const refreshes = recentRefreshes(config.oidc, now);
  const existing = refreshes.get(refreshToken);
  // A forced refresh (the JMAP server refused the token) only joins one
  // still running: a finished one may have given that very token.
  if (existing !== undefined && !(force && existing.rotated)) {
    return await existing.result;
  }
  const entry: SharedRefresh = {
    result: (async () =>
      await refresh(config.oidc, await config.metadata(), refreshToken))(),
    rotated: false,
    startedAt: now,
  };
  refreshes.set(refreshToken, entry);
  try {
    const result = await entry.result;
    if (result.ok) {
      entry.rotated = true;
    } else {
      // A failure isn't shared: the next request may try again.
      refreshes.delete(refreshToken);
    }
    return result;
  } catch (error) {
    refreshes.delete(refreshToken);
    throw error;
  }
};

/** Did this process just refresh with this refresh token? */
const wasRotated = (
  config: AuthConfig,
  refreshToken: string,
  now: number
): boolean =>
  recentRefreshes(config.oidc, now).get(refreshToken)?.rotated === true;

/**
 * Reads the session cookie and returns a usable access token, refreshing it
 * when it is about to expire (or always, with `force`). Used by /auth/token
 * and by server endpoints that call JMAP themselves (image tickets, §7.2).
 */
export const sessionAccessToken = async (
  request: Request,
  config: AuthConfig,
  { force = false, touch = true }: SessionTokenOptions = {}
): Promise<SessionToken> => {
  const session = await readSession(request, config);
  if (session === undefined) {
    return {
      ok: false,
      response: json({ error: "unauthenticated" }, 401, [
        clearCookie(cookieName(SESSION_COOKIE, config.appUrl)),
      ]),
    };
  }
  const now = config.oidc.now();
  if (!force && session.accessTokenExpiresAt - now >= REFRESH_MARGIN_MS) {
    // Rewriting the cookie on every call would race with a parallel
    // refresh: this response could arrive last and put back the refresh
    // token the other one replaced, which Stalwart 1.0 takes as a stolen
    // token and ends the session. Only `lastSeenAt` moves, rarely.
    return {
      accessToken: session.accessToken,
      cookies:
        touch &&
        now - session.lastSeenAt > LAST_SEEN_STEP_MS &&
        !wasRotated(config, session.refreshToken, now)
          ? [await sessionCookie(config, { ...session, lastSeenAt: now })]
          : [],
      expiresAt: session.accessTokenExpiresAt,
      ok: true,
      refreshed: false,
    };
  }
  const result = await sharedRefresh(config, session.refreshToken, now, force);
  if (!result.ok) {
    // Only a refused refresh token (or client) ends the session; a 429, an
    // `invalid_request` or a server error may pass, so the cookie stays.
    return {
      ok: false,
      response:
        result.kind === "rejected" && endsSession(result.error)
          ? json({ error: "unauthenticated" }, 401, [
              clearCookie(cookieName(SESSION_COOKIE, config.appUrl)),
            ])
          : json({ error: "unavailable" }, 503),
    };
  }
  const next: Session = {
    ...session,
    accessToken: result.tokens.accessToken,
    accessTokenExpiresAt: result.tokens.expiresAt,
    lastSeenAt: touch ? now : session.lastSeenAt,
    // Stalwart 1.0 sends a new one on every refresh: presenting a superseded
    // one later ends the grant, while retrying the same one (two tabs at
    // once, a lost answer) gets its successor again. Others may send none.
    refreshToken: result.tokens.refreshToken ?? session.refreshToken,
  };
  return {
    accessToken: next.accessToken,
    cookies: [await sessionCookie(config, next)],
    expiresAt: next.accessTokenExpiresAt,
    ok: true,
    refreshed: true,
  };
};

/**
 * Runs `run` with this request's access token and always sends the updated
 * session cookie with the answer, also when `run` fails or throws: a refresh
 * rotated the refresh token, and losing the new one would make the browser
 * present the superseded one later, which ends the grant (Stalwart 1.0
 * reuse detection). An exception becomes 502 `unavailable`.
 */
export const withSessionToken = async (
  request: Request,
  config: AuthConfig,
  run: (token: Extract<SessionToken, { ok: true }>) => Promise<Response>,
  options: SessionTokenOptions = {}
): Promise<Response> => {
  const token = await sessionAccessToken(request, config, options);
  if (!token.ok) {
    return token.response;
  }
  let response: Response;
  try {
    response = await run(token);
  } catch {
    response = json({ error: "unavailable" }, 502);
  }
  if (token.cookies.length === 0) {
    return response;
  }
  // A new Headers: a Response's own may not take `set-cookie`.
  const headers = new Headers(response.headers);
  for (const cookie of token.cookies) {
    headers.append("set-cookie", cookie);
  }
  return new Response(response.body, {
    headers,
    status: response.status,
    statusText: response.statusText,
  });
};

export const handleToken = async (
  request: Request,
  config: AuthConfig
): Promise<Response> => {
  if (config.jmapProxy === true) {
    // Full BFF mode: no token ever leaves the server. The browser reads
    // this answer to switch to the proxy (session/bff.ts).
    return json({ error: "jmap-proxy" }, 404);
  }
  if (request.method !== "POST" || !isSameOrigin(request, config.appUrl)) {
    return json({ error: "forbidden" }, 403);
  }
  const params = new URL(request.url).searchParams;
  // `?refresh=1`: JMAP rejected a token that hadn't expired yet.
  const force = params.get("refresh") === "1";
  // `?background=1`: the service worker (a push), not the user. It only
  // keeps the idle limit from moving, so nobody gains by forging it.
  const touch = params.get("background") !== "1";
  const token = await sessionAccessToken(request, config, { force, touch });
  if (!token.ok) {
    return token.response;
  }
  const grant: TokenGrant = {
    accessToken: token.accessToken,
    expiresAt: token.expiresAt,
    sessionUrl: config.jmapSessionUrl.href,
    verifiedSenderBadge: config.verifiedSenderBadge,
    imagePolicy: config.imagePolicy,
  };
  return json(grant, 200, token.cookies);
};

/** The Set-Cookie values that end the session in this browser. */
export const sessionClearingCookies = (config: AuthConfig): string[] => [
  clearCookie(cookieName(SESSION_COOKIE, config.appUrl)),
  ...(config.jmapProxy === true
    ? [clearCookie(cookieName(PROXY_COOKIE, config.appUrl), "Strict")]
    : []),
];

/**
 * Clears the session cookie and, when the provider has a revocation endpoint
 * (RFC 7009; the bridge and Stalwart 1.0 have one), revokes the refresh
 * token so the server ends the session too (Stalwart deletes the whole grant,
 * access tokens included). Without it the access token stays valid until it
 * expires (15 min) and the refresh token is lost with the cookie.
 */
export const handleLogout = async (
  request: Request,
  config: AuthConfig
): Promise<Response> => {
  if (request.method !== "POST" || !isSameOrigin(request, config.appUrl)) {
    return json({ error: "forbidden" }, 403);
  }
  const session = await readSession(request, config);
  if (session !== undefined) {
    try {
      await revoke(config.oidc, await config.metadata(), session.refreshToken);
    } catch {
      // Discovery failed: logout still clears the cookie.
    }
  }
  return json({ ok: true }, 200, sessionClearingCookies(config));
};

/**
 * Proxy mode: does the request carry the SameSite=Strict proxy cookie of
 * its session? Without the session, or with a session from before the
 * proxy was turned on (no `proxyKey`), never.
 */
export const hasProxyBinding = async (
  request: Request,
  config: AuthConfig
): Promise<boolean> => {
  const session = await readSession(request, config);
  const presented = parseCookies(request.headers.get("cookie")).get(
    cookieName(PROXY_COOKIE, config.appUrl)
  );
  return (
    session?.proxyKey !== undefined &&
    presented !== undefined &&
    safeEqual(presented, session.proxyKey)
  );
};

/**
 * For SSR: is there a valid session? Doesn't refresh or touch the cookie.
 * In proxy mode a session from before the proxy was turned on (no
 * `proxyKey`) can't use the proxy: it counts as none, so /login shows the
 * form instead of sending the user back to an app that can't load (a
 * redirect loop). The Strict cookie itself isn't asked for here: a link
 * from another site doesn't send it, but the app's own fetches do.
 */
export const hasSession = async (
  request: Request,
  config: AuthConfig
): Promise<boolean> => {
  const session = await readSession(request, config);
  return (
    session !== undefined &&
    (config.jmapProxy !== true || session.proxyKey !== undefined)
  );
};
