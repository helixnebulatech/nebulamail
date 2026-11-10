// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment node

// Server-side extraction in a worker thread: same URLs as
// jsdom, and hostile HTML fails closed within the deadline.

import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";

import { JSDOM } from "jsdom";
import { afterAll, describe, expect, it } from "vitest";
import { z } from "zod";

import { extractRemoteImageUrls } from "../src/extract";
import { parseLight } from "../src/parse-worker";
import { createWorkerExtractor } from "../src/server-extract";

const SECURITY = path.join(import.meta.dirname, "../../../security");
const CORPUS = path.join(SECURITY, "hostile-emails");

const Vectors = z.object({
  suffix: z.string(),
  vectors: z.array(
    z.object({ id: z.string(), repeat: z.number().int(), unit: z.string() })
  ),
});
const vectors = Vectors.parse(
  JSON.parse(readFileSync(path.join(SECURITY, "parser-vectors.json"), "utf-8"))
);

/** One parse in this thread, as the worker does it. */
const parseHere = (html: string, maxDepth: number) =>
  parseLight({
    attributes: [],
    html,
    id: 1,
    maxDepth,
    textOf: [],
  });

const parser = new new JSDOM("").window.DOMParser();
const extractor = createWorkerExtractor({ timeoutMs: 100 });

describe(createWorkerExtractor, () => {
  afterAll(async () => {
    await extractor.close();
  });

  it("gives the same URLs and trackers as jsdom on the hostile corpus", async () => {
    const files = readdirSync(CORPUS).filter((file) => file.endsWith(".html"));
    expect(files.length).toBeGreaterThanOrEqual(15);
    const roomy = createWorkerExtractor();
    try {
      const pairs = await Promise.all(
        files.map(async (file) => {
          const html = readFileSync(path.join(CORPUS, file), "utf-8");
          return {
            file,
            jsdom: extractRemoteImageUrls(html, { parser }),
            worker: await roomy.extract(html),
          };
        })
      );
      for (const { file, jsdom, worker } of pairs) {
        expect({ file, images: worker }).toStrictEqual({ file, images: jsdom });
      }
    } finally {
      await roomy.close();
    }
  });

  it("reads <style>, inline styles, background and srcset", async () => {
    const html = [
      "<style>p { background: url(https://images.example.com/s.png) }</style>",
      '<div style="background-image:url(https://images.example.com/i.png)">',
      '<table background="https://images.example.com/b.png"><tr><td>',
      '<img srcset="https://images.example.com/1x.png 1x, https://images.example.com/2x.png 2x">',
      "</td></tr></table></div>",
      '<template><img src="https://images.example.com/t.png"></template>',
    ].join("");
    const roomy = createWorkerExtractor();
    try {
      await expect(roomy.extract(html)).resolves.toStrictEqual(
        extractRemoteImageUrls(html, { parser })
      );
    } finally {
      await roomy.close();
    }
  });

  it.each(vectors.vectors)(
    "$id: no image, within the deadline",
    async ({ unit, repeat }) => {
      // Warm: the first worker start is not what this measures.
      await extractor.extract("<p>warm</p>");
      const html = `${unit.repeat(repeat)}${vectors.suffix}`;
      const started = performance.now();
      const result = await extractor.extract(html);
      expect(performance.now() - started).toBeLessThan(1000);
      expect(result?.urls ?? []).toStrictEqual([]);
    }
  );

  it("works again after a parse was killed; fails closed when full or closed", async () => {
    const slow = vectors.vectors.find((v) => v.id === "table-a-closed");
    const hostile = `${(slow?.unit ?? "").repeat(slow?.repeat ?? 0)}${vectors.suffix}`;
    const own = createWorkerExtractor({
      maxQueue: 1,
      timeoutMs: 100,
      workers: 1,
    });
    const ok = '<img src="https://images.example.com/a.png">';
    // Limits wide enough for the prefilter to let it through (it refuses
    // these vectors with the default ones): only the deadline stops it.
    const wide = { maxElements: 100_000_000, maxNestingDepth: 1_000_000 };
    const [killed, queued, refused] = await Promise.all([
      own.extract(hostile, { limits: wide }),
      own.extract(ok),
      own.extract(ok),
    ]);
    expect([killed, queued?.urls, refused]).toStrictEqual([
      null,
      ["https://images.example.com/a.png"],
      null,
    ]);
    await own.close();
    await expect(own.extract(ok)).resolves.toBeNull();
  });

  it("caps the stack while parsing and the real depth of the tree", () => {
    // A stray end tag closes nothing: the stack grows past the cap.
    expect(parseHere("<div></x>".repeat(500), 400)).toMatchObject({
      ok: false,
      reason: "too-deep",
    });
    // Stack within 100, tree deeper than 100: the tree check stops it.
    const deeper = "<a><b></table>x<nobr><div><table>".repeat(40);
    expect(parseHere(deeper, 100)).toMatchObject({
      ok: false,
      reason: "too-deep",
    });
    expect(parseHere(deeper, 1000)).toMatchObject({ ok: true });
    expect(parseHere("<div>".repeat(390), 400)).toMatchObject({ ok: true });
  });

  it("refuses bad options", () => {
    expect(() => createWorkerExtractor({ timeoutMs: 0 })).toThrow(TypeError);
    expect(() => createWorkerExtractor({ workers: 1.5 })).toThrow(TypeError);
  });
});
