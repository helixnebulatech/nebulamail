// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// The Workers adapter against security/ssrf-vectors.json and the fetch
// vectors that apply to a runtime without pinning. Everything runs on a fake
// `fetch` that plays the DoH server and the targets, so the tests also prove
// that a blocked URL never reaches `fetch`.

import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";
import { z } from "zod";

import { IMAGE_ACCEPT, USER_AGENT } from "../fetch-types";
import { createFetchBudget, parseTargetUrl } from "../index";
import { SAMPLES } from "../test-fixtures";
import { createDohResolver, createWorkersFetch } from "./index";
import type { WorkersFetchOptions } from "./index";
import { DOH, fakeNet } from "./testing";
import type { FakeDns, Route } from "./testing";

const PUBLIC = "93.184.215.14";
const MB = 1024 * 1024;

const Vectors = z.object({
  url: z.array(
    z.object({
      dns: z
        .object({
          A: z.array(z.string()).optional(),
          AAAA: z.array(z.string()).optional(),
        })
        .optional(),
      expect: z.enum(["allow", "block"]),
      id: z.string(),
      input: z.string(),
      reason: z.string().optional(),
    })
  ),
});

const vectors = Vectors.parse(
  JSON.parse(
    readFileSync(
      new URL("../../../../security/ssrf-vectors.json", import.meta.url),
      "utf-8"
    )
  )
);

const png = (): Route => () =>
  new Response(SAMPLES.png, { headers: { "content-type": "image/png" } });

const redirect =
  (location: string, status = 302): Route =>
  () =>
    new Response(null, { headers: { location }, status });

const publicDns = (): FakeDns => ({ A: [PUBLIC] });

const setup = (
  routes: Readonly<Record<string, Route>>,
  dns: (name: string) => FakeDns | undefined = publicDns,
  extra: Partial<WorkersFetchOptions> = {}
) => {
  const net = fakeNet(dns, routes);
  const fetch = createWorkersFetch({
    fetch: net.fetch,
    resolver: createDohResolver({
      endpoint: DOH,
      fetch: net.fetch,
      maxCacheMs: 0,
    }),
    ...extra,
  });
  return { fetch, net };
};

/** A body stream that yields `chunks` chunks of `size` bytes, counting pulls. */
const counted = (chunks: number, size: number) => {
  let pulled = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull: (controller) => {
      if (pulled >= chunks) {
        controller.close();
        return;
      }
      pulled += 1;
      controller.enqueue(new Uint8Array(size));
    },
  });
  return { pulled: () => pulled, stream };
};

describe("url vectors through the Workers fetch", () => {
  it.each(vectors.url)("$id", async (vector) => {
    const parsed = parseTargetUrl(vector.input);
    const href = parsed.ok ? parsed.target.href : vector.input;
    const { dns } = vector;
    const { fetch, net } = setup({ [href]: png() }, () =>
      dns === undefined ? undefined : { A: dns.A ?? [], AAAA: dns.AAAA ?? [] }
    );
    const result = await fetch(vector.input, { expect: "image" });
    // A name without DNS in the vector is NXDOMAIN over DoH, so "dns" here;
    // the core guard test never resolves it at all.
    const nxdomain =
      parsed.ok && parsed.target.host.kind === "name" && dns === undefined;
    const expected =
      vector.expect === "allow"
        ? { imageType: "image/png", ok: true, requests: [href] }
        : {
            ok: false,
            reason: nxdomain ? "dns" : vector.reason,
            requests: [],
          };
    expect({
      ...(result.ok
        ? { imageType: result.imageType }
        : { reason: result.reason }),
      ok: result.ok,
      requests: net.requests.map((r) => r.url),
    }).toStrictEqual(expected);
  });
});

describe("workers fetch", () => {
  it("sends only generic headers and never follows redirects by itself", async () => {
    const url = "https://images.example.com/a.png";
    const { fetch, net } = setup({ [url]: png() });
    const result = await fetch(url, { expect: "image" });
    expect(result).toMatchObject({ imageType: "image/png", ok: true });
    const [request] = net.requests;
    expect(request?.init.redirect).toBe("manual");
    expect(request?.init.method).toBe("GET");
    expect(request?.init.headers).toStrictEqual({
      accept: IMAGE_ACCEPT,
      "accept-encoding": "identity",
      "user-agent": USER_AGENT,
    });
    expect(net.queries).toStrictEqual([
      "images.example.com/1",
      "images.example.com/28",
    ]);
  });

  it("drops cookies and encodings from the returned headers", async () => {
    const url = "https://images.example.com/a.png";
    const { fetch } = setup({
      [url]: () =>
        new Response(SAMPLES.png, {
          headers: {
            "content-type": "image/png",
            "set-cookie": "a=b",
            "x-other": "1",
          },
        }),
    });
    const result = await fetch(url, { expect: "image" });
    expect(result.ok && result.headers).toStrictEqual({
      "content-type": "image/png",
      "x-other": "1",
    });
  });

  it("returns non-2xx statuses without the body", async () => {
    const url = "https://images.example.com/missing.png";
    const { fetch } = setup({
      [url]: () => new Response("nope", { status: 404 }),
    });
    await expect(fetch(url, { expect: "image" })).resolves.toMatchObject({
      body: new Uint8Array(0),
      ok: true,
      status: 404,
    });
  });

  it("reports a failing connection as network", async () => {
    const { fetch } = setup({});
    await expect(
      fetch("https://down.example.com/a.png")
    ).resolves.toStrictEqual({
      ok: false,
      reason: "network",
    });
  });

  it("refuses POST before any lookup", async () => {
    const { fetch, net } = setup({});
    await expect(
      fetch("https://images.example.com/", {
        body: { data: "a=b", type: "application/x-www-form-urlencoded" },
        method: "POST",
      })
    ).rejects.toThrow(TypeError);
    expect(net.queries).toStrictEqual([]);
  });

  it("fails closed when DoH fails", async () => {
    const { fetch, net } = setup({}, () => ({ A: [PUBLIC], status: 2 }));
    await expect(
      fetch("https://images.example.com/a.png")
    ).resolves.toStrictEqual({
      ok: false,
      reason: "dns",
    });
    expect(net.requests).toStrictEqual([]);
  });
});

describe("vector: redirects (guard on every hop)", () => {
  const start = "https://images.example.com/start";

  it.each([
    ["redirect-to-private", "http://127.0.0.1/", "private-ip"],
    ["redirect-to-private-name", "http://rebind.example.com/", "private-ip"],
    ["redirect-to-metadata-name", "http://metadata.google.internal/", "dns"],
    ["redirect-scheme-change", "file:///etc/passwd", "scheme"],
    ["redirect-gopher", "gopher://images.example.com/", "scheme"],
    ["redirect-port", "http://images.example.com:8080/", "port"],
  ])("%s", async (_id, location, reason) => {
    const { fetch, net } = setup({ [start]: redirect(location) }, (name) => {
      if (name === "rebind.example.com") {
        return { A: ["10.0.0.1"] };
      }
      return name === "images.example.com" ? publicDns() : undefined;
    });
    await expect(fetch(start, { expect: "image" })).resolves.toStrictEqual({
      ok: false,
      reason,
    });
    expect(net.requests.map((r) => r.url)).toStrictEqual([start]);
  });

  it("follows up to 5 redirects, relative ones too", async () => {
    const routes: Record<string, Route> = {};
    for (let i = 0; i < 5; i += 1) {
      routes[`https://images.example.com/${i}`] = redirect(`/${i + 1}`, 307);
    }
    routes["https://images.example.com/5"] = png();
    const { fetch } = setup(routes);
    await expect(
      fetch("https://images.example.com/0", { expect: "image" })
    ).resolves.toMatchObject({ ok: true, redirects: 5 });
  });

  it("blocks the 6th redirect (redirect-loop)", async () => {
    const loop = "https://images.example.com/loop";
    const { fetch, net } = setup({ [loop]: redirect(loop, 301) });
    await expect(fetch(loop)).resolves.toStrictEqual({
      ok: false,
      reason: "too-many-redirects",
    });
    expect(net.requests).toHaveLength(6);
  });

  it("re-resolves every hop: a name that turns private later is blocked", async () => {
    // Rebinding between hops: first answer public, second private.
    let calls = 0;
    const { fetch, net } = setup(
      { [start]: redirect("https://images.example.com/again") },
      () => {
        calls += 1;
        // Two queries (A, AAAA) per resolution.
        return calls <= 2 ? publicDns() : { A: ["127.0.0.1"] };
      }
    );
    await expect(fetch(start)).resolves.toStrictEqual({
      ok: false,
      reason: "private-ip",
    });
    expect(net.requests).toHaveLength(1);
  });

  it("same-host and none modes return the 3xx without following", async () => {
    const { fetch, net } = setup({
      [start]: redirect("https://other.example.com/"),
    });
    await expect(
      fetch(start, { redirects: "same-host" })
    ).resolves.toMatchObject({
      ok: true,
      status: 302,
    });
    await expect(fetch(start, { redirects: "none" })).resolves.toMatchObject({
      ok: true,
      status: 302,
    });
    expect(net.requests).toHaveLength(2);
  });
});

describe("caller rules: allowRedirect and budget", () => {
  const start = "https://images.example.com/start";

  it("refuses a redirect target before fetching it", async () => {
    const { fetch, net } = setup({
      [start]: redirect("https://pixel.example.net/open.gif"),
    });
    await expect(
      fetch(start, {
        allowRedirect: (url) => !url.startsWith("https://pixel."),
        expect: "image",
      })
    ).resolves.toStrictEqual({ ok: false, reason: "refused" });
    expect(net.requests.map((r) => r.url)).toStrictEqual([start]);
  });

  it("every hop takes a request; the body is capped by the bytes left", async () => {
    const { fetch, net } = setup({
      [start]: redirect("/a.png"),
      "https://images.example.com/a.png": png(),
    });
    const tight = createFetchBudget({ maxBytes: MB, maxRequests: 1 });
    await expect(fetch(start, { budget: tight })).resolves.toStrictEqual({
      ok: false,
      reason: "budget",
    });
    expect(net.requests).toHaveLength(1);
    const small = createFetchBudget({ maxBytes: 10, maxRequests: 5 });
    await expect(fetch(start, { budget: small })).resolves.toStrictEqual({
      ok: false,
      reason: "size",
    });
    expect(small.exhausted()).toBeTruthy();
  });

  it("takes the bytes read even when the body is then refused", async () => {
    const url = "https://images.example.com/fake.png";
    const { fetch } = setup({
      [url]: () =>
        new Response("x".repeat(1000), {
          headers: { "content-type": "image/png" },
        }),
    });
    const budget = createFetchBudget({ maxBytes: MB, maxRequests: 5 });
    await expect(
      fetch(url, { budget, expect: "image" })
    ).resolves.toStrictEqual({ ok: false, reason: "content-type" });
    expect(budget.remainingBytes()).toBe(MB - 1000);
  });
});

describe("vector: not-an-image", () => {
  const url = "https://images.example.com/x";

  it.each([
    ["html", "text/html", SAMPLES.png],
    ["svg", "image/svg+xml", new TextEncoder().encode("<svg/>")],
    ["mismatch", "image/png", SAMPLES.gif],
  ])("%s", async (_name, type, body) => {
    const { fetch } = setup({
      [url]: () => new Response(body, { headers: { "content-type": type } }),
    });
    await expect(fetch(url, { expect: "image" })).resolves.toStrictEqual({
      ok: false,
      reason: "content-type",
    });
  });
});

describe("vector: too-large", () => {
  const url = "https://images.example.com/big.png";

  it("blocks a declared Content-Length over 5 MB without reading it", async () => {
    const body = counted(10, MB);
    const { fetch } = setup({
      [url]: () =>
        new Response(body.stream, {
          headers: {
            "content-length": String(10 * MB),
            "content-type": "image/png",
          },
        }),
    });
    await expect(fetch(url, { expect: "image" })).resolves.toStrictEqual({
      ok: false,
      reason: "size",
    });
    // A ReadableStream may pull one chunk ahead; never the whole body.
    expect(body.pulled()).toBeLessThanOrEqual(1);
  });

  it("stops a streamed body (or an already decompressed bomb) at 5 MB", async () => {
    // Workers decode gzip/br before we read, so a bomb arrives as this: a
    // stream of decoded bytes much bigger than the wire size.
    const body = counted(1000, MB);
    const { fetch } = setup({
      [url]: () =>
        new Response(body.stream, { headers: { "content-type": "image/png" } }),
    });
    await expect(fetch(url, { expect: "image" })).resolves.toStrictEqual({
      ok: false,
      reason: "size",
    });
    expect(body.pulled()).toBeLessThanOrEqual(7);
  });

  it("cannot be loosened through options", async () => {
    const body = counted(7, MB);
    const { fetch } = setup(
      {
        [url]: () =>
          new Response(body.stream, {
            headers: { "content-type": "image/png" },
          }),
      },
      publicDns,
      { limits: { maxBodyBytes: 100 * MB } }
    );
    await expect(fetch(url)).resolves.toStrictEqual({
      ok: false,
      reason: "size",
    });
  });
});

describe("vector: slow-loris and deadlines", () => {
  const url = "https://images.example.com/slow.png";

  it("times out a body that never ends", async () => {
    const { fetch } = setup(
      {
        [url]: (init) =>
          new Response(
            new ReadableStream({
              start: (controller) => {
                init.signal?.addEventListener("abort", () => {
                  controller.error(new DOMException("aborted", "AbortError"));
                });
              },
            }),
            { headers: { "content-type": "image/png" } }
          ),
      },
      publicDns,
      { limits: { totalTimeoutMs: 50 } }
    );
    await expect(fetch(url, { expect: "image" })).resolves.toStrictEqual({
      ok: false,
      reason: "timeout",
    });
  });

  it("times out a resolver that never answers", async () => {
    const fetch = createWorkersFetch({
      fetch: fakeNet(publicDns).fetch,
      limits: { totalTimeoutMs: 50 },
      resolver: async () => await Promise.withResolvers<never>().promise,
    });
    await expect(fetch(url)).resolves.toStrictEqual({
      ok: false,
      reason: "timeout",
    });
  });

  it("reports a caller abort as timeout", async () => {
    const { fetch } = setup({ [url]: png() });
    await expect(
      fetch(url, { signal: AbortSignal.abort() })
    ).resolves.toStrictEqual({ ok: false, reason: "timeout" });
  });
});
