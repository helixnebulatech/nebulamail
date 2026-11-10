// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { JmapClient, WEBPUSH_VAPID } from "@nebulamail/jmap-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { session } from "../../e2e/fake-jmap";
import {
  VERIFY_WAIT_MS,
  deviceId,
  needsRenewal,
  sameKey,
  webPushAvailable,
} from "./web-push";

vi.mock(import("../mail/client"), () => ({
  // A real client on the browser tests' fake session, with a VAPID key.
  getClient: async () => {
    const fake = session();
    return await JmapClient.connect({
      fetch: async () =>
        await Promise.resolve(
          Response.json({
            ...fake,
            capabilities: {
              ...fake.capabilities,
              [WEBPUSH_VAPID]: { applicationServerKey: "B".repeat(87) },
            },
          })
        ),
      getAccessToken: async () => await Promise.resolve("token"),
      sessionUrl: "http://jmap.test/.well-known/jmap",
    });
  },
  resetClient: vi.fn<() => void>(),
}));

const subscription = (key: number[] | null) => ({
  options: {
    applicationServerKey: key === null ? null : new Uint8Array(key).buffer,
    userVisibleOnly: true,
  },
});

describe(sameKey, () => {
  it("tells a subscription made for another VAPID key", () => {
    const key = new Uint8Array([4, 1, 2, 3]);
    expect(sameKey(subscription([4, 1, 2, 3]), key)).toBeTruthy();
    // The server rotated its key: subscribe again.
    expect(sameKey(subscription([4, 1, 2, 9]), key)).toBeFalsy();
    expect(sameKey(subscription([4, 1, 2]), key)).toBeFalsy();
    expect(sameKey(subscription(null), key)).toBeFalsy();
  });
});

describe(needsRenewal, () => {
  const NOW = Date.parse("2026-11-08T12:00:00Z");
  const inDays = (days: number): string =>
    new Date(NOW + days * 24 * 60 * 60_000).toISOString();
  const verified = { expires: inDays(6), verificationCode: "code" };

  it("renews a missing, lost or expiring subscription", () => {
    expect([
      needsRenewal(undefined, false, NOW, 0),
      needsRenewal(verified, true, NOW, 0),
      needsRenewal({ ...verified, expires: inDays(1) }, false, NOW, 0),
      needsRenewal({ ...verified, expires: null }, false, NOW, 0),
      needsRenewal(verified, false, NOW, 0),
    ]).toStrictEqual([true, true, true, true, false]);
  });

  // RFC 8620 §7.2.2: the server pushes nothing else until it is verified.
  it("renews one still unverified once its verification had time to come", () => {
    const unverified = { ...verified, verificationCode: null };

    expect([
      needsRenewal(unverified, false, NOW, NOW - VERIFY_WAIT_MS),
      needsRenewal({ expires: inDays(6) }, false, NOW, 0),
      // Just made (maybe by another tab): its code may be on its way.
      needsRenewal(unverified, false, NOW, NOW - 1000),
    ]).toStrictEqual([true, true, false]);
  });
});

describe(deviceId, () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  it("keeps the id it stored", () => {
    const stored = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => stored.set(key, value),
    });
    const first = deviceId();
    expect(first).toMatch(/^nebula-[\da-f-]{36}$/u);
    expect(deviceId()).toBe(first);
    expect([...stored.values()]).toStrictEqual([first]);
  });

  it("without storage, a random id per page load, never a fixed one", async () => {
    // Storage blocked (strict privacy mode).
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    });
    const first = deviceId();
    expect(first).toMatch(/^nebula-[\da-f-]{36}$/u);
    // The same for the whole load (subscribe and its renewals).
    expect(deviceId()).toBe(first);
    // Another browser (or load) gets another: RFC 8620 §7.2 wants them
    // different, since one replaces the other's subscription.
    const again = await import("./web-push");
    expect(again.deviceId()).not.toBe(first);
  });
});

describe(webPushAvailable, () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
  });

  it("finds the worker registered at the app's base (static build)", async () => {
    vi.stubEnv("BASE_URL", "/nebulamail/");
    const getRegistration = vi.fn<
      (scope?: string) => Promise<object | undefined>
    >(
      async (scope) =>
        await Promise.resolve(scope === "/nebulamail/" ? {} : undefined)
    );
    vi.stubGlobal("navigator", { serviceWorker: { getRegistration } });
    // Only their presence is checked.
    vi.stubGlobal("PushManager", {});
    vi.stubGlobal("Notification", {});

    await expect(webPushAvailable()).resolves.toBeTruthy();
    expect(getRegistration).toHaveBeenCalledWith("/nebulamail/");
  });
});
