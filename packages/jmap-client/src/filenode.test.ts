// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";
import { z } from "zod";

import { JmapClient } from "./client";
import { FileNodeId, fileNodeCreate } from "./filenode";
import { AccountId, BlobId } from "./ids";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  sessionThen,
  tokens,
} from "./test-support";

const accountId = AccountId.parse("d");

describe("FileNode", () => {
  it("creates a folder and a file in it in one request", async () => {
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          [
            "FileNode/set",
            {
              accountId: "d",
              created: { dir: { id: "b" } },
              newState: "s2",
              oldState: "s1",
            },
            "c0",
          ],
          [
            "FileNode/set",
            {
              accountId: "d",
              created: { file: { id: "c" } },
              newState: "s3",
              oldState: "s2",
            },
            "c1",
          ],
          [
            "FileNode/get",
            {
              accountId: "d",
              list: [
                {
                  "@type": "File",
                  blobId: "Gf",
                  id: "c",
                  modified: "2026-10-26T21:10:45Z",
                  name: "preferences.json",
                  parentId: "b",
                },
              ],
              notFound: [],
              state: "s3",
            },
            "c2",
          ],
        ])
      )
    );
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    const blobId = BlobId.parse("Gf");
    const [dir, file, got] = await client.request(
      (b) =>
        [
          b.call("FileNode/set", {
            accountId,
            create: {
              dir: { "@type": "Directory", name: ".app", parentId: null },
            },
          }),
          b.call("FileNode/set", {
            accountId,
            create: {
              file: {
                "@type": "File",
                blobId,
                mediaType: "application/json",
                name: "preferences.json",
                parentId: "#dir",
              },
            },
          }),
          b.call("FileNode/get", {
            accountId,
            ids: [FileNodeId.parse("c")],
            properties: ["blobId", "name"],
          }),
        ] as const
    );
    expect(dir.created.dir?.id).toBe("b");
    expect(file.created.file?.id).toBe("c");
    expect(got.list[0]?.blobId).toBe("Gf");
    const body = z
      .object({
        methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
        using: z.array(z.string()),
      })
      .parse(fake.calls[1]?.body);
    expect(body.using).toContain("urn:ietf:params:jmap:filenode");
  });
});

describe(fileNodeCreate, () => {
  const blobId = BlobId.parse("Gf");

  it("sends @type, and mediaType for a file", () => {
    expect(
      fileNodeCreate({
        kind: "directory",
        name: ".app",
        parentId: null,
      })
    ).toStrictEqual({ "@type": "Directory", name: ".app", parentId: null });
    expect(
      fileNodeCreate({
        blobId,
        kind: "file",
        mediaType: "application/json",
        name: "a.json",
        parentId: "#dir",
      })
    ).toStrictEqual({
      "@type": "File",
      blobId,
      mediaType: "application/json",
      name: "a.json",
      parentId: "#dir",
    });
  });

  it("takes an update answered with what the server changed, without the id", async () => {
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          // Stalwart sends the new size and dates (no id).
          [
            "FileNode/set",
            {
              accountId: "d",
              newState: "s4",
              oldState: "s3",
              updated: { c: { modified: "2026-10-30T12:55:00Z", size: 42 } },
            },
            "c0",
          ],
        ])
      )
    );
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    const [set] = await client.request((b) => [
      b.call("FileNode/set", {
        accountId,
        update: { [FileNodeId.parse("c")]: { blobId: BlobId.parse("Gg") } },
      }),
    ]);
    expect(set.updated).toStrictEqual({
      c: { modified: "2026-10-30T12:55:00Z", size: 42 },
    });
  });
});
