// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import { AccountId, BlobId } from "./ids";
import {
  SESSION_URL,
  fakeFetch,
  rejection,
  sessionThen,
  stalwartSession,
  tokens,
} from "./test-support";
import type { Handler } from "./test-support";

const accountId = AccountId.parse("d");
const blobId = BlobId.parse("G1");

const setup = async (api: Handler, maxSizeUpload = 10) => {
  const fake = fakeFetch(
    sessionThen(api, stalwartSession({}, { maxSizeUpload }))
  );
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...tokens(),
  });
  return { calls: fake.calls, client };
};

const uploaded = (size: number) =>
  Response.json(
    { accountId: "d", blobId: "G9", size, type: "text/plain" },
    { status: 201 }
  );

const streamOf = (...chunks: string[]): ReadableStream<Uint8Array> =>
  new ReadableStream({
    start: (controller) => {
      for (const chunk of chunks) {
        controller.enqueue(new TextEncoder().encode(chunk));
      }
      controller.close();
    },
  });

describe("upload", () => {
  it("rejects a Blob above maxSizeUpload before sending anything", async () => {
    const { calls, client } = await setup(() => uploaded(0));
    const error = await rejection(
      client.upload(accountId, new Blob(["0123456789AB"]))
    );
    expect(error.detail).toStrictEqual({
      actual: 12,
      kind: "limit",
      limit: "maxSizeUpload",
      max: 10,
    });
    expect(calls).toHaveLength(1);
  });

  it("uploads to the expanded uploadUrl and parses the blob id", async () => {
    const { calls, client } = await setup(() => uploaded(5));
    const result = await client.upload(
      accountId,
      new Blob(["hello"], { type: "text/plain" })
    );
    expect(result.blobId).toBe("G9");
    expect(calls[1]?.url).toBe("https://mail.example.com/jmap/upload/d/");
    expect(new Headers(calls[1]?.init.headers).get("content-type")).toBe(
      "text/plain"
    );
  });

  it("meters stream bodies and reports progress", async () => {
    const progress: number[] = [];
    const { client } = await setup(async ({ body }) => {
      const text = await new Response(
        body instanceof ReadableStream ? body : null
      ).text();
      return uploaded(text.length);
    });
    await client.upload(accountId, streamOf("abc", "de"), {
      onProgress: (sent) => {
        progress.push(sent);
      },
    });
    expect(progress).toStrictEqual([3, 5, 5]);
  });

  it("errors a stream that grows past maxSizeUpload", async () => {
    const { client } = await setup(async ({ body }) => {
      await new Response(body instanceof ReadableStream ? body : null).text();
      return uploaded(0);
    });
    await expect(
      client.upload(accountId, streamOf("012345", "6789AB"))
    ).rejects.toMatchObject({
      detail: { kind: "limit", limit: "maxSizeUpload" },
    });
  });
});

describe("download", () => {
  it("expands and encodes the download URL", async () => {
    const { client } = await setup(() => new Response("x"));
    expect(
      client.downloadUrl({
        accountId,
        blobId,
        name: "a b/c.txt",
        type: "text/plain",
      }).href
    ).toBe(
      "https://mail.example.com/jmap/download/d/G1/a%20b%2Fc.txt?accept=text%2Fplain"
    );
  });

  it("refuses a declared Content-Length above maxBytes", async () => {
    const { client } = await setup(
      () => new Response("0123456789", { headers: { "Content-Length": "10" } })
    );
    const error = await rejection(
      client.download({ accountId, blobId, maxBytes: 4 })
    );
    expect(error.detail).toMatchObject({ kind: "limit", limit: "maxBytes" });
  });

  it("stops reading a body that grows past maxBytes", async () => {
    const { client } = await setup(() => new Response(streamOf("012", "345")));
    const response = await client.download({ accountId, blobId, maxBytes: 4 });
    await expect(response.text()).rejects.toMatchObject({
      detail: { kind: "limit" },
    });
  });

  it("returns the body when within limits", async () => {
    const { calls, client } = await setup(() => new Response("hi"));
    const blob = await client.downloadBlob({ accountId, blobId, maxBytes: 4 });
    await expect(blob.text()).resolves.toBe("hi");
    expect(calls[1]?.authorization).toBe("Bearer old-token");
  });
});
