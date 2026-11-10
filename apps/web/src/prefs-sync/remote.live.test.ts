// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The synced preferences against a real Stalwart: the
// FileNode file and the mail-folder fallback, written twice and read
// back. Skipped unless these are set (values from docker/.env):
//
//   PREFS_LIVE_URL=http://localhost:8080 \
//   PREFS_LIVE_USER=bob@nebulamail.test PREFS_LIVE_PASSWORD=… bun run test
//
// Careful: it uses the account's real `.nebula-mail` folder and "Nebula
// Mail" mailbox, so it saves what was there and puts it back.

import type { FetchLike, JmapClient } from "@nebulamail/jmap-client";
import { JmapClient as Client } from "@nebulamail/jmap-client";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { EMPTY_DOC, withValue } from "./model";
import type { SyncDoc } from "./model";
import { SYNC_DIR, SYNC_MAILBOX, backendFor, remoteFor } from "./remote";

const env = {
  password: process.env.PREFS_LIVE_PASSWORD,
  url: process.env.PREFS_LIVE_URL,
  user: process.env.PREFS_LIVE_USER,
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

describe.skipIf(!configured)("live Stalwart: synced preferences", () => {
  let client: JmapClient;
  const saved: { filenode: SyncDoc | null; mail: SyncDoc | null } = {
    filenode: null,
    mail: null,
  };
  let hadDir = false;
  let hadMailbox = false;

  const findDir = async () => {
    const [dirs] = await client.request((b) => [
      b.call("FileNode/query", {
        accountId: client.primaryAccountId(),
        filter: { isTopLevel: true, name: SYNC_DIR },
      }),
    ]);
    return dirs.ids[0] ?? null;
  };
  const findMailbox = async () => {
    const [boxes] = await client.request((b) => [
      b.call("Mailbox/get", {
        accountId: client.primaryAccountId(),
        properties: ["name", "parentId", "isSubscribed"],
      }),
    ]);
    return boxes.list.find(
      (box) => box.name === SYNC_MAILBOX && (box.parentId ?? null) === null
    );
  };

  beforeAll(async () => {
    client = await Client.connect({
      fetch: basicFetch(env.user ?? "", env.password ?? ""),
      getAccessToken: async () => await Promise.resolve("basic"),
      sessionUrl: `${env.url ?? ""}/.well-known/jmap`,
    });
    hadDir = (await findDir()) !== null;
    hadMailbox = (await findMailbox()) !== undefined;
    saved.filenode = await remoteFor(client, "filenode").read();
    saved.mail = await remoteFor(client, "mail").read();
  });

  afterAll(async () => {
    if (client === undefined) {
      return;
    }
    const accountId = client.primaryAccountId();
    if (saved.filenode !== null) {
      await remoteFor(client, "filenode").write(saved.filenode);
    }
    const dir = await findDir();
    if (!hadDir && dir !== null) {
      await client.request((b) => [
        b.call("FileNode/set", {
          accountId,
          destroy: [dir],
          onDestroyRemoveChildren: true,
        }),
      ]);
    }
    if (saved.mail !== null) {
      await remoteFor(client, "mail").write(saved.mail);
    }
    const box = await findMailbox();
    if (!hadMailbox && box !== undefined) {
      await client.request((b) => [
        b.call("Mailbox/set", {
          accountId,
          destroy: [box.id],
          onDestroyRemoveEmails: true,
        }),
      ]);
    }
  });

  it("finds FileNode on Stalwart", () => {
    expect(backendFor(client, false)).toBe("filenode");
  });

  it.each(["filenode", "mail"] as const)(
    "writes, replaces and reads back the %s copy",
    async (kind) => {
      const remote = remoteFor(client, kind);
      const first = withValue(EMPTY_DOC, "theme", "dark", 1);
      await remote.write(first);
      await expect(remote.read()).resolves.toStrictEqual(first);
      const second = withValue(
        withValue(first, "vacationPausedScript", { id: "x", name: "Ñ 🌌" }, 2),
        "theme",
        "light",
        3
      );
      await remote.write(second);
      await expect(remote.read()).resolves.toStrictEqual(second);
    }
  );

  it("keeps one message, in an unsubscribed folder", async () => {
    const box = await findMailbox();
    expect(box?.isSubscribed).toBeFalsy();
    const [query] = await client.request((b) => [
      b.call("Email/query", {
        accountId: client.primaryAccountId(),
        filter: box === undefined ? null : { inMailbox: box.id },
      }),
    ]);
    expect(query.ids).toHaveLength(1);
  });
});
