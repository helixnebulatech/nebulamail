// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { inProcessExtractor } from "@nebulamail/mail-render/extract";
import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";
import { z } from "zod";

import type { FetchImage } from "../index";
import {
  createMemoryStateStore,
  createMemoryStorage,
  createProxyHandler,
  createRecipientHasher,
  preloadKey,
} from "../index";
import { MAX_TOKEN_HTML_CHARS } from "../preload/parse";
import { APP_ORIGIN, keys, NOW, png } from "../test-fixtures";
import { hashUrl } from "../tickets/ticket";
import type { RelayConfig, RelayLogEvent } from "./relay";
import { createRelay, RelayConfigError } from "./relay";

const PROXY = "https://img.example.com";
const RELAY = "http://127.0.0.1:3004";
const PROXY_SECRET = "p".repeat(40);
const HOOK_SECRET = "h".repeat(40);
const URL_A = "https://images.example.com/a.png";
const TRACKER = "https://www.google-analytics.com/collect?v=1";
const SUBJECT = "Secret subject 4471";
const TEXT = "the private text of the email";

const extractor = inProcessExtractor(new new JSDOM("").window.DOMParser());

/** An email as Stalwart's MTA Hook sends it. */
const email = (
  html: string,
  headers: [string, string][] = [["Content-Type", " text/html"]],
  to = ["Alice+news@Example.org"]
): string =>
  JSON.stringify({
    context: { stage: "DATA" },
    envelope: {
      from: { address: "news@sender.example" },
      to: to.map((address) => ({ address })),
    },
    message: {
      contents: html,
      headers: [["Subject", ` ${SUBJECT}`], ...headers],
      size: html.length,
    },
  });

const hook = (body: string, token = HOOK_SECRET): Request =>
  new Request(`${RELAY}/hooks/stalwart`, {
    body,
    headers: {
      authorization: `Bearer ${token}`,
      "content-type": "application/json",
    },
    method: "POST",
  });

/** A proxy and a relay wired to it; records what the relay sends. */
const setup = async (
  options: { tokens?: boolean; proxyDown?: boolean } = {},
  relay: Partial<RelayConfig> = {}
) => {
  const storage = createMemoryStorage({ now: () => NOW });
  const calls: string[] = [];
  const fetchImage: FetchImage = async (url) => {
    calls.push(url);
    await Promise.resolve();
    return url === URL_A
      ? { bytes: png(), type: "image/png" }
      : { error: "dns" };
  };
  const jobs: Promise<unknown>[] = [];
  const context = {
    waitUntil: (job: Promise<unknown>) => {
      jobs.push(job);
    },
  };
  const proxy = createProxyHandler({
    appOrigin: APP_ORIGIN,
    fetchImage,
    keys: await keys(),
    now: () => NOW,
    preload: {
      secret: PROXY_SECRET,
      state: createMemoryStateStore(),
      ...(options.tokens === true ? { emailTokens: true } : {}),
    },
    storage,
  });
  const sent: string[] = [];
  const events: RelayLogEvent[] = [];
  const handler = createRelay({
    fetch: async (input, init) => {
      if (options.proxyDown === true) {
        throw new TypeError("connection refused");
      }
      const request = new Request(input, init);
      sent.push(await request.clone().text());
      return await proxy(request, context);
    },
    hookSecret: HOOK_SECRET,
    log: (event) => {
      events.push(event);
    },
    extractor,
    upstream: PROXY,
    upstreamSecret: PROXY_SECRET,
    ...(options.tokens === true ? { emailTokens: true } : {}),
    ...relay,
  });
  const send = async (request: Request) => {
    const response = await handler(request, context);
    // Settle background sends, then the proxy's own background preloads.
    await Promise.all(jobs);
    await Promise.all(jobs);
    return response;
  };
  return { calls, events, proxy, send, sent, storage };
};

const Modifications = z.object({
  action: z.literal("accept"),
  modifications: z.array(z.record(z.string(), z.unknown())).optional(),
});

describe("preload relay", () => {
  it("accepts at once and the proxy preloads the images", async () => {
    const { send, calls, storage } = await setup();
    const response = await send(
      hook(email(`<p>${TEXT}</p><img src="${URL_A}"><img src="${TRACKER}">`))
    );
    await expect(response.json()).resolves.toStrictEqual({ action: "accept" });
    expect(calls).toStrictEqual([URL_A]);
    await expect(storage.get(await preloadKey(URL_A))).resolves.toMatchObject({
      type: "image/png",
    });
  });

  it("sends the proxy only the image URLs and hashed recipients", async () => {
    const { send, sent } = await setup();
    await send(hook(email(`<p>${TEXT}</p><img src="${URL_A}">`)));
    expect(sent).toHaveLength(1);
    const body = JSON.parse(sent[0] ?? "") as unknown;
    expect(body).toStrictEqual({
      // Normalized like the proxy does (`+tag` and case), then hashed with
      // the proxy's index key: an opt-out made in the app applies.
      recipients: [
        await createRecipientHasher(PROXY_SECRET)("alice@example.org"),
      ],
      urls: [URL_A],
      v: 1,
    });
    for (const secret of [SUBJECT, TEXT, "alice", "sender.example", TRACKER]) {
      expect(sent[0]).not.toContain(secret);
    }
  });

  it("uses the proxy's index key when it has one", async () => {
    const indexKey = "i".repeat(40);
    const { send, sent } = await setup({}, { indexKey });
    await send(hook(email(`<img src="${URL_A}">`)));
    expect(sent[0]).toContain(
      await createRecipientHasher(indexKey)("alice@example.org")
    );
  });

  it("sends nothing for an email without images, a bad body or too many recipients", async () => {
    const { send, sent } = await setup();
    const many = Array.from(
      { length: 101 },
      (_, i) => `u${String(i)}@x.example`
    );
    for (const body of [
      email("<p>no images</p>"),
      "{",
      email(`<img src="${URL_A}">`, undefined, many),
      email(`<img src="${URL_A}">`, undefined, []),
    ]) {
      // oxlint-disable-next-line no-await-in-loop -- one email at a time.
      const response = await send(hook(body));
      // oxlint-disable-next-line no-await-in-loop -- same.
      await expect(response.json()).resolves.toStrictEqual({
        action: "accept",
      });
    }
    expect(sent).toStrictEqual([]);
  });

  it("refuses a wrong token and does not send anything", async () => {
    const { send, sent } = await setup();
    const response = await send(
      hook(email(`<img src="${URL_A}">`), `${HOOK_SECRET}x`)
    );
    expect(response.status).toBe(401);
    expect(sent).toStrictEqual([]);
  });

  it("still accepts when the proxy is down", async () => {
    const { send, events } = await setup({ proxyDown: true });
    const response = await send(hook(email(`<img src="${URL_A}">`)));
    await expect(response.json()).resolves.toStrictEqual({ action: "accept" });
    expect(events).toContainEqual({ event: "relay-upstream-error", status: 0 });
  });

  it("logs counts only, never URLs or addresses", async () => {
    const { send, events } = await setup();
    await send(hook(email(`<img src="${URL_A}">`)));
    expect(events).toStrictEqual([
      { event: "relay-sent", images: 1, recipients: 1, token: false },
    ]);
  });

  it("is 404 on other paths; healthz answers", async () => {
    const relay = createRelay({
      hookSecret: HOOK_SECRET,
      extractor,
      upstream: PROXY,
      upstreamSecret: PROXY_SECRET,
    });
    await expect(relay(new Request(`${RELAY}/batch`))).resolves.toMatchObject({
      status: 404,
    });
    await expect(relay(new Request(`${RELAY}/healthz`))).resolves.toMatchObject(
      { status: 200 }
    );
  });
});

describe("preload relay with email tokens", () => {
  it("puts the proxy's token in the email and removes forged ones", async () => {
    const { send, proxy } = await setup({ tokens: true });
    const response = await send(
      hook(
        email(`<img src="${URL_A}">`, [
          ["X-Nebula-Preload", " 1.1.forged"],
          ["Content-Type", " text/html"],
        ])
      )
    );
    const body = Modifications.parse(await response.json());
    expect(body.modifications?.[0]).toStrictEqual({
      index: 1,
      name: "X-Nebula-Preload",
      type: "deleteHeader",
    });
    const added = body.modifications?.[1];
    expect(added?.type).toBe("addHeader");
    const token = z.string().parse(added?.value);
    const image = await proxy(
      new Request(`${PROXY}/p/${token}/${await hashUrl(URL_A)}`)
    );
    expect(image.status).toBe(200);
  });

  it("too much HTML: answers at once, without a token, and still preloads", async () => {
    const { send, sent, events } = await setup({ tokens: true });
    const big = `<img src="${URL_A}">${"x".repeat(MAX_TOKEN_HTML_CHARS)}`;
    const response = await send(hook(email(big)));
    await expect(response.json()).resolves.toStrictEqual({ action: "accept" });
    expect(events).toContainEqual({
      event: "relay-token-skipped",
      reason: "too-large",
    });
    expect(sent).toHaveLength(1);
    expect(sent[0]).not.toContain('"token"');
  });

  it("without the proxy, only removes forged tokens", async () => {
    const { send } = await setup({ proxyDown: true, tokens: true });
    const response = await send(
      hook(
        email(`<img src="${URL_A}">`, [
          ["X-Nebula-Preload", " 1.1.forged"],
          ["Content-Type", " text/html"],
        ])
      )
    );
    await expect(response.json()).resolves.toStrictEqual({
      action: "accept",
      modifications: [
        { index: 1, name: "X-Nebula-Preload", type: "deleteHeader" },
      ],
    });
  });
});

describe("preload relay configuration", () => {
  const base: RelayConfig = {
    hookSecret: HOOK_SECRET,
    extractor,
    upstream: PROXY,
    upstreamSecret: PROXY_SECRET,
  };

  it.each([
    ["the same secret both ways", { hookSecret: PROXY_SECRET }],
    ["a short hook secret", { hookSecret: "short" }],
    ["a short proxy secret", { upstreamSecret: "short" }],
    ["http to another machine", { upstream: "http://img.example.com" }],
    [
      "credentials in the upstream",
      { upstream: "https://u:p@img.example.com" },
    ],
    ["not a URL", { upstream: "img" }],
  ] satisfies [string, Partial<RelayConfig>][])("refuses %s", (_name, bad) => {
    expect(() => createRelay({ ...base, ...bad })).toThrow(RelayConfigError);
  });

  it("allows http on loopback", () => {
    expect(() =>
      createRelay({ ...base, upstream: "http://127.0.0.1:3003" })
    ).not.toThrow();
  });
});
