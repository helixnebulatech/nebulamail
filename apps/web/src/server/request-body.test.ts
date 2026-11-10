// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { readJsonBody } from "./request-body";

const CHUNK = 64 * 1024;

/** A 10 MB body as a stream that counts what was read from it. */
const hugeStream = () => {
  let pulled = 0;
  const stream = new ReadableStream<Uint8Array>({
    pull: (controller) => {
      if (pulled >= 10 * 1024 * 1024) {
        controller.close();
        return;
      }
      pulled += CHUNK;
      controller.enqueue(new Uint8Array(CHUNK).fill(0x61));
    },
  });
  return { pulled: () => pulled, stream };
};

const post = (body: BodyInit | null, headers: Record<string, string> = {}) =>
  new Request("https://mail.example.com/api/x", {
    body,
    // Needed by Node to send a stream as the body.
    duplex: "half",
    headers,
    method: "POST",
  } as RequestInit);

describe(readJsonBody, () => {
  it("reads a small JSON body", async () => {
    await expect(readJsonBody(post('{"a":1}'), 1024)).resolves.toStrictEqual({
      ok: true,
      value: { a: 1 },
    });
  });

  it("stops reading a huge stream right after the limit", async () => {
    const { pulled, stream } = hugeStream();
    const result = await readJsonBody(post(stream), 1024);

    expect(result).toStrictEqual({ ok: false, status: 413 });
    expect(pulled()).toBeLessThanOrEqual(1024 + 2 * CHUNK);
  });

  it("refuses a too large Content-Length without reading the body", async () => {
    const { pulled, stream } = hugeStream();
    const result = await readJsonBody(
      post(stream, { "content-length": "999999" }),
      1024
    );

    expect(result).toStrictEqual({ ok: false, status: 413 });
    expect(pulled()).toBeLessThanOrEqual(CHUNK);
  });

  it.each(["abc", "-1", "1.5", "1e3"])(
    "refuses Content-Length %j as invalid",
    async (length) => {
      const request = post("{}", { "content-length": length });

      await expect(readJsonBody(request, 1024)).resolves.toStrictEqual({
        ok: false,
        status: 400,
      });
    }
  );

  it("refuses invalid UTF-8 and invalid JSON", async () => {
    await expect(
      readJsonBody(post(new Uint8Array([0x7b, 0xff, 0x7d])), 1024)
    ).resolves.toStrictEqual({ ok: false, status: 400 });
    await expect(readJsonBody(post("{nope"), 1024)).resolves.toStrictEqual({
      ok: false,
      status: 400,
    });
  });

  it("takes an empty body only when told what it means", async () => {
    await expect(readJsonBody(post(null), 1024)).resolves.toStrictEqual({
      ok: false,
      status: 400,
    });
    await expect(
      readJsonBody(post(""), 1024, { emptyAs: {} })
    ).resolves.toStrictEqual({ ok: true, value: {} });
  });
});
