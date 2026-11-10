// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Per-recipient quota and opt-out of preload.

import { setTimeout as sleep } from "node:timers/promises";

import { inProcessExtractor } from "@nebulamail/mail-render/extract";
import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";

import type {
  FetchImage,
  ImageStorage,
  ProxyConfig,
  ProxyLogEvent,
  RequestContext,
  PreloadStateStore,
} from "../index";
import {
  createMemoryStateStore,
  createMemoryStorage,
  createProxyHandler,
  createRateLimiter,
  preloadKey,
} from "../index";
import { APP_ORIGIN, keys, NOW, png, ticket } from "../test-fixtures";
import { parseTicketKeys, signTicket } from "../tickets/index";
import {
  PREFERENCES_PATH,
  signPreferenceRequest,
} from "../tickets/preload-prefs";
import { createRecipientHasher, normalizeAddress } from "./recipients";
import { createUsageIndex } from "./usage";

const BASE = "https://img.example.com";
const SECRET = "s".repeat(40);
const URL_A = "https://images.example.com/a.png";
const URL_B = "https://images.example.com/b.png";
const URL_C = "https://images.example.com/c.png";
const extractor = inProcessExtractor(new new JSDOM("").window.DOMParser());

const hookBody = (to: readonly string[], html: string): string =>
  JSON.stringify({
    context: { stage: "DATA" },
    envelope: {
      from: { address: "news@example.com" },
      to: to.map((address) => ({ address })),
    },
    message: {
      contents: html,
      headers: [["Content-Type", " text/html; charset=utf-8"]],
    },
  });

const hook = (to: readonly string[], html: string): Request =>
  new Request(`${BASE}/hooks/stalwart`, {
    body: hookBody(to, html),
    headers: {
      authorization: `Bearer ${SECRET}`,
      "content-type": "application/json",
    },
    method: "POST",
  });

/** Wraps a store to record every key and value written. */
const recording = (
  inner: PreloadStateStore
): PreloadStateStore & { readonly writes: string[] } => {
  const writes: string[] = [];
  return {
    delete: inner.delete,
    list: inner.list,
    get: inner.get,
    put: async (key, value) => {
      writes.push(key, value);
      await inner.put(key, value);
    },
    writes,
  };
};

const setup = async (
  options: {
    maxBytesPerRecipient?: number;
    maxImagesPerRecipient?: number;
    rateLimit?: ProxyConfig["rateLimit"];
    wrapStorage?: (storage: ImageStorage) => ImageStorage;
  } = {}
) => {
  const { rateLimit, wrapStorage, ...preloadOptions } = options;
  let now = NOW;
  const storage = createMemoryStorage({ now: () => now });
  const state = recording(createMemoryStateStore());
  const calls: string[] = [];
  const fetchImage: FetchImage = async (url) => {
    calls.push(url);
    return await Promise.resolve({ bytes: png(), type: "image/png" });
  };
  const events: ProxyLogEvent[] = [];
  const k = await keys();
  const handler = createProxyHandler({
    appOrigin: APP_ORIGIN,
    fetchImage,
    keys: k,
    log: (event) => {
      events.push(event);
    },
    now: () => now,
    preload: {
      fullEmail: true,
      extractor,
      secret: SECRET,
      state,
      ...preloadOptions,
    },
    ...(rateLimit === undefined ? {} : { rateLimit }),
    storage: wrapStorage === undefined ? storage : wrapStorage(storage),
  });
  const jobs: Promise<unknown>[] = [];
  const context: RequestContext = {
    waitUntil: (job) => {
      jobs.push(job);
    },
  };
  const send = async (request: Request) => {
    const response = await handler(request, context);
    await Promise.all(jobs);
    return response;
  };
  const prefs = async (
    addresses: string[],
    optOut?: boolean,
    at = now,
    clientAddress?: string
  ) => {
    const signed = await signPreferenceRequest(k, {
      addresses,
      now: at,
      ...(optOut === undefined ? {} : { optOut }),
    });
    const request = new Request(`${BASE}${PREFERENCES_PATH}`, {
      body: signed.body,
      headers: {
        authorization: signed.authorization,
        "content-type": "application/json",
      },
      method: "POST",
    });
    if (clientAddress === undefined) {
      return await send(request);
    }
    const response = await handler(request, { ...context, clientAddress });
    await Promise.all(jobs);
    return response;
  };
  return {
    advance: (ms: number) => {
      now += ms;
    },
    calls,
    events,
    handler,
    k,
    prefs,
    send,
    state,
    storage,
  };
};

describe("recipient addresses", () => {
  it("normalizes case, brackets and +tags; refuses non-addresses", () => {
    expect(normalizeAddress(" <Alice+News@Example.ORG> ")).toBe(
      "alice@example.org"
    );
    expect(normalizeAddress("+x@example.org")).toBe("+x@example.org");
    for (const bad of ["", "alice", "@x", "a@", "a b@x", "a\n@b", "a@b@c"]) {
      expect(normalizeAddress(bad)).toBeNull();
    }
  });

  it("hashes with a key: other key, other hash; never the address", async () => {
    const a = await createRecipientHasher(SECRET)("alice@example.org");
    const b = await createRecipientHasher("t".repeat(40))("alice@example.org");
    expect(a).toMatch(/^[0-9a-f]{64}$/u);
    expect(a).not.toBe(b);
  });
});

describe("preload quota per recipient", () => {
  it("stops preloading a full recipient; others still get theirs", async () => {
    const { send, calls, events } = await setup({ maxImagesPerRecipient: 2 });
    await send(
      hook(["alice@example.org"], `<img src="${URL_A}"><img src="${URL_B}">`)
    );
    expect(calls).toStrictEqual([URL_A, URL_B]);
    calls.length = 0;
    await send(hook(["alice@example.org"], `<img src="${URL_C}">`));
    expect(calls).toStrictEqual([]);
    expect(events).toContainEqual({
      event: "preload-skipped",
      reason: "quota",
    });
    await send(
      hook(["alice@example.org", "bob@example.org"], `<img src="${URL_C}">`)
    );
    expect(calls).toStrictEqual([URL_C]);
    expect(events).toContainEqual(
      expect.objectContaining({ event: "preload", overQuota: 1, recipients: 1 })
    );
  });

  it("counts bytes, and frees the quota when entries expire", async () => {
    const { send, calls, advance } = await setup({ maxBytesPerRecipient: 1 });
    await send(hook(["alice@example.org"], `<img src="${URL_A}">`));
    await send(hook(["alice@example.org"], `<img src="${URL_B}">`));
    expect(calls).toStrictEqual([URL_A]);
    advance(31 * 24 * 60 * 60 * 1000);
    await send(hook(["alice@example.org"], `<img src="${URL_B}">`));
    expect(calls).toStrictEqual([URL_A, URL_B]);
  });

  it("a full recipient's email is then fetched on open (cache mode)", async () => {
    const { send, calls, handler, k } = await setup({
      maxImagesPerRecipient: 1,
    });
    await send(hook(["alice@example.org"], `<img src="${URL_A}">`));
    await send(hook(["alice@example.org"], `<img src="${URL_B}">`));
    expect(calls).toStrictEqual([URL_A]);
    const t = await ticket(k, { url: URL_B });
    const batch = await handler(
      new Request(`${BASE}/batch`, {
        body: JSON.stringify({ tickets: [t] }),
        headers: { "content-type": "application/json", origin: APP_ORIGIN },
        method: "POST",
      })
    );
    await expect(batch.json()).resolves.toStrictEqual({
      images: { [t]: { cached: true } },
    });
    expect(calls).toStrictEqual([URL_A, URL_B]);
  });

  it("skips emails without envelope recipients or with too many", async () => {
    const { send, calls, events } = await setup();
    await send(hook([], `<img src="${URL_A}">`));
    const many = Array.from({ length: 101 }, (_, i) => `u${String(i)}@x.org`);
    await send(hook(many, `<img src="${URL_A}">`));
    expect(calls).toStrictEqual([]);
    expect(events).toContainEqual({
      event: "preload-skipped",
      reason: "no-recipient",
    });
    expect(events).toContainEqual({
      event: "preload-skipped",
      reason: "too-many-recipients",
    });
  });

  it("tells the user a full address, and until when", async () => {
    const day = 24 * 60 * 60 * 1000;
    const { send, prefs, advance } = await setup({ maxImagesPerRecipient: 2 });
    await send(hook(["erin@example.com"], `<img src="${URL_A}">`));
    advance(day);
    await send(hook(["erin@example.com"], `<img src="${URL_B}">`));
    const room = await prefs(["frank@example.com"]);
    await expect(room.json()).resolves.toStrictEqual({
      full: null,
      optOut: false,
    });
    // Full at 2 images: room again when the oldest one (A) expires.
    const full = await prefs(["frank@example.com", "erin@example.com"]);
    await expect(full.json()).resolves.toStrictEqual({
      full: { until: NOW + 30 * day },
      optOut: false,
    });
    // Off: the quota no longer matters.
    const off = await prefs(["erin@example.com"], true);
    await expect(off.json()).resolves.toStrictEqual({
      full: null,
      optOut: true,
    });
  });

  it("stores hashes only, never addresses or URLs", async () => {
    const { send, prefs, state } = await setup();
    await send(hook(["Alice@Example.org"], `<img src="${URL_A}">`));
    await prefs(["bob@example.org"], true);
    const written = state.writes.join("\n");
    expect(written).not.toMatch(/alice|bob|example|images/iu);
    expect(state.writes.length).toBeGreaterThan(0);
  });
});

describe("POST /preload/preferences", () => {
  it("opts out: later emails for that address are not preloaded", async () => {
    const { send, calls, prefs, events } = await setup();
    const response = await prefs(["alice@example.org"], true);
    await expect(response.json()).resolves.toStrictEqual({
      full: null,
      optOut: true,
    });
    await send(hook(["alice+news@EXAMPLE.org"], `<img src="${URL_A}">`));
    expect(calls).toStrictEqual([]);
    expect(events).toContainEqual({
      event: "preload-skipped",
      reason: "opted-out",
    });
    const read = await prefs(["alice@example.org"]);
    await expect(read.json()).resolves.toStrictEqual({
      full: null,
      optOut: true,
    });
    // Opting back in works again.
    await prefs(["alice@example.org"], false, NOW + 1);
    await send(hook(["alice@example.org"], `<img src="${URL_A}">`));
    expect(calls).toStrictEqual([URL_A]);
  });

  it("deletes the images only the opted-out user had, keeps shared ones", async () => {
    const { send, prefs, storage } = await setup();
    await send(hook(["alice@example.org"], `<img src="${URL_A}">`));
    await send(
      hook(["alice@example.org", "bob@example.org"], `<img src="${URL_B}">`)
    );
    await prefs(["alice@example.org"], true);
    await expect(storage.get(await preloadKey(URL_A))).resolves.toBeNull();
    await expect(storage.get(await preloadKey(URL_B))).resolves.not.toBeNull();
    // Once bob opts out too, nobody uses it.
    await prefs(["bob@example.org"], true);
    await expect(storage.get(await preloadKey(URL_B))).resolves.toBeNull();
  });

  it("reads 'off' only when every address is off", async () => {
    const { prefs } = await setup();
    await prefs(["carol@example.org"], true);
    const mixed = await prefs(["carol@example.org", "dave@example.org"]);
    await expect(mixed.json()).resolves.toStrictEqual({
      full: null,
      optOut: false,
    });
    const one = await prefs(["carol@example.org"]);
    await expect(one.json()).resolves.toStrictEqual({
      full: null,
      optOut: true,
    });
  });

  it("answers 'on' when a newer 'on' keeps one address preloaded, and purges only the others", async () => {
    const { send, prefs, storage } = await setup();
    await send(hook(["carol@example.org"], `<img src="${URL_A}">`));
    await send(hook(["dave@example.org"], `<img src="${URL_B}">`));
    // Dave turned it back on later (another device): that one wins for him.
    await prefs(["dave@example.org"], false, NOW + 5000);
    const response = await prefs(
      ["carol@example.org", "dave@example.org"],
      true,
      NOW
    );

    await expect(response.json()).resolves.toStrictEqual({
      full: null,
      optOut: false,
    });
    await expect(storage.get(await preloadKey(URL_A))).resolves.toBeNull();
    await expect(storage.get(await preloadKey(URL_B))).resolves.not.toBeNull();
  });

  it("an opt-out between the lookup and the charge keeps the other user's image", async () => {
    let between: (() => Promise<unknown>) | undefined;
    const { send, handler, k, storage } = await setup({
      wrapStorage: (inner) => ({
        ...inner,
        get: async (key) => {
          const found = await inner.get(key);
          const job = between;
          between = undefined;
          // Alice opts out right after bob's email found the shared entry.
          await job?.();
          return found;
        },
      }),
    });
    await send(hook(["alice@example.org"], `<img src="${URL_A}">`));
    between = async () => {
      // Its own context: the hook's job (bob's email) is waiting on this.
      const signed = await signPreferenceRequest(k, {
        addresses: ["alice@example.org"],
        now: NOW,
        optOut: true,
      });
      const purges: Promise<unknown>[] = [];
      await handler(
        new Request(`${BASE}${PREFERENCES_PATH}`, {
          body: signed.body,
          headers: {
            authorization: signed.authorization,
            "content-type": "application/json",
          },
          method: "POST",
        }),
        {
          waitUntil: (job) => {
            purges.push(job);
          },
        }
      );
      await Promise.all(purges);
    };
    await send(hook(["bob@example.org"], `<img src="${URL_A}">`));
    expect(between).toBeUndefined();
    await expect(storage.get(await preloadKey(URL_A))).resolves.not.toBeNull();
  });

  it("ignores an older (replayed) choice", async () => {
    const { prefs } = await setup();
    await prefs(["alice@example.org"], true, NOW);
    const old = await prefs(["alice@example.org"], false, NOW - 1000);
    await expect(old.json()).resolves.toStrictEqual({
      full: null,
      optOut: true,
    });
  });

  it("refuses unsigned, tampered, wrongly keyed and expired requests", async () => {
    const { send, k, advance } = await setup();
    const signed = await signPreferenceRequest(k, {
      addresses: ["alice@example.org"],
      now: NOW,
      optOut: true,
    });
    const post = async (body: string, authorization?: string) => {
      const response = await send(
        new Request(`${BASE}${PREFERENCES_PATH}`, {
          body,
          headers: authorization === undefined ? {} : { authorization },
          method: "POST",
        })
      );
      return response.status;
    };
    await expect(post(signed.body)).resolves.toBe(401);
    await expect(
      post(signed.body.replace("alice", "bobby"), signed.authorization)
    ).resolves.toBe(401);
    const other = await parseTicketKeys(`1:${"B".repeat(42)}A`);
    const forged = await signPreferenceRequest(other, {
      addresses: ["alice@example.org"],
      now: NOW,
      optOut: true,
    });
    await expect(post(forged.body, forged.authorization)).resolves.toBe(401);
    // A ticket's MAC is not a preference MAC (domain separation).
    const t = await signTicket(
      k,
      {
        accountId: "a",
        emailId: "e",
        expiresAt: NOW + 60_000,
        url: URL_A,
      },
      NOW
    );
    const ticketMac = t.split(".")[3] ?? "";
    await expect(
      post(signed.body, `Nebula-Preload 1.${ticketMac}`)
    ).resolves.toBe(401);
    advance(3 * 60 * 1000 + 1);
    await expect(post(signed.body, signed.authorization)).resolves.toBe(400);
  });

  it("preload-prefs:global is spent only by signed requests", async () => {
    const { handler, prefs } = await setup({
      rateLimit: createRateLimiter({
        rules: {
          "preload-prefs:global": { capacity: 2, refillPerSecond: 0 },
          "preload-prefs:ip": { capacity: 3, refillPerSecond: 0 },
        },
      }),
    });
    for (let i = 0; i < 10; i += 1) {
      // oxlint-disable-next-line no-await-in-loop
      const response = await handler(
        new Request(`${BASE}${PREFERENCES_PATH}`, {
          body: "{}",
          method: "POST",
        }),
        { clientAddress: `203.0.113.${i}` }
      );
      expect(response.status).toBe(401);
    }
    const signed = await prefs(["x@example.com"], true, NOW, "198.51.100.1");
    expect(signed.status).toBe(200);
    // Per IP: the same address goes over its own bucket.
    const statuses: number[] = [];
    for (let i = 0; i < 3; i += 1) {
      // oxlint-disable-next-line no-await-in-loop
      const response = await handler(
        new Request(`${BASE}${PREFERENCES_PATH}`, {
          body: "{}",
          method: "POST",
        }),
        { clientAddress: "203.0.113.200" }
      );
      statuses.push(response.status);
    }
    expect(statuses).toStrictEqual([401, 401, 401]);
    const over = await handler(
      new Request(`${BASE}${PREFERENCES_PATH}`, { body: "{}", method: "POST" }),
      { clientAddress: "203.0.113.200" }
    );
    expect(over.status).toBe(429);
  });

  it("is 404 without preload", async () => {
    const plain = createProxyHandler({
      appOrigin: APP_ORIGIN,
      fetchImage: async () => await Promise.resolve({ error: "dns" }),
      keys: await keys(),
      storage: createMemoryStorage(),
    });
    const response = await plain(
      new Request(`${BASE}${PREFERENCES_PATH}`, { body: "{}", method: "POST" })
    );
    expect(response.status).toBe(404);
  });

  it("logs no address", async () => {
    const { prefs, events } = await setup();
    await prefs(["alice@example.org"], true);
    expect(JSON.stringify(events)).not.toContain("alice");
    expect(events).toContainEqual(
      expect.objectContaining({ event: "preload-preferences", optOut: true })
    );
  });
});

/** The URL hashes left in a recipient's usage record. */
const usageOf = async (
  store: PreloadStateStore,
  recipient: string
): Promise<string[]> => {
  const parsed: unknown = JSON.parse(
    (await store.get(`usage:${recipient}`)) ?? "null"
  );
  const entries =
    typeof parsed === "object" && parsed !== null && "e" in parsed
      ? parsed.e
      : null;
  return typeof entries === "object" && entries !== null
    ? Object.keys(entries)
    : [];
};

describe("usage index: references, purge and sweep", () => {
  const TTL = 1000;
  const ALICE = "a".repeat(64);
  const BOB = "b".repeat(64);
  const IMAGE_X = "c".repeat(64);
  const IMAGE_Y = "d".repeat(64);

  const index = (
    options: {
      store?: PreloadStateStore;
      images?: ImageStorage;
      clock?: { t: number };
    } = {}
  ) => {
    const clock = options.clock ?? { t: NOW };
    const store = options.store ?? createMemoryStateStore();
    const deletes: string[] = [];
    const memory = createMemoryStorage({ now: () => clock.t });
    const images: ImageStorage = options.images ?? {
      ...memory,
      delete: async (key) => {
        deletes.push(key);
        await memory.delete?.(key);
      },
    };
    const usage = createUsageIndex({
      images,
      now: () => clock.t,
      store,
      ttlMs: TTL,
    });
    return { clock, deletes, store, usage };
  };

  it("release without a refs: record never deletes the image", async () => {
    const { deletes, store, usage } = index();
    await store.put(
      `usage:${ALICE}`,
      JSON.stringify({ e: { [IMAGE_X]: [10, NOW + TTL] }, v: 1 })
    );
    await expect(usage.purge(ALICE)).resolves.toBe(0);
    expect(deletes).toStrictEqual([]);
    await expect(store.get(`usage:${ALICE}`)).resolves.toBeNull();
  });

  it("an image is reserved before it is charged: a failed reserve is not charged", async () => {
    const inner = createMemoryStateStore();
    const failing: PreloadStateStore = {
      ...inner,
      put: async (key, value) => {
        if (key === `refs:${IMAGE_Y}`) {
          throw new Error("disk full");
        }
        await inner.put(key, value);
      },
    };
    const { deletes, usage } = index({ store: failing });
    const reserved = await usage.reserve([ALICE, BOB], [IMAGE_X, IMAGE_Y]);
    expect([...reserved]).toStrictEqual([IMAGE_X]);
    await usage.charge([ALICE, BOB], [{ bytes: 10, urlHash: IMAGE_X }]);
    await usage.purge(ALICE);
    expect(deletes).toStrictEqual([]);
    await usage.purge(BOB);
    expect(deletes).toHaveLength(1);
  });

  it("an opt-out racing with another recipient's reserve keeps the image", async () => {
    // Alice uses X; bob's email reserves X while alice opts out. Whatever the
    // order inside the lock, X is kept because bob is already in refs:.
    const inner = createMemoryStateStore();
    const slow: PreloadStateStore = {
      ...inner,
      get: async (key) => {
        await sleep(5);
        return await inner.get(key);
      },
    };
    const { deletes, usage } = index({ store: slow });
    await usage.reserve([ALICE], [IMAGE_X]);
    await usage.charge([ALICE], [{ bytes: 10, urlHash: IMAGE_X }]);
    const purged = usage.purge(ALICE);
    await usage.reserve([BOB], [IMAGE_X]);
    await purged;
    expect(deletes).toStrictEqual([]);
    await usage.charge([BOB], [{ bytes: 10, urlHash: IMAGE_X }]);
    await usage.purge(BOB);
    expect(deletes).toHaveLength(1);
  });

  it("purge keeps in usage: the entries whose release failed", async () => {
    let failures = 1;
    const memory = createMemoryStorage();
    const images: ImageStorage = {
      ...memory,
      delete: async (key) => {
        if (failures > 0) {
          failures -= 1;
          throw new Error("storage down");
        }
        await memory.delete?.(key);
      },
    };
    const { store, usage } = index({ images });
    await usage.reserve([ALICE], [IMAGE_X]);
    await usage.charge([ALICE], [{ bytes: 10, urlHash: IMAGE_X }]);
    await expect(usage.purge(ALICE)).resolves.toBe(0);
    const left = await usageOf(store, ALICE);
    expect(left).toStrictEqual([IMAGE_X]);
    await expect(usage.purge(ALICE)).resolves.toBe(1);
    await expect(store.get(`usage:${ALICE}`)).resolves.toBeNull();
    await expect(store.get(`refs:${IMAGE_X}`)).resolves.toBeNull();
  });

  it("sweep drops expired usage: and refs:, keeps prefs: and live entries", async () => {
    const { clock, store, usage } = index();
    await usage.reserve([ALICE], [IMAGE_X]);
    await usage.charge([ALICE], [{ bytes: 10, urlHash: IMAGE_X }]);
    await usage.setOptOut([BOB], true, NOW);
    clock.t = NOW + TTL / 2;
    await usage.reserve([ALICE], [IMAGE_Y]);
    await usage.charge([ALICE], [{ bytes: 10, urlHash: IMAGE_Y }]);
    const present = async () => ({
      prefs: (await store.get(`prefs:${BOB}`)) !== null,
      refsX: (await store.get(`refs:${IMAGE_X}`)) !== null,
      refsY: (await store.get(`refs:${IMAGE_Y}`)) !== null,
      usage: await usageOf(store, ALICE),
    });
    clock.t = NOW + TTL + 1;
    await expect(usage.sweep()).resolves.toBe(1);
    await expect(present()).resolves.toStrictEqual({
      prefs: true,
      refsX: false,
      refsY: true,
      usage: [IMAGE_Y],
    });
    clock.t = NOW + 2 * TTL;
    await expect(usage.sweep()).resolves.toBe(2);
    await expect(present()).resolves.toStrictEqual({
      prefs: true,
      refsX: false,
      refsY: false,
      usage: [],
    });
  });

  it("a pinned record expires; an old one without until gets one", async () => {
    const { clock, store, usage } = index();
    await store.put(
      `refs:${IMAGE_X}`,
      JSON.stringify({ pinned: true, r: {}, v: 1 })
    );
    const swept: number[] = [await usage.sweep()];
    const pinned: unknown = JSON.parse(
      (await store.get(`refs:${IMAGE_X}`)) ?? "null"
    );
    expect(pinned).toStrictEqual({
      pinned: true,
      r: {},
      until: NOW + TTL,
      v: 1,
    });
    // Used again: the pin lasts until that use expires.
    clock.t = NOW + TTL / 2;
    await usage.reserve([ALICE], [IMAGE_X]);
    clock.t = NOW + TTL + 1;
    swept.push(await usage.sweep());
    clock.t = NOW + 2 * TTL;
    swept.push(await usage.sweep());
    expect(swept).toStrictEqual([0, 0, 1]);
    await expect(store.get(`refs:${IMAGE_X}`)).resolves.toBeNull();
  });

  it("the handler exposes the sweep only with preload", async () => {
    const { send, handler, advance } = await setup();
    await send(hook(["alice@example.org"], `<img src="${URL_A}">`));
    await expect(handler.sweepPreload?.()).resolves.toBe(0);
    advance(31 * 24 * 60 * 60 * 1000);
    await expect(handler.sweepPreload?.()).resolves.toBe(2);
    const plain = createProxyHandler({
      appOrigin: APP_ORIGIN,
      fetchImage: async () => await Promise.resolve({ error: "dns" }),
      keys: await keys(),
    });
    expect(plain.sweepPreload).toBeUndefined();
  });

  it("sweep leaves an unreadable record alone (it counts as full)", async () => {
    const { store, usage } = index();
    await store.put(`usage:${ALICE}`, "not json");
    await expect(usage.sweep()).resolves.toBe(0);
    await expect(store.get(`usage:${ALICE}`)).resolves.toBe("not json");
  });
});
