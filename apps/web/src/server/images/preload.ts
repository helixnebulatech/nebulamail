// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// POST /api/images/preload { optOut?: boolean }
//
// Lets a user turn off `preload` (images fetched when the email arrives)
// for their own addresses. The image proxy can't read the user's
// preferences, so the BFF tells it, in a request signed with the ticket
// keys (`signPreferenceRequest`). The addresses come from JMAP with the
// user's own token (login name and mail identities), never from the
// browser, so nobody can opt out (or in) an address that isn't theirs.
//
// Without `optOut` it only reads the state. Answer:
// `{ available: false }` when the proxy has no preload (or none is
// configured), else `{ available: true, optOut, full }`. `full` is null,
// or `{ until }` when an address of the user that still preloads is over
// its quota (accepted risk H15): until then their new mail
// works like `cache`, and the app says so.

import { MAX_PREFERENCE_ADDRESSES } from "@nebulamail/image-proxy/tickets";
import { CAPABILITIES, hasCapability } from "@nebulamail/jmap-client";
import type { JmapClient } from "@nebulamail/jmap-client";
import { z } from "zod";

import type { AuthConfig } from "../auth/handlers";
import { isSameOrigin, json, withSessionToken } from "../auth/handlers";
import { connectAs } from "../jmap-connect";
import { bodyError, readJsonBody } from "../request-body";

export interface PreloadPrefsConfig {
  /** Origin of the image proxy (the same as for tickets). */
  readonly proxyOrigin: string;
  readonly sign: (input: {
    readonly addresses: readonly string[];
    readonly optOut?: boolean;
  }) => Promise<{ readonly body: string; readonly authorization: string }>;
}

export interface PreloadPrefsDeps {
  readonly fetch: typeof fetch;
}

const PREFERENCES_PATH = "/preload/preferences";
const MAX_REQUEST_BYTES = 256;
const PROXY_TIMEOUT_MS = 10_000;

const Body = z.strictObject({ optOut: z.boolean().optional() });
const ProxyAnswer = z.object({
  /** Missing in proxies older than the warning: read as "not full". */
  full: z
    .object({ until: z.number().int().nonnegative().nullable() })
    .nullable()
    .optional(),
  optOut: z.boolean(),
});

/** `Name <a@b>` or `a@b` → `a@b` in lower case; null if not an address. */
const addressOf = (raw: string): string | null => {
  const inner = /<(?<address>[^<>]+)>\s*$/u.exec(raw)?.groups?.address ?? raw;
  const address = inner.trim().toLowerCase();
  return /^[^\s@]+@[^\s@]+$/u.test(address) && address.length <= 320
    ? address
    : null;
};

type OwnAddresses =
  | { readonly ok: true; readonly addresses: readonly string[] }
  | { readonly ok: false };

/**
 * The user's own addresses, from JMAP with their token. Fails (instead of
 * going on with the login name only) when Identity/get fails: an opt-out
 * that silently misses an alias would keep preloading its mail.
 */
const ownAddresses = async (client: JmapClient): Promise<OwnAddresses> => {
  const own = new Set<string>();
  const login = addressOf(client.session.username);
  if (login !== null) {
    own.add(login);
  }
  const accountId = client.primaryAccountId();
  if (hasCapability(client.session, CAPABILITIES.submission, accountId)) {
    const [identities] = await client.requestSettled((b) => [
      b.call("Identity/get", { accountId, properties: ["email"] }),
    ]);
    if (!identities.ok) {
      return { ok: false };
    }
    for (const identity of identities.value.list) {
      const address = addressOf(identity.email);
      if (address !== null) {
        own.add(address);
      }
    }
  }
  return { addresses: [...own], ok: true };
};

export const handlePreloadPreference = async (
  request: Request,
  auth: AuthConfig,
  config: PreloadPrefsConfig | undefined,
  deps: PreloadPrefsDeps
): Promise<Response> => {
  if (request.method !== "POST" || !isSameOrigin(request, auth.appUrl)) {
    return json({ error: "forbidden" }, 403);
  }
  if (config === undefined) {
    // No proxy: nothing to read or sign.
    return json({ available: false });
  }
  const read = await readJsonBody(request, MAX_REQUEST_BYTES, { emptyAs: {} });
  if (!read.ok) {
    return json(bodyError(read.status), read.status);
  }
  const body = Body.safeParse(read.value);
  if (!body.success) {
    return json({ error: "invalid" }, 400);
  }
  return await withSessionToken(request, auth, async (token) => {
    const client = await connectAs(auth, deps.fetch, token.accessToken);
    const own = await ownAddresses(client);
    if (!own.ok) {
      return json({ error: "unavailable" }, 502);
    }
    const { addresses } = own;
    if (addresses.length > MAX_PREFERENCE_ADDRESSES) {
      // Never a partial opt-out: the addresses left out would still be
      // preloaded while the switch says "off". Nobody has that many.
      return json({ error: "too-many-addresses" }, 422);
    }
    if (addresses.length === 0) {
      // Nothing the hook could match (e.g. a login name without a domain).
      return json({ available: false }, 200);
    }
    const signed = await config.sign({
      addresses,
      ...(body.data.optOut === undefined ? {} : { optOut: body.data.optOut }),
    });
    let answer: Response;
    try {
      answer = await deps.fetch(`${config.proxyOrigin}${PREFERENCES_PATH}`, {
        body: signed.body,
        headers: {
          authorization: signed.authorization,
          "content-type": "application/json",
        },
        method: "POST",
        redirect: "manual",
        signal: AbortSignal.timeout(PROXY_TIMEOUT_MS),
      });
    } catch {
      return json({ error: "unavailable" }, 502);
    }
    if (answer.status === 404) {
      await answer.body?.cancel();
      return json({ available: false }, 200);
    }
    let parsed: ReturnType<typeof ProxyAnswer.safeParse> | null = null;
    try {
      parsed =
        answer.status === 200
          ? ProxyAnswer.safeParse(await answer.json())
          : null;
    } catch {
      parsed = null;
    }
    if (parsed?.success !== true) {
      if (!answer.bodyUsed) {
        await answer.body?.cancel();
      }
      return json({ error: "unavailable" }, 502);
    }
    return json(
      {
        available: true,
        full: parsed.data.optOut ? null : (parsed.data.full ?? null),
        optOut: parsed.data.optOut,
      },
      200
    );
  });
};
