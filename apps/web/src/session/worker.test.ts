// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The session worker (static build), run as a SharedWorker with a fake
// global scope: a real MessageChannel stands in for the tab's port.

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

import type { ProviderMetadata, TokenResult } from "./oauth";
import { EventSchema, ReplySchema } from "./protocol";
import type { WorkerReply } from "./protocol";

const ORIGIN = "https://mail.example";

const oauth = vi.hoisted(() => ({
  revokeRequest:
    vi.fn<
      (
        endpoint: URL,
        params: { readonly clientId: string; readonly refreshToken: string }
      ) => Promise<void>
    >(),
  tokenRequest:
    vi.fn<
      (
        endpoint: URL,
        params: Readonly<Record<string, string>>
      ) => Promise<TokenResult>
    >(),
}));

vi.mock(import("./oauth"), async (importOriginal) => ({
  ...(await importOriginal()),
  fetchMetadata: vi.fn<() => Promise<ProviderMetadata>>().mockResolvedValue({
    authorization_endpoint: `${ORIGIN}/authorize`,
    issuer: ORIGIN,
    revocation_endpoint: `${ORIGIN}/revoke`,
    token_endpoint: `${ORIGIN}/token`,
  }),
  revokeRequest: oauth.revokeRequest,
  tokenRequest: oauth.tokenRequest,
}));

const tokens = (refreshToken: string, accessToken = "A"): TokenResult => ({
  ok: true,
  tokens: { accessToken, expiresAt: Date.now() + 3_600_000, refreshToken },
});

let tab: MessagePort;
let worker: MessagePort;
let nextId = 1;

/** Sends a request from the tab and waits for its reply. */
const ask = async (body: Record<string, unknown>): Promise<WorkerReply> => {
  const id = nextId;
  nextId += 1;
  const { promise, resolve } = Promise.withResolvers<WorkerReply>();
  const onMessage = (event: MessageEvent): void => {
    const reply = ReplySchema.safeParse(event.data);
    if (reply.success && reply.data.id === id) {
      tab.removeEventListener("message", onMessage);
      resolve(reply.data);
    }
  };
  tab.addEventListener("message", onMessage);
  // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a MessagePort, not a window: there is no target origin.
  tab.postMessage({ ...body, id });
  return await promise;
};

const complete = async (): Promise<WorkerReply> =>
  await ask({
    clientId: "nebulamail",
    code: "code",
    op: "complete",
    redirectUri: `${ORIGIN}/login`,
    verifier: "v".repeat(43),
  });

/** The events the worker sends to the tab from now on. */
const recordEvents = (): { events: string[]; stop: () => void } => {
  const events: string[] = [];
  const onMessage = (event: MessageEvent): void => {
    const parsed = EventSchema.safeParse(event.data);
    if (parsed.success) {
      events.push(parsed.data.event);
    }
  };
  tab.addEventListener("message", onMessage);
  return {
    events,
    stop: () => {
      tab.removeEventListener("message", onMessage);
    },
  };
};

/** Logged out, with no revocation recorded yet. */
const loggedOut = async (): Promise<void> => {
  await ask({ op: "logout" });
  oauth.revokeRequest.mockReset().mockResolvedValue();
};

const revoked = (): string[] =>
  oauth.revokeRequest.mock.calls.map(([, params]) => params.refreshToken);

describe("session worker", () => {
  beforeAll(async () => {
    const target = new EventTarget();
    vi.stubGlobal("location", { origin: ORIGIN });
    vi.stubGlobal("onconnect", null);
    vi.stubGlobal("addEventListener", target.addEventListener.bind(target));
    await import("./worker");
    const channel = new MessageChannel();
    tab = channel.port1;
    worker = channel.port2;
    target.dispatchEvent(new MessageEvent("connect", { ports: [worker] }));
    tab.start();
  });

  afterAll(() => {
    tab.close();
    worker.close();
    vi.unstubAllGlobals();
  });

  it("revokes the refresh token rotated during a logout", async () => {
    oauth.revokeRequest.mockReset().mockResolvedValue();
    oauth.tokenRequest.mockReset().mockResolvedValueOnce(tokens("R1"));
    await expect(complete()).resolves.toMatchObject({ ok: true });

    const held = Promise.withResolvers<TokenResult>();
    oauth.tokenRequest.mockReturnValueOnce(held.promise);
    const refreshing = ask({ forceRefresh: true, op: "grant" });
    // The refresh is waiting for the server when the logout arrives.
    await vi.waitFor(() => {
      expect(oauth.tokenRequest).toHaveBeenCalledTimes(2);
    });
    await expect(ask({ op: "logout" })).resolves.toMatchObject({ ok: true });
    held.resolve(tokens("R2"));
    await expect(refreshing).resolves.toMatchObject({
      error: "expired",
      ok: false,
    });
    await vi.waitFor(() => {
      expect(revoked()).toStrictEqual(["R1", "R2"]);
    });
  });

  it("revokes the previous session after a new login", async () => {
    await loggedOut();
    oauth.tokenRequest
      .mockReset()
      .mockResolvedValueOnce(tokens("first"))
      .mockResolvedValueOnce(tokens("second", "B"));
    await expect(complete()).resolves.toMatchObject({ ok: true });
    await expect(complete()).resolves.toMatchObject({ ok: true });
    expect(revoked()).toStrictEqual(["first"]);
    // The new grant exists before the previous one is revoked (Stalwart
    // revokes only that grant, e2e-static/static.e2e.ts).
    const [, tokenOrder] = oauth.tokenRequest.mock.invocationCallOrder;
    const [revokeOrder] = oauth.revokeRequest.mock.invocationCallOrder;
    expect(tokenOrder).toBeLessThan(revokeOrder ?? 0);
    // And the new session is the one that answers.
    await expect(
      ask({ forceRefresh: false, op: "grant" })
    ).resolves.toMatchObject({
      ok: true,
      value: { accessToken: "B" },
    });
  });

  it("tells the tabs the previous account left before the new one", async () => {
    await loggedOut();
    oauth.tokenRequest
      .mockReset()
      .mockResolvedValueOnce(tokens("alice"))
      .mockResolvedValueOnce(tokens("bob"));
    await expect(complete()).resolves.toMatchObject({ ok: true });
    const seen = recordEvents();
    await expect(complete()).resolves.toMatchObject({ ok: true });
    await vi.waitFor(() => {
      expect(seen.events).toStrictEqual(["loggedOut", "loggedIn"]);
    });
    seen.stop();
  });

  it("revokes the rotated token when a refresh runs during the exchange", async () => {
    await loggedOut();
    oauth.tokenRequest.mockReset().mockResolvedValueOnce(tokens("R1"));
    await expect(complete()).resolves.toMatchObject({ ok: true });

    // The refresh waits for the server while the new login's code is
    // exchanged; it rotates R1 into R2 after the new session is in place.
    const held = Promise.withResolvers<TokenResult>();
    oauth.tokenRequest.mockReturnValueOnce(held.promise);
    const refreshing = ask({ forceRefresh: true, op: "grant" });
    await vi.waitFor(() => {
      expect(oauth.tokenRequest).toHaveBeenCalledTimes(2);
    });
    oauth.tokenRequest.mockResolvedValueOnce(tokens("N1", "N"));
    await expect(complete()).resolves.toMatchObject({ ok: true });
    held.resolve(tokens("R2", "old"));
    // The page gets the new session, not the replaced one.
    await expect(refreshing).resolves.toMatchObject({
      ok: true,
      value: { accessToken: "N" },
    });
    await vi.waitFor(() => {
      expect(revoked().toSorted()).toStrictEqual(["R1", "R2"]);
    });
  });

  it("revokes the token a refresh rotated before the new login", async () => {
    await loggedOut();
    oauth.tokenRequest.mockReset().mockResolvedValueOnce(tokens("N1", "N"));
    await expect(complete()).resolves.toMatchObject({ ok: true });
    // The rotated token is the one revoked, not the session's first one.
    oauth.tokenRequest.mockResolvedValueOnce(tokens("N2", "N"));
    await expect(
      ask({ forceRefresh: true, op: "grant" })
    ).resolves.toMatchObject({ ok: true });
    oauth.tokenRequest.mockResolvedValueOnce(tokens("M1", "M"));
    await expect(complete()).resolves.toMatchObject({ ok: true });
    expect(revoked()).toStrictEqual(["N2"]);
  });

  it("ends and revokes the previous session when the exchange fails", async () => {
    await loggedOut();
    oauth.tokenRequest.mockReset().mockResolvedValueOnce(tokens("kept"));
    await expect(complete()).resolves.toMatchObject({ ok: true });
    const seen = recordEvents();
    oauth.tokenRequest.mockResolvedValueOnce({
      error: "invalid_grant",
      kind: "rejected",
      ok: false,
    });
    await expect(complete()).resolves.toMatchObject({
      error: "invalid",
      ok: false,
    });
    expect(revoked()).toStrictEqual(["kept"]);
    await vi.waitFor(() => {
      expect(seen.events).toStrictEqual(["loggedOut"]);
    });
    seen.stop();
    await expect(ask({ op: "status" })).resolves.toMatchObject({
      ok: true,
      value: { authenticated: false },
    });
  });
});
