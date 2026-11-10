// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { inProcessExtractor } from "@nebulamail/mail-render/extract";
import { JSDOM } from "jsdom";
import { describe, expect, it, vi } from "vitest";
import { z } from "zod";

import type { FetchImage, ProxyConfig, ProxyLogEvent } from "../index";
import {
  createMemoryStateStore,
  createMemoryStorage,
  createProxyHandler,
  createRecipientHasher,
  preloadKey,
} from "../index";
import { APP_ORIGIN, keys, NOW, png } from "../test-fixtures";
import { hashUrl } from "../tickets/ticket";
import { URL_HOOK_PATH } from "./url-hook";

const BASE = "https://img.example.com";
const SECRET = "s".repeat(40);
const URL_A = "https://images.example.com/a.png";
const TRACKER = "https://www.google-analytics.com/collect?v=1";

const recipient = async (address: string): Promise<string> =>
  await createRecipientHasher(SECRET)(address);

const setup = async (preload: Partial<ProxyConfig["preload"]> = {}) => {
  const storage = createMemoryStorage({ now: () => NOW });
  const calls: string[] = [];
  const fetchImage: FetchImage = async (url) => {
    calls.push(url);
    await Promise.resolve();
    return url === URL_A
      ? { bytes: png(), type: "image/png" }
      : { error: "dns" };
  };
  const events: ProxyLogEvent[] = [];
  const jobs: Promise<unknown>[] = [];
  const handler = createProxyHandler({
    appOrigin: APP_ORIGIN,
    fetchImage,
    keys: await keys(),
    log: (event) => {
      events.push(event);
    },
    now: () => NOW,
    preload: { secret: SECRET, state: createMemoryStateStore(), ...preload },
    storage,
  });
  const send = async (request: Request) => {
    const response = await handler(request, {
      waitUntil: (job) => {
        jobs.push(job);
      },
    });
    await Promise.all(jobs);
    return response;
  };
  return { calls, events, handler, send, storage };
};

const urlHook = (body: unknown, token: string | null = SECRET): Request =>
  new Request(`${BASE}${URL_HOOK_PATH}`, {
    body: JSON.stringify(body),
    headers: {
      "content-type": "application/json",
      ...(token === null ? {} : { authorization: `Bearer ${token}` }),
    },
    method: "POST",
  });

describe("POST /hooks/preload-urls (relay)", () => {
  it("preloads the URLs for hashed recipients, without a parser", async () => {
    const { send, calls, storage } = await setup();
    const response = await send(
      urlHook({
        recipients: [await recipient("alice@example.org")],
        urls: [URL_A],
        v: 1,
      })
    );
    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toStrictEqual({});
    expect(calls).toStrictEqual([URL_A]);
    await expect(storage.get(await preloadKey(URL_A))).resolves.toMatchObject({
      type: "image/png",
    });
  });

  it("preloads without waitUntil too, as on Node", async () => {
    const { calls, handler } = await setup();
    await handler(
      urlHook({
        recipients: [await recipient("alice@example.org")],
        urls: [URL_A],
        v: 1,
      })
    );
    await vi.waitFor(() => {
      expect(calls).toStrictEqual([URL_A]);
    });
  });

  it("checks the URLs again: trackers, other schemes and credentials are dropped", async () => {
    const { send, calls } = await setup();
    await send(
      urlHook({
        recipients: [await recipient("alice@example.org")],
        urls: [
          TRACKER,
          "data:image/png;base64,iVBORw0KGgo=",
          "https://user:pass@images.example.com/x.png",
          "not a url",
          URL_A,
          URL_A,
        ],
        v: 1,
      })
    );
    expect(calls).toStrictEqual([URL_A]);
  });

  it("refuses a missing or wrong token", async () => {
    const { send, calls } = await setup();
    const body = {
      recipients: [await recipient("alice@example.org")],
      urls: [URL_A],
      v: 1,
    };
    await expect(send(urlHook(body, null))).resolves.toMatchObject({
      status: 401,
    });
    await expect(send(urlHook(body, `${SECRET}x`))).resolves.toMatchObject({
      status: 401,
    });
    expect(calls).toStrictEqual([]);
  });

  it.each([
    [
      "an address instead of a hash",
      { recipients: ["alice@example.org"], urls: [URL_A], v: 1 },
    ],
    ["no recipient", { recipients: [], urls: [URL_A], v: 1 }],
    [
      "an unknown field",
      { extra: 1, recipients: ["a".repeat(64)], urls: [URL_A], v: 1 },
    ],
    ["another version", { recipients: ["a".repeat(64)], urls: [URL_A], v: 2 }],
  ])("400 for %s", async (_name, body) => {
    const { send, calls } = await setup();
    await expect(send(urlHook(body))).resolves.toMatchObject({ status: 400 });
    expect(calls).toStrictEqual([]);
  });

  it("gives an email token when asked and tokens are on", async () => {
    const { send, handler } = await setup({ emailTokens: true });
    const response = await send(
      urlHook({
        recipients: [await recipient("alice@example.org")],
        token: true,
        urls: [URL_A],
        v: 1,
      })
    );
    const { token } = z
      .object({ token: z.string() })
      .parse(await response.json());
    const image = await handler(
      new Request(`${BASE}/p/${token}/${await hashUrl(URL_A)}`)
    );
    expect(image.status).toBe(200);
  });

  it("gives no token when no recipient takes more preloaded images", async () => {
    const { send } = await setup({
      emailTokens: true,
      maxImagesPerRecipient: 1,
    });
    const request = {
      recipients: [await recipient("alice@example.org")],
      token: true,
      urls: [URL_A],
      v: 1,
    };
    const first = await send(urlHook(request));
    await expect(first.json()).resolves.toHaveProperty("token");
    const second = await send(urlHook(request));
    await expect(second.json()).resolves.toStrictEqual({});
  });

  it("gives no token when tokens are off", async () => {
    const { send } = await setup();
    const response = await send(
      urlHook({
        recipients: [await recipient("alice@example.org")],
        token: true,
        urls: [URL_A],
        v: 1,
      })
    );
    await expect(response.json()).resolves.toStrictEqual({});
  });
});

describe("POST /hooks/stalwart (the whole email) is opt-in", () => {
  const email = JSON.stringify({
    context: { stage: "DATA" },
    envelope: { to: [{ address: "alice@example.org" }] },
    message: {
      contents: `<img src="${URL_A}">`,
      headers: [["Content-Type", " text/html"]],
    },
  });
  const full = (): Request =>
    new Request(`${BASE}/hooks/stalwart`, {
      body: email,
      headers: {
        authorization: `Bearer ${SECRET}`,
        "content-type": "application/json",
      },
      method: "POST",
    });

  it("is 404 by default, and the proxy needs no HTML parser", async () => {
    const { send, calls, events } = await setup();
    await expect(send(full())).resolves.toMatchObject({ status: 404 });
    expect(calls).toStrictEqual([]);
    expect(events).not.toContainEqual({ event: "preload-full-email" });
  });

  it("with fullEmail it works and warns once at start", async () => {
    const extractor = inProcessExtractor(new new JSDOM("").window.DOMParser());
    const { send, calls, events } = await setup({ extractor, fullEmail: true });
    expect(events).toStrictEqual([{ event: "preload-full-email" }]);
    await expect(send(full())).resolves.toMatchObject({ status: 200 });
    expect(calls).toStrictEqual([URL_A]);
  });

  it("fullEmail without an extractor is a configuration error", async () => {
    await expect(setup({ fullEmail: true })).rejects.toThrow(/HTML extractor/u);
  });
});
