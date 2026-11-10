// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { CAPABILITIES } from "./capabilities";
import { JmapClient } from "./client";
import { AccountId } from "./ids";
import {
  hasCapability,
  mailCapability,
  parseSession,
  sendsWhenSignedOut,
  serverAlerts,
} from "./session";
import {
  ORIGIN,
  SESSION_URL,
  errorOf,
  fakeFetch,
  rejection,
  stalwartSession,
  tokens,
} from "./test-support";
import { makeOriginPolicy } from "./url";

const base = new URL(SESSION_URL);
const policy = makeOriginPolicy(base, false);

// Browsers expose manual redirects as an opaque response.
const opaque = (): Response => {
  const response = new Response(null, { status: 200 });
  Object.defineProperty(response, "type", { value: "opaqueredirect" });
  return response;
};
const probed = (): Response => {
  const response = new Response("unauthorized", { status: 401 });
  Object.defineProperty(response, "url", {
    value: `${ORIGIN}/jmap/session`,
  });
  return response;
};

describe(parseSession, () => {
  it("parses a Stalwart session with typed core limits", () => {
    const session = parseSession(stalwartSession(), base, policy);
    expect(session.core.maxCallsInRequest).toBe(16);
    expect(session.core.maxSizeUpload).toBe(50_000_000);
    expect(session.apiUrl.href).toBe(`${ORIGIN}/jmap/`);
    expect(session.primaryAccounts[CAPABILITIES.mail]).toBe("d");
    expect(session.username).toBe("jane@example.com");
  });

  it("answers capability questions per server and per account", () => {
    const session = parseSession(stalwartSession(), base, policy);
    const personal = AccountId.parse("d");
    const shared = AccountId.parse("e");
    expect(hasCapability(session, CAPABILITIES.sieve)).toBeTruthy();
    expect(hasCapability(session, CAPABILITIES.sieve, personal)).toBeTruthy();
    expect(hasCapability(session, CAPABILITIES.sieve, shared)).toBeFalsy();
    expect(hasCapability(session, CAPABILITIES.mailShare)).toBeFalsy();
    expect(mailCapability(session, personal)?.maxMailboxDepth).toBe(10);
  });

  it("says whether the bridge sends held mail after sign-out", () => {
    const plain = parseSession(stalwartSession(), base, policy);
    const withBridge = (bridge: unknown) => ({
      ...plain,
      capabilities: { ...plain.capabilities, [CAPABILITIES.bridge]: bridge },
    });
    expect(sendsWhenSignedOut(plain)).toBeFalsy();
    expect(sendsWhenSignedOut(withBridge({}))).toBeFalsy();
    expect(
      sendsWhenSignedOut(withBridge({ sendWhenSignedOut: "yes" }))
    ).toBeFalsy();
    expect(
      sendsWhenSignedOut(withBridge({ sendWhenSignedOut: true }))
    ).toBeTruthy();
  });

  it("reads the mail server's alerts of an account (RFC 9051 §7.1)", () => {
    const plain = parseSession(stalwartSession(), base, policy);
    const personal = AccountId.parse("d");
    const withAlerts = (alerts: unknown) => {
      const account = plain.accounts[personal];
      if (account === undefined) {
        throw new Error("no account");
      }
      return {
        ...plain,
        accounts: {
          ...plain.accounts,
          [personal]: {
            ...account,
            accountCapabilities: {
              ...account.accountCapabilities,
              [CAPABILITIES.bridge]: { alerts },
            },
          },
        },
      };
    };
    const good = {
      id: "a1",
      receivedAt: "2026-11-06T10:00:00Z",
      text: "Mailbox 95% full",
    };
    expect(serverAlerts(plain, personal)).toStrictEqual([]);
    expect(serverAlerts(withAlerts("x"), personal)).toStrictEqual([]);
    expect(
      serverAlerts(
        withAlerts([good, { id: "", text: "x" }, { ...good, text: 3 }]),
        personal
      )
    ).toStrictEqual([good]);
    expect(
      serverAlerts(withAlerts([good]), AccountId.parse("e"))
    ).toStrictEqual([]);
  });

  it("requires the core capability", () => {
    const raw = stalwartSession({ capabilities: { [CAPABILITIES.mail]: {} } });
    expect(errorOf(() => parseSession(raw, base, policy)).kind).toBe(
      "invalidResponse"
    );
  });

  it.each([
    ["apiUrl", "https://evil.example/jmap/"],
    ["downloadUrl", "https://evil.example/{accountId}/{blobId}"],
    ["uploadUrl", "https://mail.example.com.evil.example/{accountId}"],
    ["eventSourceUrl", "http://mail.example.com/es?types={types}"],
  ])("rejects a cross-origin %s", (field, url) => {
    const raw = stalwartSession({ [field]: url });
    expect(errorOf(() => parseSession(raw, base, policy)).kind).toBe(
      "crossOrigin"
    );
  });

  it.each([
    ["apiUrl", "data:text/plain,x"],
    ["apiUrl", "ftp://mail.example.com/jmap/"],
    ["apiUrl", "https://user:pass@mail.example.com/jmap/"],
    ["downloadUrl", `${ORIGIN}/download/{accountId}`],
    ["downloadUrl", `${ORIGIN}/download/{accountId}/{+blobId}`],
    ["uploadUrl", `${ORIGIN}/upload/{accountId`],
  ])("rejects an invalid %s (%s)", (field, url) => {
    const raw = stalwartSession({ [field]: url });
    expect(errorOf(() => parseSession(raw, base, policy)).kind).toBe(
      "invalidUrl"
    );
  });

  it("accepts explicitly allowed extra origins", () => {
    const raw = stalwartSession({ apiUrl: "https://api.example.com/jmap/" });
    const allowed = makeOriginPolicy(base, ["https://api.example.com"]);
    expect(parseSession(raw, base, allowed).apiUrl.origin).toBe(
      "https://api.example.com"
    );
  });

  it("resolves relative URLs against the session URL", () => {
    const raw = stalwartSession({ apiUrl: "/jmap/api" });
    expect(parseSession(raw, base, policy).apiUrl.href).toBe(
      `${ORIGIN}/jmap/api`
    );
  });

  it("moves URLs on a server alias to the session's own origin", () => {
    const publicName = "https://mail.example.org";
    const raw = stalwartSession({
      apiUrl: `${publicName}/jmap/`,
      downloadUrl: `${publicName}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`,
      eventSourceUrl: `${publicName}/jmap/eventsource/?types={types}&closeafter={closeafter}&ping={ping}`,
      uploadUrl: `${publicName}/jmap/upload/{accountId}/`,
    });
    const aliased = makeOriginPolicy(base, false, [publicName]);
    const session = parseSession(raw, base, aliased);
    expect(session.apiUrl.href).toBe(`${ORIGIN}/jmap/`);
    expect(session.downloadUrl).toBe(
      `${ORIGIN}/jmap/download/{accountId}/{blobId}/{name}?accept={type}`
    );
    expect(session.uploadUrl).toBe(`${ORIGIN}/jmap/upload/{accountId}/`);
    expect(session.eventSourceUrl.startsWith(`${ORIGIN}/jmap/`)).toBeTruthy();
  });

  it("still rejects origins that are not aliases", () => {
    const raw = stalwartSession({ apiUrl: "https://evil.example/jmap/" });
    const aliased = makeOriginPolicy(base, false, ["https://mail.example.org"]);
    expect(errorOf(() => parseSession(raw, base, aliased)).kind).toBe(
      "crossOrigin"
    );
  });

  it("does not treat a longer host name as an alias", () => {
    const raw = stalwartSession({
      uploadUrl:
        "https://mail.example.org.evil.example/jmap/upload/{accountId}/",
    });
    const aliased = makeOriginPolicy(base, false, ["https://mail.example.org"]);
    expect(errorOf(() => parseSession(raw, base, aliased)).kind).toBe(
      "crossOrigin"
    );
  });
});

describe("JmapClient.connect", () => {
  it("sends the bearer token without cookies or redirects", async () => {
    const { calls, fetch } = fakeFetch(() => Response.json(stalwartSession()));
    const client = await JmapClient.connect({
      fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    expect(client.session.state).toBe("s1");
    expect(calls[0]?.authorization).toBe("Bearer old-token");
    expect(calls[0]?.init.credentials).toBe("omit");
    expect(calls[0]?.init.redirect).toBe("manual");
  });

  it("follows a same-origin redirect itself, re-checking the target", async () => {
    const { calls, fetch } = fakeFetch((call) =>
      call.url === SESSION_URL
        ? new Response(null, {
            headers: { Location: "/jmap/session" },
            status: 307,
          })
        : Response.json(stalwartSession())
    );
    const client = await JmapClient.connect({
      fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    expect(calls.map((call) => call.url)).toStrictEqual([
      SESSION_URL,
      `${ORIGIN}/jmap/session`,
    ]);
    expect(client.session.sessionUrl.href).toBe(`${ORIGIN}/jmap/session`);
  });

  it("never sends the token to a cross-origin redirect target", async () => {
    const { calls, fetch } = fakeFetch(
      () =>
        new Response(null, {
          headers: { Location: "https://evil.example/jmap" },
          status: 301,
        })
    );
    const error = await rejection(
      JmapClient.connect({ fetch, sessionUrl: SESSION_URL, ...tokens() })
    );
    expect(error.kind).toBe("crossOrigin");
    expect(calls.map((call) => call.url)).toStrictEqual([SESSION_URL]);
  });

  it("discovers opaque (browser) redirects with an unauthenticated probe", async () => {
    const { calls, fetch } = fakeFetch((call) => {
      if (call.url !== SESSION_URL) {
        return Response.json(stalwartSession());
      }
      return call.authorization === undefined ? probed() : opaque();
    });
    await JmapClient.connect({ fetch, sessionUrl: SESSION_URL, ...tokens() });
    expect(calls.map((call) => call.authorization)).toStrictEqual([
      "Bearer old-token",
      undefined,
      "Bearer old-token",
    ]);
    expect(calls[2]?.url).toBe(`${ORIGIN}/jmap/session`);
  });

  it("turns a problem document into a typed request error", async () => {
    const { fetch } = fakeFetch(() =>
      Response.json(
        { status: 503, type: "urn:ietf:params:jmap:error:serverUnavailable" },
        { status: 503 }
      )
    );
    const error = await rejection(
      JmapClient.connect({ fetch, sessionUrl: SESSION_URL, ...tokens() })
    );
    expect(error.detail).toStrictEqual({
      kind: "request",
      status: 503,
      type: "urn:ietf:params:jmap:error:serverUnavailable",
    });
  });
});
