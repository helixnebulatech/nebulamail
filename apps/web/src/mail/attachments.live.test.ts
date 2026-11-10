// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Blob/copy against a real Stalwart: a blob of an email in
// a folder shared with us is copied into our account and can be
// downloaded from it. Needs `bun run dev:share` data. Skipped unless
// these are set (values from docker/.env):
//
//   SHARE_LIVE_URL=http://localhost:8080 \
//   SHARE_LIVE_USER=alice@nebulamail.test SHARE_LIVE_PASSWORD=… bun run test
//
// Nothing to clean up: an unused blob is deleted by the server.

import type { FetchLike, JmapClient } from "@nebulamail/jmap-client";
import {
  AccountId,
  BlobId,
  JmapClient as Client,
} from "@nebulamail/jmap-client";
import { beforeAll, describe, expect, it, vi } from "vitest";

const env = {
  password: process.env.SHARE_LIVE_PASSWORD,
  url: process.env.SHARE_LIVE_URL,
  user: process.env.SHARE_LIVE_USER,
};
const configured = Object.values(env).every(
  (value) => value !== undefined && value !== ""
);

const basicFetch =
  (user: string, password: string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("Authorization", `Basic ${btoa(`${user}:${password}`)}`);
    return await fetch(input, { ...init, headers });
  };

const live: { client: JmapClient | null } = { client: null };

vi.mock(import("./client"), () => ({
  getClient: async () => {
    if (live.client === null) {
      throw new Error("not connected");
    }
    return await Promise.resolve(live.client);
  },
}));

const { copyToOwnAccount } = await import("./attachments");

describe.skipIf(!configured)("live Stalwart: Blob/copy", () => {
  beforeAll(async () => {
    live.client = await Client.connect({
      fetch: basicFetch(env.user ?? "", env.password ?? ""),
      getAccessToken: async () => await Promise.resolve("basic"),
      sessionUrl: `${env.url ?? ""}/.well-known/jmap`,
    });
  });

  it("copies a shared email's blob into our account", async () => {
    const { client } = live;
    if (client === null) {
      throw new Error("not connected");
    }
    const primary = client.primaryAccountId();
    const shared = AccountId.parse(
      Object.keys(client.session.accounts).find((id) => id !== primary)
    );
    const [, get] = await client.request((b) => {
      const query = b.call("Email/query", { accountId: shared, limit: 1 });
      const emails = b.call("Email/get", {
        "#ids": query.ref("/ids"),
        accountId: shared,
        properties: ["blobId", "size"],
      });
      return [query, emails] as const;
    });
    const [email] = get.list;
    expect(email?.blobId).toBeDefined();
    const result = await copyToOwnAccount([
      {
        accountId: shared,
        blobId: BlobId.parse(email?.blobId),
        name: "message.eml",
        size: email?.size ?? 0,
        type: "message/rfc822",
      },
    ]);
    expect(result.failed).toBe(0);
    const [copy] = result.attachments;
    const blob = await client.downloadBlob({
      accountId: primary,
      blobId: BlobId.parse(copy?.blobId),
      maxBytes: 10_000_000,
      name: "message.eml",
      type: "message/rfc822",
    });
    expect(blob.size).toBe(email?.size);
  });
});
