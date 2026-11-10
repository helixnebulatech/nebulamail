// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { JmapError } from "@nebulamail/jmap-client";
import { afterEach, describe, expect, it } from "vitest";

import { connectAs, forgetSessionUrl } from "./jmap-connect";

const JMAP = "https://jmap.example.com";
const WELL_KNOWN = new URL("/.well-known/jmap", JMAP);

const session = (origin: string) => ({
  accounts: {
    acc1: {
      accountCapabilities: { "urn:ietf:params:jmap:mail": {} },
      isPersonal: true,
      isReadOnly: false,
      name: "[EMAIL]",
    },
  },
  apiUrl: `${origin}/jmap/`,
  capabilities: {
    "urn:ietf:params:jmap:core": {
      collationAlgorithms: [],
      maxCallsInRequest: 16,
      maxConcurrentRequests: 4,
      maxConcurrentUpload: 4,
      maxObjectsInGet: 500,
      maxObjectsInSet: 500,
      maxSizeRequest: 10_000_000,
      maxSizeUpload: 50_000_000,
    },
    "urn:ietf:params:jmap:mail": {},
  },
  downloadUrl: `${origin}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
  eventSourceUrl: `${origin}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
  primaryAccounts: { "urn:ietf:params:jmap:mail": "acc1" },
  state: "s1",
  uploadUrl: `${origin}/jmap/upload/{accountId}/`,
  username: "[EMAIL]",
});

/** /.well-known/jmap redirects to `target`, which answers the session. */
const server = (target: string) => {
  const calls: { url: string; auth: string | null }[] = [];
  let moved = false;
  const fake: typeof fetch = async (input, init) => {
    await Promise.resolve();
    const url = input instanceof Request ? input.url : String(input);
    calls.push({ auth: new Headers(init?.headers).get("authorization"), url });
    if (url === WELL_KNOWN.href) {
      return new Response(null, { headers: { location: target }, status: 307 });
    }
    if (url === target && !moved) {
      return Response.json(session(JMAP));
    }
    return new Response("not found", { status: 404 });
  };
  return {
    calls,
    fake,
    move: () => {
      moved = true;
    },
  };
};

const auth = { jmapSessionUrl: WELL_KNOWN };

describe(connectAs, () => {
  afterEach(() => {
    forgetSessionUrl();
  });

  it("remembers the redirect target, not the token", async () => {
    const { calls, fake } = server(`${JMAP}/jmap/session`);
    await connectAs(auth, fake, "token-a", () => 0);
    expect(calls.map(({ url }) => url)).toStrictEqual([
      WELL_KNOWN.href,
      `${JMAP}/jmap/session`,
    ]);
    calls.length = 0;
    const client = await connectAs(auth, fake, "token-b", () => 1000);
    // One request, with the second user's own token.
    expect(calls).toStrictEqual([
      { auth: "Bearer token-b", url: `${JMAP}/jmap/session` },
    ]);
    expect(client.session.username).toBe("[EMAIL]");
  });

  it("finds the session again after the TTL", async () => {
    const { calls, fake } = server(`${JMAP}/jmap/session`);
    await connectAs(auth, fake, "t", () => 0);
    calls.length = 0;
    await connectAs(auth, fake, "t", () => 11 * 60 * 1000);
    expect(calls[0]?.url).toBe(WELL_KNOWN.href);
  });

  it("starts over once when the remembered URL is gone", async () => {
    const { calls, fake, move } = server(`${JMAP}/jmap/session`);
    await connectAs(auth, fake, "t", () => 0);
    move();
    calls.length = 0;
    await expect(connectAs(auth, fake, "t", () => 1)).rejects.toThrow(
      JmapError
    );
    expect(calls.map(({ url }) => url)).toStrictEqual([
      `${JMAP}/jmap/session`,
      WELL_KNOWN.href,
      `${JMAP}/jmap/session`,
    ]);
  });

  it("never remembers another origin", async () => {
    const { calls, fake } = server("https://other.example.net/jmap/session");
    await expect(connectAs(auth, fake, "t", () => 0)).rejects.toThrow(
      JmapError
    );
    calls.length = 0;
    await expect(connectAs(auth, fake, "t", () => 1)).rejects.toThrow(
      JmapError
    );
    expect(calls[0]?.url).toBe(WELL_KNOWN.href);
  });
});
