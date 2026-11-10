// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

import "fake-indexeddb/auto";
import { IdentityId, JmapClient } from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { session } from "../../e2e/fake-jmap";
import { accountKey } from "./account-key";
import { forgetDeviceDb, withStore } from "./device-db";
import { loadIdentities, saveIdentities } from "./identity-cache";

const client = vi.hoisted(() => ({ online: true }));

// A real client on the browser tests' fake session (http://jmap.test,
// jane@example.com); "offline" makes connecting fail like fetch does.
vi.mock(import("./client"), () => ({
  getClient: async () => {
    if (!client.online) {
      throw new TypeError("Failed to fetch");
    }
    return await JmapClient.connect({
      fetch: async () => await Promise.resolve(Response.json(session())),
      getAccessToken: async () => await Promise.resolve("token"),
      sessionUrl: "http://jmap.test/.well-known/jmap",
    });
  },
  resetClient: vi.fn<() => void>(),
}));

const ACCOUNT = "http://jmap.test jane@example.com";

const alice = {
  email: "alice@x.com",
  id: IdentityId.parse("i1"),
  name: "Alice",
  signature: "<p>Alice</p>",
};

describe("identities on this device", () => {
  beforeEach(async () => {
    client.online = true;
    await forgetDeviceDb();
  });

  it("are saved and read back", async () => {
    await saveIdentities(ACCOUNT, [alice]);
    await expect(loadIdentities()).resolves.toStrictEqual({
      account: ACCOUNT,
      identities: [alice],
    });
  });

  it("keep the Reply-To and Bcc (RFC 8621 §6)", async () => {
    const withLists = {
      ...alice,
      bcc: [{ email: ["archive", "example.com"].join("@") }],
      replyTo: [{ email: ["help", "example.com"].join("@"), name: "Help" }],
    };
    await saveIdentities(ACCOUNT, [withLists]);
    await expect(loadIdentities()).resolves.toStrictEqual({
      account: ACCOUNT,
      identities: [withLists],
    });
  });

  it("sanitize signatures again on the way in", async () => {
    await withStore("identities", "readwrite", (store) =>
      store.put({
        account: ACCOUNT,
        id: "current",
        identities: [
          {
            ...alice,
            signature: `<p>Alice<img src="https://t.example/p.gif"></p>`,
          },
        ],
        savedAt: 1,
      })
    );
    await expect(loadIdentities()).resolves.toStrictEqual({
      account: ACCOUNT,
      identities: [{ ...alice, signature: "<p>Alice</p>" }],
    });
  });

  it("ignore a record they can't use", async () => {
    await withStore("identities", "readwrite", (store) =>
      store.put({ id: "current", identities: "nope" })
    );
    await expect(loadIdentities()).resolves.toBeNull();
  });

  it("give the account key when the session can't be reached", async () => {
    await expect(accountKey()).resolves.toBe(ACCOUNT);
    client.online = false;
    await expect(accountKey()).rejects.toThrow("Failed to fetch");
    await saveIdentities(ACCOUNT, [alice]);
    await expect(accountKey()).resolves.toBe(ACCOUNT);
  });

  it("are forgotten on logout", async () => {
    await saveIdentities(ACCOUNT, [alice]);
    await forgetDeviceDb();
    await expect(loadIdentities()).resolves.toBeNull();
  });
});
