// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { readFile } from "node:fs/promises";

import { inProcessExtractor } from "@nebulamail/mail-render/extract";
import { createWorkerExtractor } from "@nebulamail/mail-render/server-extract";
import { JSDOM } from "jsdom";
import { describe, expect, it, vi } from "vitest";
import { z } from "zod";

import type {
  FetchImage,
  ProxyConfig,
  ProxyLogEvent,
  RequestContext,
} from "../index";
import {
  createMemoryStateStore,
  createMemoryStorage,
  createProxyHandler,
  createRateLimiter,
  preloadKey,
} from "../index";
import { APP_ORIGIN, gif, keys, NOW, png, ticket } from "../test-fixtures";
import { hashUrl } from "../tickets/ticket";
import { createBlobStorage } from "../vercel/blob-storage";
import { createFakeBlob } from "../vercel/test-blob";
import { MAX_TOKEN_HTML_CHARS } from "./hook";
import {
  decodeBase64,
  decodeQuotedPrintable,
  htmlBodies,
  parseContentType,
  splitHeaders,
} from "./mime";

const BASE = "https://img.example.com";
const SECRET = "s".repeat(40);
const URL_A = "https://images.example.com/a.png";
const URL_B = "https://images.example.com/b.gif";
const TRACKER = "https://www.google-analytics.com/collect?v=1";

const extractor = inProcessExtractor(new new JSDOM("").window.DOMParser());

const Json = z.record(z.string(), z.unknown());

/** An email as Stalwart's MTA Hook sends it (top-level headers + the rest). */
const hookBody = (
  headers: [string, string][],
  contents: string,
  stage = "DATA"
): string =>
  JSON.stringify({
    context: { stage },
    envelope: {
      from: { address: "news@example.com" },
      to: [{ address: "alice@example.org" }],
    },
    message: { contents, headers, serverHeaders: [], size: contents.length },
  });

const htmlEmail = (html: string): string =>
  hookBody([["Content-Type", ' text/html; charset="utf-8"']], html);

const hook = (body: string, token: string | null = SECRET): Request =>
  new Request(`${BASE}/hooks/stalwart`, {
    body,
    headers: {
      "content-type": "application/json",
      ...(token === null ? {} : { authorization: `Bearer ${token}` }),
    },
    method: "POST",
  });

const setup = async (overrides: Partial<ProxyConfig> = {}) => {
  const storage = createMemoryStorage({ now: () => NOW });
  const state = createMemoryStateStore();
  const calls: string[] = [];
  const fetchImage: FetchImage = async (url) => {
    calls.push(url);
    await Promise.resolve();
    if (url === URL_A) {
      return { bytes: png(), type: "image/png" };
    }
    if (url === URL_B) {
      return { bytes: gif(), type: "image/gif" };
    }
    return { error: "dns" };
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
    now: () => NOW,
    preload: { fullEmail: true, extractor, secret: SECRET, state },
    storage,
    ...overrides,
  });
  const jobs: Promise<unknown>[] = [];
  const context: RequestContext = {
    waitUntil: (job) => {
      jobs.push(job);
    },
  };
  /** Send a hook request and wait for its background work. */
  const send = async (request: Request) => {
    const response = await handler(request, context);
    await Promise.all(jobs);
    return response;
  };
  return { calls, context, events, handler, jobs, k, send, state, storage };
};

/** Quoted-printable decoded as UTF-8 text, or null. */
const qp = (text: string, max?: number) => {
  const bytes = decodeQuotedPrintable(text, max);
  return bytes === null ? null : new TextDecoder().decode(bytes);
};

describe("MIME", () => {
  it("parses content types and folded header blocks", () => {
    expect(
      parseContentType('Multipart/Alternative; boundary="a b"; x=1')
    ).toStrictEqual({
      params: new Map([
        ["boundary", "a b"],
        ["x", "1"],
      ]),
      type: "multipart/alternative",
    });
    expect(parseContentType(undefined).type).toBe("text/plain");
    const { headers, body } = splitHeaders(
      "Content-Type: text/html;\r\n charset=utf-8\r\nX: y\r\n\r\n<p>hi</p>"
    );
    expect(headers).toStrictEqual([
      ["Content-Type", "text/html; charset=utf-8"],
      ["X", "y"],
    ]);
    expect(body).toBe("<p>hi</p>");
  });

  it("decodes quoted-printable and base64", () => {
    expect(
      new TextDecoder().decode(
        decodeQuotedPrintable('<img src=3D"x">caf=C3=A9 =\r\nend') ??
          new Uint8Array()
      )
    ).toBe('<img src="x">café end');
    expect(
      new TextDecoder().decode(decodeBase64("aGk=\r\n") ?? new Uint8Array())
    ).toBe("hi");
    expect(decodeBase64("not base64!")).toBeNull();
  });

  it("finds every inline HTML body in nested multiparts, never attachments", () => {
    const html = '<p>café</p><img src="https://images.example.com/a.png">';
    const b64 = btoa(String.fromCodePoint(...new TextEncoder().encode(html)));
    const latin1 = "=E9t=E9";
    const body = [
      "preamble",
      "--outer",
      'Content-Type: multipart/alternative; boundary="inner"',
      "",
      "--inner",
      "Content-Type: text/plain",
      "",
      "plain",
      "--inner",
      "Content-Type: text/html; charset=utf-8",
      "Content-Transfer-Encoding: base64",
      "",
      b64,
      "--inner--",
      "--outer",
      "Content-Type: text/html; charset=iso-8859-1",
      "Content-Transfer-Encoding: quoted-printable",
      "",
      latin1,
      "--outer",
      "Content-Type: text/html",
      "Content-Disposition: attachment; filename=a.html",
      "",
      '<img src="https://evil.example/attached.png">',
      "--outer--",
      "epilogue",
    ].join("\r\n");
    expect(
      htmlBodies([["Content-Type", "multipart/mixed; boundary=outer"]], body)
    ).toStrictEqual([html, "été"]);
  });

  it("keeps the quoted-printable rules: soft breaks, = without hex, bounds", () => {
    expect([
      qp("a=\nb=  \t\r\nc=3d=3D"),
      qp("50=% =G1 end="),
      qp("é=E2=82=AC"),
    ]).toStrictEqual(["abc==", "50=% =G1 end=", "é€"]);
    expect([qp("=41=42", 2), qp("=41=42", 1), qp("abcd", 1)]).toStrictEqual([
      "AB",
      null,
      null,
    ]);
  });

  it("skips a part too big for maxHtmlChars without decoding it all", () => {
    const limits = {
      maxDepth: 8,
      maxHtmlChars: 5 * 1024 * 1024,
      maxParts: 100,
    };
    const part = (encoding: string, body: string) =>
      htmlBodies(
        [
          ["Content-Type", "text/html; charset=utf-8"],
          ["Content-Transfer-Encoding", encoding],
        ],
        body,
        limits
      );
    const started = performance.now();
    expect(
      part("quoted-printable", "a".repeat(25 * 1024 * 1024))
    ).toStrictEqual([]);
    expect(part("7bit", "a".repeat(6 * 1024 * 1024))).toStrictEqual([]);
    expect(performance.now() - started).toBeLessThan(2000);
    const atob = vi.spyOn(globalThis, "atob");
    try {
      expect(part("base64", "QUFB".repeat(8 * 1024 * 1024))).toStrictEqual([]);
      expect(atob).not.toHaveBeenCalled();
    } finally {
      atob.mockRestore();
    }
  });

  it("a part that fits is still read, also after another one", () => {
    const limits = { maxDepth: 8, maxHtmlChars: 10, maxParts: 100 };
    const body = [
      "--b",
      "Content-Type: text/html",
      "Content-Transfer-Encoding: base64",
      "",
      btoa("<p>1234</p>"),
      "--b",
      "Content-Type: text/html",
      "Content-Transfer-Encoding: quoted-printable",
      "",
      "<b>=33</b>",
      "--b--",
    ].join("\r\n");
    expect(
      htmlBodies(
        [["Content-Type", "multipart/mixed; boundary=b"]],
        body,
        limits
      )
    ).toStrictEqual(["<b>3</b>"]);
  });

  it("is bounded: depth and parts", () => {
    let body = "<p>deep</p>";
    for (let i = 0; i < 12; i += 1) {
      body = `--b${String(i)}\r\nContent-Type: ${
        i === 0 ? "text/html" : `multipart/mixed; boundary=b${String(i - 1)}`
      }\r\n\r\n${body}\r\n--b${String(i)}--`;
    }
    expect(
      htmlBodies([["Content-Type", "multipart/mixed; boundary=b11"]], body)
    ).toStrictEqual([]);
  });
});

describe("POST /hooks/stalwart (preload)", () => {
  it("refuses a missing or wrong token, and is 404 without preload", async () => {
    const { send, calls } = await setup();
    const body = htmlEmail(`<img src="${URL_A}">`);
    await expect(send(hook(body, null))).resolves.toMatchObject({
      status: 401,
    });
    await expect(send(hook(body, `${SECRET}x`))).resolves.toMatchObject({
      status: 401,
    });
    expect(calls).toStrictEqual([]);
  });

  it("is 404 when preload is not configured", async () => {
    const body = htmlEmail(`<img src="${URL_A}">`);
    const plain = createProxyHandler({
      appOrigin: APP_ORIGIN,
      fetchImage: async () => await Promise.resolve({ error: "dns" }),
      keys: await keys(),
      storage: createMemoryStorage(),
    });
    await expect(plain(hook(body))).resolves.toMatchObject({ status: 404 });
  });

  it("accepts at once and stores the email's images under the preload key", async () => {
    const { send, calls, storage, events } = await setup();
    const response = await send(
      hook(
        htmlEmail(
          `<img src="${URL_A}"><div style="background:url(${URL_B})"></div><img src="${TRACKER}">`
        )
      )
    );
    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toStrictEqual({ action: "accept" });
    expect(calls.toSorted()).toStrictEqual([URL_A, URL_B]);
    await expect(storage.get(await preloadKey(URL_A))).resolves.toMatchObject({
      type: "image/png",
    });
    expect(events).toContainEqual({
      bytes: 64,
      event: "preload",
      failed: 0,
      fetched: 2,
      fromCache: 0,
      images: 2,
      optedOut: 0,
      overQuota: 0,
      recipients: 1,
    });
  });

  it("RFC 9111: never preloads private, no-store or no-cache images (shared cache)", async () => {
    const answers: Record<string, string> = {
      [URL_A]: "private, max-age=600",
      [URL_B]: "no-store",
    };
    const { send, storage, events } = await setup({
      fetchImage: async (url) =>
        await Promise.resolve({
          bytes: png(),
          cacheHeaders: { "cache-control": answers[url] ?? "" },
          type: "image/png",
        }),
    });
    await send(hook(htmlEmail(`<img src="${URL_A}"><img src="${URL_B}">`)));
    await expect(storage.get(await preloadKey(URL_A))).resolves.toBeNull();
    await expect(storage.get(await preloadKey(URL_B))).resolves.toBeNull();
    expect(events).toContainEqual(
      expect.objectContaining({ event: "preload", failed: 2, fetched: 0 })
    );
  });

  it("parses in a worker: a quadratic email is cut at the deadline, accepted, nothing fetched", async () => {
    const vectors = z
      .object({
        suffix: z.string(),
        vectors: z.array(
          z.object({ id: z.string(), repeat: z.number(), unit: z.string() })
        ),
      })
      .parse(
        JSON.parse(
          await readFile(
            new URL(
              "../../../../security/parser-vectors.json",
              import.meta.url
            ),
            "utf-8"
          )
        )
      );
    const slow = vectors.vectors.find((v) => v.id === "table-a-closed");
    const hostile = `${(slow?.unit ?? "").repeat(slow?.repeat ?? 0)}<img src="${URL_A}">`;
    const worker = createWorkerExtractor({ timeoutMs: 100 });
    try {
      const { send, calls, events } = await setup({
        preload: {
          // Limits wide enough for the prefilter (html-estimate) to let the
          // vector through (it refuses it with the default ones, which is
          // also safe): only the worker's deadline stops it here.
          extractor: {
            extract: async (html, options) =>
              await worker.extract(html, {
                ...options,
                limits: {
                  maxElements: 100_000_000,
                  maxNestingDepth: 1_000_000,
                },
              }),
          },
          fullEmail: true,
          secret: SECRET,
          state: createMemoryStateStore(),
        },
      });
      // A normal email through the same worker is preloaded.
      await send(hook(htmlEmail(`<img src="${URL_B}">`)));
      expect(calls).toStrictEqual([URL_B]);
      const started = performance.now();
      const response = await send(hook(htmlEmail(hostile)));
      expect(performance.now() - started).toBeLessThan(2000);
      await expect(response.json()).resolves.toStrictEqual({
        action: "accept",
      });
      expect(calls).toStrictEqual([URL_B]);
      expect(events).toContainEqual({
        event: "preload-skipped",
        reason: "unparsable",
      });
    } finally {
      await worker.close();
    }
  });

  it("logs counts only, never URLs", async () => {
    const { send, events } = await setup();
    await send(hook(htmlEmail(`<img src="${URL_A}">`)));
    expect(JSON.stringify(events)).not.toContain("images.example.com");
  });

  it("then any account's ticket is served without downloading", async () => {
    const { send, calls, handler, k } = await setup();
    await send(hook(htmlEmail(`<img src="${URL_A}">`)));
    calls.length = 0;
    const t = await ticket(k, { accountId: "bob", url: URL_A });
    const get = await handler(new Request(`${BASE}/i/${t}`));
    expect(get.status).toBe(200);
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
    expect(calls).toStrictEqual([]);
  });

  it("does not download again what is already preloaded", async () => {
    const { send, calls } = await setup();
    await send(hook(htmlEmail(`<img src="${URL_A}">`)));
    await send(hook(htmlEmail(`<img src="${URL_A}"><img src="${URL_B}">`)));
    expect(calls).toStrictEqual([URL_A, URL_B]);
  });

  it("refuses trackers on redirect hops too", async () => {
    const seen: string[] = [];
    const { send } = await setup({
      fetchImage: async (url, _signal, options) => {
        seen.push(url);
        // A redirect to a tracker: the guarded fetch asks allowRedirect.
        const allowed = options?.allowRedirect(TRACKER) ?? true;
        return await Promise.resolve(
          allowed ? { bytes: png(), type: "image/png" } : { error: "refused" }
        );
      },
    });
    await send(hook(htmlEmail(`<img src="${URL_A}">`)));
    expect(seen).toStrictEqual([URL_A]);
  });

  it.each([
    [
      "rate limited",
      { rateLimit: () => false },
      htmlEmail(`<img src="${URL_A}">`),
    ],
    ["not the DATA stage", {}, hookBody([], `<img src="${URL_A}">`, "RCPT")],
    ["invalid JSON", {}, "{"],
    ["not a hook request", {}, JSON.stringify({ message: 1 })],
    [
      "too large",
      {
        preload: {
          fullEmail: true,
          maxMessageBytes: 100,
          extractor,
          secret: SECRET,
          state: createMemoryStateStore(),
        },
      },
      htmlEmail(`<img src="${URL_A}">${"x".repeat(200)}`),
    ],
  ] satisfies [string, Partial<ProxyConfig>, string][])(
    "%s: accepts, downloads nothing",
    async (_name, overrides, body) => {
      const { send, calls } = await setup(overrides);
      const response = await send(hook(body));
      expect(response.status).toBe(200);
      expect(Json.parse(await response.json())).toStrictEqual({
        action: "accept",
      });
      expect(calls).toStrictEqual([]);
    }
  );

  it("skips whole emails as busy past maxQueuedChars (memory held in the queue)", async () => {
    const html = `<img src="${URL_A}">${"x".repeat(10_000)}`;
    const size = htmlEmail(html).length;
    const { handler, context, jobs, events } = await setup({
      preload: {
        concurrency: 1,
        fullEmail: true,
        maxQueuedChars: Math.floor(size * 2.5),
        extractor,
        secret: SECRET,
        state: createMemoryStateStore(),
      },
    });
    const answers = await Promise.all(
      [1, 2, 3].map(async () => await handler(hook(htmlEmail(html)), context))
    );
    await Promise.all(jobs);
    expect(answers.map((answer) => answer.status)).toStrictEqual([
      200, 200, 200,
    ]);
    expect(
      events.filter(
        (event) =>
          event.event === "preload-skipped" &&
          "reason" in event &&
          event.reason === "busy"
      )
    ).toHaveLength(1);
    // Released when the jobs end: the next email is preloaded again.
    await handler(hook(htmlEmail(html)), context);
    await Promise.all(jobs);
    expect(
      events.filter((event) => event.event === "preload-skipped")
    ).toHaveLength(1);
  });

  it("parses a whole message when Stalwart sends no header list", async () => {
    const { send, calls } = await setup();
    await send(
      hook(
        hookBody(
          [],
          `Content-Type: text/html\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<img src=3D"${URL_A}">`
        )
      )
    );
    expect(calls).toStrictEqual([URL_A]);
  });

  it("healthz lists preload; preload needs storage and a long secret", async () => {
    const { handler } = await setup();
    const health = await handler(new Request(`${BASE}/healthz`));
    await expect(health.json()).resolves.toMatchObject({
      modes: ["batch", "cache", "preload"],
    });
    const base = {
      appOrigin: APP_ORIGIN,
      fetchImage: async () => await Promise.resolve({ error: "dns" as const }),
      keys: await keys(),
    };
    expect(() =>
      createProxyHandler({
        ...base,
        preload: { extractor, secret: SECRET, state: createMemoryStateStore() },
      })
    ).toThrow(/storage/u);
    expect(() =>
      createProxyHandler({
        ...base,
        preload: {
          extractor,
          secret: "short",
          state: createMemoryStateStore(),
        },
        storage: createMemoryStorage(),
      })
    ).toThrow(/32/u);
  });
});

const setupTokens = async (
  preload: Partial<NonNullable<ProxyConfig["preload"]>> = {},
  config: Partial<ProxyConfig> = {}
) => {
  const base = await setup();
  const handler = createProxyHandler({
    appOrigin: APP_ORIGIN,
    fetchImage: async (url) => {
      await Promise.resolve();
      return url === URL_A
        ? { bytes: png(), type: "image/png" }
        : { error: "dns" };
    },
    keys: base.k,
    now: () => NOW,
    preload: {
      emailTokens: true,
      fullEmail: true,
      extractor,
      secret: SECRET,
      state: base.state,
      ...preload,
    },
    storage: base.storage,
    ...config,
  });
  const send = async (request: Request) => {
    const response = await handler(request, base.context);
    await Promise.all(base.jobs);
    return response;
  };
  return { handler, send };
};

const get = (path: string): Request => new Request(`${BASE}${path}`);

describe("email tokens (static build)", () => {
  const Modifications = z.object({
    action: z.literal("accept"),
    modifications: z.array(z.record(z.string(), z.unknown())).optional(),
  });

  const tokenOf = async (response: Response): Promise<string> => {
    const body = Modifications.parse(await response.json());
    const added = body.modifications?.find((mod) => mod.type === "addHeader");
    expect(added?.name).toBe("X-Nebula-Preload");
    return z.string().parse(added?.value);
  };

  it("adds a token and serves only that email's preloaded images", async () => {
    const { handler, send } = await setupTokens();
    const token = await tokenOf(
      await send(hook(htmlEmail(`<img src="${URL_A}"><img src="${TRACKER}">`)))
    );
    const hashA = await hashUrl(URL_A);
    const image = await handler(get(`/p/${token}/${hashA}`));
    expect(image.status).toBe(200);
    expect(image.headers.get("content-type")).toBe("image/png");
    expect(image.headers.get("cross-origin-resource-policy")).toBe(
      "cross-origin"
    );
    // Not an image of this email (the tracker was never preloaded either).
    const other = await handler(get(`/p/${token}/${await hashUrl(TRACKER)}`));
    expect(other.status).toBe(404);
  });

  it("serves /p/ from a storage that is not memory (Vercel Blob)", async () => {
    const { handler, send } = await setupTokens(
      {},
      { storage: createBlobStorage(createFakeBlob()) }
    );
    const token = await tokenOf(
      await send(hook(htmlEmail(`<img src="${URL_A}">`)))
    );
    const image = await handler(get(`/p/${token}/${await hashUrl(URL_A)}`));
    expect(image.status).toBe(200);
  });

  it("image:global is spent only by valid tokens", async () => {
    const { handler, send } = await setupTokens(
      {},
      {
        rateLimit: createRateLimiter({
          rules: {
            "image:global": { capacity: 2, refillPerSecond: 0 },
            "image:ip": { capacity: 100, refillPerSecond: 0 },
          },
        }),
      }
    );
    const token = await tokenOf(
      await send(hook(htmlEmail(`<img src="${URL_A}">`)))
    );
    const hashA = await hashUrl(URL_A);
    const forged = `${token.slice(0, -2)}${token.endsWith("AA") ? "BB" : "AA"}`;
    for (let i = 0; i < 10; i += 1) {
      // oxlint-disable-next-line no-await-in-loop
      const response = await handler(get(`/p/${forged}/${hashA}`), {
        clientAddress: `203.0.113.${i}`,
      });
      expect(response.status).toBe(403);
    }
    const image = await handler(get(`/p/${token}/${hashA}`));
    expect(image.status).toBe(200);
  });

  it("refuses a forged token", async () => {
    const { handler, send } = await setupTokens();
    const token = await tokenOf(
      await send(hook(htmlEmail(`<img src="${URL_A}">`)))
    );
    const forged = `${token.slice(0, -2)}${token.endsWith("AA") ? "BB" : "AA"}`;
    const response = await handler(get(`/p/${forged}/${await hashUrl(URL_A)}`));
    expect(response.status).toBe(403);
  });

  it("signs the time it was made, so a changed time fails", async () => {
    const { handler, send } = await setupTokens();
    const token = await tokenOf(
      await send(hook(htmlEmail(`<img src="${URL_A}">`)))
    );
    const issuedAt = token.split(".").at(2);
    expect(issuedAt).toBe(String(Math.floor(NOW / 1000)));
    // Made to look like the time of another delivery: the MAC breaks.
    const moved = token.replace(
      `.${String(issuedAt)}.`,
      `.${String(Number(issuedAt) + 1)}.`
    );
    const response = await handler(get(`/p/${moved}/${await hashUrl(URL_A)}`));
    expect(response.status).toBe(403);
  });

  it("removes token headers the email already had", async () => {
    const { send } = await setupTokens();
    const response = await send(
      hook(
        hookBody(
          [
            ["X-Nebula-Preload", " 1.1.forged"],
            ["Content-Type", " text/html"],
            ["x-nebula-preload", " 1.1.again"],
          ],
          `<img src="${URL_A}">`
        )
      )
    );
    const body = Modifications.parse(await response.json());
    expect(body.modifications?.slice(0, 2)).toStrictEqual([
      { index: 2, name: "X-Nebula-Preload", type: "deleteHeader" },
      { index: 1, name: "X-Nebula-Preload", type: "deleteHeader" },
    ]);
    expect(body.modifications?.[2]?.type).toBe("addHeader");
  });

  it("adds nothing to an email without images", async () => {
    const { send } = await setupTokens();
    const response = await send(hook(htmlEmail("<p>no images</p>")));
    await expect(response.json()).resolves.toStrictEqual({ action: "accept" });
  });

  it("gives no token to an email whose HTML is too big to read in time", async () => {
    const { send } = await setupTokens();
    const big = `<img src="${URL_A}">${"x".repeat(MAX_TOKEN_HTML_CHARS)}`;
    const response = await send(
      hook(
        hookBody(
          [
            ["X-Nebula-Preload", " 1.1.forged"],
            ["Content-Type", " text/html"],
          ],
          big
        )
      )
    );
    const body = Modifications.parse(await response.json());
    // The forged one still goes; no token is added.
    expect(body.modifications).toStrictEqual([
      { index: 1, name: "X-Nebula-Preload", type: "deleteHeader" },
    ]);
  });

  it("gives no token when no recipient takes preloaded images", async () => {
    const { send } = await setupTokens({ maxImagesPerRecipient: 1 });
    await tokenOf(await send(hook(htmlEmail(`<img src="${URL_A}">`))));
    // The recipient's quota is full now.
    const response = await send(hook(htmlEmail(`<img src="${URL_B}">`)));
    await expect(response.json()).resolves.toStrictEqual({ action: "accept" });
  });

  it("is off by default", async () => {
    const { send } = await setup();
    const response = await send(hook(htmlEmail(`<img src="${URL_A}">`)));
    await expect(response.json()).resolves.toStrictEqual({ action: "accept" });
  });
});
