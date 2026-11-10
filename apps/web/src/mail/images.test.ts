// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { BlobId } from "@nebulamail/jmap-client";
import { normalizeCid } from "@nebulamail/mail-render";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { InlinePart } from "./api";
import { forgetProxyModes, inlineToLoad, loadRemoteImages } from "./images";

const MB = 1024 * 1024;

const part = (cid: string, size: number, type = "image/png"): InlinePart => ({
  blobId: BlobId.parse(`b-${cid.replaceAll(/\W/gu, "")}`),
  cid,
  name: `${cid}.png`,
  size,
  type,
});

describe(inlineToLoad, () => {
  it("skips what doesn't fit the budget and keeps going", () => {
    const parts = [
      part("a", 4 * MB),
      part("b", 4 * MB),
      part("c", 4 * MB),
      part("d", 1 * MB),
    ];
    expect(
      inlineToLoad(parts, ["a", "b", "c", "d"]).map((item) => item.cid)
    ).toStrictEqual(["a", "b", "d"]);
  });

  it("only raster images the HTML needs, each under its limit", () => {
    const parts = [
      part("<A>", 1),
      part("svg", 1, "image/svg+xml"),
      part("big", 6 * MB),
      part("unused", 1),
    ];
    expect(
      inlineToLoad(parts, ["a", "svg", "big"]).map((item) => item.cid)
    ).toStrictEqual(["<A>"]);
  });

  // RFC 2392 §2: only the cid: URL is percent-decoded, never the
  // Content-ID header itself.
  it("matches a Content-ID with % to its percent-encoded cid: URL", () => {
    const parts = [part("<a%41b@x>", 1), part("aAb@x", 1)];
    // What mail-render reports for <img src="cid:a%2541b@x">.
    const needed = [normalizeCid("a%2541b@x")];

    expect(inlineToLoad(parts, needed).map((item) => item.cid)).toStrictEqual([
      "<a%41b@x>",
    ]);
  });
});

const PIXEL =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/** The BFF's tickets, then the proxy's cache-mode answer. */
const answers = (images: Record<string, unknown>) => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      await Promise.resolve();
      return input === "/api/images/tickets"
        ? Response.json({
            images: ["a", "b", "c", "d"].map((name) => ({
              src: `https://img.example/i/${name}`,
              ticket: name,
              url: `https://cdn.example/${name}.png`,
            })),
            proxyOrigin: "https://img.example",
          })
        : Response.json({ images });
    })
  );
};

describe(loadRemoteImages, () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    forgetProxyModes();
  });

  it("cache mode: stored ones by URL, ones the proxy may not keep as data:", async () => {
    answers({
      a: { cached: true },
      // RFC 9111: the sender said no-store; the proxy answers it inline.
      b: { data: PIXEL, type: "image/png" },
      // Never anything but a raster type and plain base64.
      c: { data: PIXEL, type: "text/html" },
      d: { data: "<svg>", type: "image/png" },
    });
    const images = await loadRemoteImages("e1");
    expect(images).toStrictEqual({
      kind: "proxy",
      origin: "https://img.example",
      urls: new Map([
        ["https://cdn.example/a.png", "https://img.example/i/a"],
        ["https://cdn.example/b.png", `data:image/png;base64,${PIXEL}`],
      ]),
    });
  });

  it("asks a proxy without storage for cache only once", async () => {
    const modes: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string, init?: RequestInit) => {
        await Promise.resolve();
        if (input === "/api/images/tickets") {
          return Response.json({
            images: [
              {
                src: "https://img.example/i/a",
                ticket: "a",
                url: "https://cdn.example/a.png",
              },
            ],
            proxyOrigin: "https://img.example",
          });
        }
        const body: unknown =
          typeof init?.body === "string" ? JSON.parse(init.body) : null;
        const mode =
          typeof body === "object" && body !== null && "mode" in body
            ? String(body.mode)
            : "";
        modes.push(mode);
        return mode === "cache"
          ? new Response("no storage", { status: 400 })
          : Response.json({
              images: { a: { data: PIXEL, type: "image/png" } },
            });
      })
    );
    const created = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");

    await expect(loadRemoteImages("e1")).resolves.toMatchObject({
      kind: "batch",
    });
    await expect(loadRemoteImages("e2")).resolves.toMatchObject({
      kind: "batch",
    });
    expect(modes).toStrictEqual(["cache", "batch", "batch"]);
    created.mockRestore();
  });
});
