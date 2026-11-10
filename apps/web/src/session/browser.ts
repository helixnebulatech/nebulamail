// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Browser side of the login in the static build: OAuth as a
// public client with PKCE against the server that serves the app
// (Stalwart). The refresh token lives only in the session worker
// (worker.ts); this page gets short access tokens and keeps them in memory.

import { leaveForExpiredLogin } from "../lib/session-expired";
import "../lib/zod-config";
import {
  authorizationUrl,
  fetchMetadata,
  parseCallback,
  pkceChallenge,
  randomToken,
  safeReturnTo,
} from "./oauth";
import type { ProviderMetadata } from "./oauth";
import { EventSchema, ReplySchema, TokenSchema } from "./protocol";
import type { RequestBody, WorkerError, WorkerReply } from "./protocol";
import { GRANT_MARGIN_MS, SessionExpiredError } from "./types";
import type { Grant, LoginCompletion, SessionApi } from "./types";

export { SessionExpiredError } from "./types";
export type { Grant } from "./types";

/** Registered in Stalwart as a public client (docs/static.md). */
const CLIENT_ID = import.meta.env.NEBULA_OAUTH_CLIENT_ID;
/** The login in progress, while the page is away at the login form. */
const LOGIN_KEY = "nebula-login";
/** A login older than this is refused (like the BFF's login cookie). */
const LOGIN_TTL_MS = 10 * 60 * 1000;

const base = (): string => import.meta.env.BASE_URL;
const redirectUri = (): string =>
  new URL(`${base()}login`, globalThis.location.origin).href;

interface Channel {
  readonly post: (message: unknown) => void;
  readonly listen: (
    onMessage: (data: unknown) => void,
    onError: () => void
  ) => void;
}

const openChannel = (): Channel => {
  const name = "nebula-session";
  // Vite bundles the worker from these `new URL(…, import.meta.url)`.
  if (typeof SharedWorker === "function") {
    const shared = new SharedWorker(new URL("worker.ts", import.meta.url), {
      // Chrome: the worker outlives the last tab for a while, so a reload
      // (the only tab leaves, then comes back) keeps the session.
      extendedLifetime: true,
      name,
      type: "module",
    });
    shared.port.start();
    return {
      listen: (onMessage, onError) => {
        shared.port.addEventListener("message", (event) => {
          onMessage(event.data);
        });
        shared.addEventListener("error", onError);
      },
      post: (message) => {
        // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a MessagePort, not a window: there is no target origin.
        shared.port.postMessage(message);
      },
    };
  }
  const dedicated = new Worker(new URL("worker.ts", import.meta.url), {
    name,
    type: "module",
  });
  return {
    listen: (onMessage, onError) => {
      dedicated.addEventListener("message", (event) => {
        onMessage(event.data);
      });
      dedicated.addEventListener("error", onError);
    },
    post: (message) => {
      // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a Worker, not a window: there is no target origin.
      dedicated.postMessage(message);
    },
  };
};

class WorkerFailureError extends Error {
  override readonly name = "WorkerFailureError";
  readonly kind: WorkerError;
  constructor(kind: WorkerError) {
    super(`Session worker: ${kind}`);
    this.kind = kind;
  }
}

/** A reply slower than this means the worker is stuck. */
const CALL_TIMEOUT_MS = 30_000;

let current: Grant | undefined;
/** This page has shown someone's mail (it got a grant). */
let signedIn = false;
/** This page asked for the logout (its own loggedOut is expected). */
let loggingOut = false;
let completing: Promise<LoginCompletion> | undefined;
let metadataRequest: Promise<ProviderMetadata> | undefined;
let channel: Channel | undefined;
let nextId = 1;
const waiting = new Map<number, PromiseWithResolvers<WorkerReply>>();

const worker = (): Channel => {
  if (channel !== undefined) {
    return channel;
  }
  const opened = openChannel();
  opened.listen(
    (data) => {
      const reply = ReplySchema.safeParse(data);
      if (reply.success) {
        waiting.get(reply.data.id)?.resolve(reply.data);
        waiting.delete(reply.data.id);
        return;
      }
      const event = EventSchema.safeParse(data).data?.event;
      // Logged out in another tab (or the refresh token was rejected).
      // A page that showed mail doesn't keep it on screen: it leaves for the
      // login (a full load, so nothing stays in memory), unless this page is
      // the one logging out (it goes where its logout takes it).
      if (event === "loggedOut") {
        current = undefined;
        if (signedIn && !loggingOut) {
          leaveForExpiredLogin();
        }
      }
      // Logged in from another tab, maybe as someone else: this page may
      // still hold the previous account's mail, so it starts over.
      if (event === "loggedIn" && signedIn && completing === undefined) {
        globalThis.location.reload();
      }
    },
    () => {
      // The worker didn't start (or crashed): nothing will answer these.
      channel = undefined;
      for (const pending of waiting.values()) {
        pending.reject(new WorkerFailureError("unavailable"));
      }
      waiting.clear();
    }
  );
  channel = opened;
  return opened;
};

const call = async (body: RequestBody): Promise<unknown> => {
  const id = nextId;
  nextId += 1;
  const pending = Promise.withResolvers<WorkerReply>();
  waiting.set(id, pending);
  const timer = setTimeout(() => {
    waiting.delete(id);
    pending.reject(new WorkerFailureError("unavailable"));
  }, CALL_TIMEOUT_MS);
  worker().post({ ...body, id });
  const reply = await pending.promise.finally(() => {
    clearTimeout(timer);
  });
  if (!reply.ok) {
    throw new WorkerFailureError(reply.error);
  }
  return reply.value;
};

/** Fetched once per page; a failure is forgotten, so the next call retries. */
const loadMetadata = async (): Promise<ProviderMetadata> => {
  try {
    return await fetchMetadata(globalThis.location.origin);
  } catch (error) {
    metadataRequest = undefined;
    throw error;
  }
};

/** The server's metadata, once per page (it doesn't change). */
const providerMetadata = async (): Promise<ProviderMetadata> => {
  metadataRequest ??= loadMetadata();
  return await metadataRequest;
};

export const getAuthState = async (): Promise<{
  readonly authenticated: boolean;
}> => {
  // Prerendering the shell at build time: nobody is logged in there.
  if (typeof document === "undefined") {
    return { authenticated: false };
  }
  const value = await call({ op: "status" });
  return {
    authenticated:
      typeof value === "object" &&
      value !== null &&
      "authenticated" in value &&
      value.authenticated === true,
  };
};

let pending: Promise<Grant> | undefined;

const requestGrant = async (forceRefresh: boolean): Promise<Grant> => {
  let value: unknown;
  try {
    value = await call({ forceRefresh, op: "grant" });
  } catch (error) {
    if (error instanceof WorkerFailureError && error.kind === "expired") {
      current = undefined;
      throw new SessionExpiredError("Session expired");
    }
    throw error;
  }
  const token = TokenSchema.parse(value);
  const { issuer } = await providerMetadata();
  signedIn = true;
  current = {
    ...token,
    // Stalwart writes its public name in the JMAP session.
    serverAliases: [new URL(issuer).origin],
    // The JMAP session of this same server, on this origin.
    sessionUrl: new URL("/.well-known/jmap", globalThis.location.origin).href,
    // No admin setting in the static build: each user decides.
    verifiedSenderBadge: null,
  };
  return current;
};

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

/**
 * Ends the session: the worker forgets the tokens at once, then revokes the
 * refresh token on the server (Stalwart 1.0 ends the whole grant) when it
 * can, without failing the logout.
 */
export const logout = async (): Promise<void> => {
  current = undefined;
  loggingOut = true;
  try {
    await call({ op: "logout" });
  } finally {
    loggingOut = false;
  }
};

/** No JMAP proxy without our server (bff.ts): nothing to remember. */
export const setJmapProxyMode = (_on: boolean): void => {
  // The static build always talks JMAP with a token.
};

/** No JMAP proxy (bff.ts): the session URL comes with the token. */
export const proxySessionUrl = (): string | null => null;

interface PendingLogin {
  readonly state: string;
  readonly verifier: string;
  readonly returnTo: string;
  readonly createdAt: number;
}

const readPendingLogin = (): PendingLogin | undefined => {
  const raw = sessionStorage.getItem(LOGIN_KEY);
  sessionStorage.removeItem(LOGIN_KEY);
  if (raw === null) {
    return undefined;
  }
  try {
    const value: unknown = JSON.parse(raw);
    if (
      typeof value === "object" &&
      value !== null &&
      "state" in value &&
      typeof value.state === "string" &&
      "verifier" in value &&
      typeof value.verifier === "string" &&
      "returnTo" in value &&
      typeof value.returnTo === "string" &&
      "createdAt" in value &&
      typeof value.createdAt === "number"
    ) {
      return {
        createdAt: value.createdAt,
        returnTo: value.returnTo,
        state: value.state,
        verifier: value.verifier,
      };
    }
  } catch {
    // Not ours: ignore it.
  }
  return undefined;
};

/**
 * Leaves for the server's login form. The PKCE verifier and `state` wait in
 * sessionStorage (this tab only) until the callback: the worker may end
 * while the page is away (it was the last tab).
 */
export const beginLogin = async (
  returnTo: string | undefined
): Promise<void> => {
  const { origin } = globalThis.location;
  const metadata = await providerMetadata();
  const verifier = randomToken(32);
  const login: PendingLogin = {
    createdAt: Date.now(),
    returnTo: safeReturnTo(returnTo),
    state: randomToken(16),
    verifier,
  };
  sessionStorage.setItem(LOGIN_KEY, JSON.stringify(login));
  globalThis.location.assign(
    authorizationUrl(metadata, origin, {
      clientId: CLIENT_ID,
      codeChallenge: await pkceChallenge(verifier),
      redirectUri: redirectUri(),
      state: login.state,
    })
  );
};

const isCallback = (params: URLSearchParams): boolean =>
  params.has("state") && (params.has("code") || params.has("error"));

const finishLogin = async (
  params: URLSearchParams
): Promise<LoginCompletion> => {
  const login = readPendingLogin();
  if (login === undefined || Date.now() - login.createdAt > LOGIN_TTL_MS) {
    return { error: "expired", kind: "failed" };
  }
  let metadata;
  try {
    metadata = await providerMetadata();
  } catch {
    return { error: "unavailable", kind: "failed" };
  }
  const result = parseCallback(params, login.state, metadata);
  if (!result.ok) {
    return { error: result.error, kind: "failed" };
  }
  try {
    await call({
      clientId: CLIENT_ID,
      code: result.code,
      op: "complete",
      redirectUri: redirectUri(),
      verifier: login.verifier,
    });
  } catch (error) {
    return {
      error:
        error instanceof WorkerFailureError && error.kind === "unavailable"
          ? "unavailable"
          : "invalid",
      kind: "failed",
    };
  }
  return { kind: "done", returnTo: login.returnTo };
};

/**
 * On /login: finishes the login when the server sent the user back here.
 * The router may run /login's `beforeLoad` again while this is going (the
 * URL is clean by then): that call waits for the same result.
 */
export const completeLogin = async (
  search: string
): Promise<LoginCompletion> => {
  const params = new URLSearchParams(search);
  if (!isCallback(params)) {
    return (await completing) ?? { kind: "none" };
  }
  // The code must not stay in the address bar or the history.
  globalThis.history.replaceState(null, "", `${base()}login`);
  completing = finishLogin(params).finally(() => {
    completing = undefined;
  });
  return await completing;
};

export const session = {
  beginLogin,
  completeLogin,
  getAuthState,
  getGrant,
  logout,
} satisfies SessionApi;
