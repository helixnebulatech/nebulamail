// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The session worker of the static build. It is the only
// place that ever holds the refresh token, in memory: pages can't read a
// worker's variables, so a script injected in a page can use the session
// while the page is open, but can't take the long-lived token away.
//
// Runs as a SharedWorker (one session for every tab of the app; it ends
// when the last tab closes) or, where SharedWorker doesn't exist, as a
// dedicated Worker (one session per tab). Nothing is ever written to
// storage.

import { endsSession } from "../lib/oauth-errors";
import "../lib/zod-config";
import { fetchMetadata, onOrigin, revokeRequest, tokenRequest } from "./oauth";
import type { ProviderMetadata, TokenResult } from "./oauth";
import { RequestSchema } from "./protocol";
import type {
  WorkerError,
  WorkerEvent,
  WorkerReply,
  WorkerRequest,
} from "./protocol";
import { GRANT_MARGIN_MS } from "./types";

interface Port {
  postMessage: (message: WorkerReply | WorkerEvent) => void;
}

interface Session {
  readonly clientId: string;
  readonly refreshToken: string;
  readonly accessToken: string;
  readonly expiresAt: number;
}

const { origin } = globalThis.location;
const ports = new Set<Port>();
let session: Session | undefined;
let refreshing: Promise<Session | WorkerError> | undefined;
let metadata: Promise<ProviderMetadata> | undefined;

/** Fetched once; a failure is forgotten, so the next call tries again. */
const loadMetadata = async (): Promise<ProviderMetadata> => {
  try {
    return await fetchMetadata(origin);
  } catch (error) {
    metadata = undefined;
    throw error;
  }
};

const providerMetadata = async (): Promise<ProviderMetadata> => {
  metadata ??= loadMetadata();
  return await metadata;
};

/** The token endpoint, on this origin (oauth.ts). */
const tokenEndpoint = async (): Promise<URL> => {
  const { token_endpoint: endpoint } = await providerMetadata();
  return onOrigin(endpoint, origin);
};

const broadcast = (event: WorkerEvent): void => {
  for (const port of ports) {
    port.postMessage(event);
  }
};

const end = (): void => {
  const had = session !== undefined;
  session = undefined;
  if (had) {
    broadcast({ event: "loggedOut" });
  }
};

const failure = (result: Extract<TokenResult, { ok: false }>): WorkerError =>
  result.kind === "unavailable" ? "unavailable" : "expired";

const endsSessionResult = (
  result: Extract<TokenResult, { ok: false }>
): boolean => result.kind === "rejected" && endsSession(result.error);

/**
 * Ends the grant on the server too, when it has a revocation endpoint (on
 * this origin, like the token endpoint). Never fails.
 */
const revoke = async (ended: Session): Promise<void> => {
  let endpoint: URL;
  try {
    const { revocation_endpoint: url } = await providerMetadata();
    if (url === undefined) {
      return;
    }
    endpoint = onOrigin(url, origin);
  } catch {
    return;
  }
  await revokeRequest(endpoint, {
    clientId: ended.clientId,
    refreshToken: ended.refreshToken,
  });
};

const refresh = async (current: Session): Promise<Session | WorkerError> => {
  let endpoint: URL;
  try {
    endpoint = await tokenEndpoint();
  } catch {
    return "unavailable";
  }
  const result = await tokenRequest(endpoint, {
    client_id: current.clientId,
    grant_type: "refresh_token",
    refresh_token: current.refreshToken,
  });
  if (!result.ok) {
    // Only a refused refresh token ends the session; anything else
    // (a 429, a malformed answer…) may pass.
    if (!endsSessionResult(result)) {
      return "unavailable";
    }
    if (session === current) {
      end();
    }
    return failure(result);
  }
  // Logged out (or in again) while this was running: keep that. The
  // rotated refresh token is nobody's now; it is revoked so it can't
  // outlive the logout (without blocking the answer; `revoke` never fails).
  if (session !== current) {
    const rotated = result.tokens.refreshToken;
    if (rotated !== undefined) {
      void revoke({ ...current, refreshToken: rotated });
    }
    return session ?? "expired";
  }
  session = {
    accessToken: result.tokens.accessToken,
    clientId: current.clientId,
    expiresAt: result.tokens.expiresAt,
    // Stalwart 1.0 sends a new one on every refresh, and presenting the
    // old one again later ends the whole session: always keep the latest
    // (refreshes are serialised by `refreshing`). Others may send none.
    refreshToken: result.tokens.refreshToken ?? current.refreshToken,
  };
  return session;
};

const grant = async (forceRefresh: boolean): Promise<Session | WorkerError> => {
  const current = session;
  if (current === undefined) {
    return "expired";
  }
  if (!forceRefresh && current.expiresAt - Date.now() > GRANT_MARGIN_MS) {
    return current;
  }
  refreshing ??= refresh(current).finally(() => {
    refreshing = undefined;
  });
  return await refreshing;
};

/**
 * A login that didn't go through: the session it would have replaced ends
 * too (the user was leaving it), on the server as well.
 */
const leave = async (): Promise<void> => {
  const ended = session;
  if (ended !== undefined) {
    end();
    await revoke(ended);
  }
};

const complete = async (
  request: Extract<WorkerRequest, { op: "complete" }>
): Promise<WorkerError | undefined> => {
  if (new URL(request.redirectUri).origin !== origin) {
    return "invalid";
  }
  // A login over a session: the previous grant is revoked once the new one
  // exists, so a failed exchange doesn't leave the user without both.
  // Stalwart 1.0 revokes only the grant of the token it is given, not every
  // grant of the user and client (checked live, e2e-static/static.e2e.ts).
  let endpoint: URL;
  try {
    endpoint = await tokenEndpoint();
  } catch {
    await leave();
    return "unavailable";
  }
  const result = await tokenRequest(endpoint, {
    client_id: request.clientId,
    code: request.code,
    code_verifier: request.verifier,
    grant_type: "authorization_code",
    redirect_uri: request.redirectUri,
  });
  if (!result.ok) {
    await leave();
    return result.kind === "unavailable" ? "unavailable" : "invalid";
  }
  // Without a refresh token the session would end with the access token.
  if (result.tokens.refreshToken === undefined) {
    await leave();
    return "invalid";
  }
  // The session there is now, not the one when this started: a refresh
  // meanwhile may have rotated its refresh token. A refresh still running
  // finds `session` changed and revokes what it gets (`refresh`).
  const replaced = session;
  session = {
    accessToken: result.tokens.accessToken,
    clientId: request.clientId,
    expiresAt: result.tokens.expiresAt,
    refreshToken: result.tokens.refreshToken,
  };
  if (replaced !== undefined) {
    // Maybe someone else now: the tabs drop the previous account first.
    broadcast({ event: "loggedOut" });
  }
  broadcast({ event: "loggedIn" });
  if (replaced !== undefined) {
    // Awaited, as in the logout: a dedicated worker dies with its page.
    await revoke(replaced);
  }
  return undefined;
};

const answer = async (request: WorkerRequest): Promise<WorkerReply> => {
  const { id } = request;
  switch (request.op) {
    case "status": {
      return { id, ok: true, value: { authenticated: session !== undefined } };
    }
    case "grant": {
      const result = await grant(request.forceRefresh);
      return typeof result === "string"
        ? { error: result, id, ok: false }
        : {
            id,
            ok: true,
            // Never the refresh token: only what the page needs.
            value: {
              accessToken: result.accessToken,
              expiresAt: result.expiresAt,
            },
          };
    }
    case "complete": {
      const error = await complete(request);
      return error === undefined
        ? { id, ok: true, value: null }
        : { error, id, ok: false };
    }
    case "logout": {
      const ended = session;
      // Forgotten first: every tab is logged out whatever the server does.
      end();
      if (ended !== undefined) {
        // Awaited (briefly, oauth.ts) because a dedicated worker dies with
        // the page that may leave right after the answer.
        await revoke(ended);
      }
      return { id, ok: true, value: null };
    }
    default: {
      return { error: "invalid", id, ok: false };
    }
  }
};

const listen = (port: Port & EventTarget): void => {
  ports.add(port);
  port.addEventListener("message", (event) => {
    const data: unknown = event instanceof MessageEvent ? event.data : null;
    const request = RequestSchema.safeParse(data);
    if (!request.success) {
      return;
    }
    void (async () => {
      let reply: WorkerReply;
      try {
        reply = await answer(request.data);
      } catch {
        reply = { error: "unavailable", id: request.data.id, ok: false };
      }
      port.postMessage(reply);
    })();
  });
};

// A SharedWorker gets a port per tab; a dedicated worker talks through its
// own global scope.
if ("onconnect" in globalThis) {
  globalThis.addEventListener("connect", (event) => {
    const [port] = event instanceof MessageEvent ? event.ports : [];
    if (port !== undefined) {
      listen(port);
      port.start();
    }
  });
} else {
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- a dedicated worker's global has postMessage; the DOM lib types it as Window.
  listen(globalThis);
}
