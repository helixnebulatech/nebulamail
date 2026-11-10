// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The incremental sync (sync.ts) against a real JMAP server: the list is
// loaded as the app does (`emailListQuery`), then each change on the
// server is followed with `Email/queryChanges` (RFC 8620 §5.6) and the
// cache must end up as `Email/query` says, one row per thread. The steps
// are those of Stalwart's old bug
// a reply that takes its thread's row, and back.
// Skipped unless these are set (bob's password from docker/.env; the
// bridge's URL and login work too):
//
//   SYNC_LIVE_URL=http://localhost:8080 \
//   SYNC_LIVE_USER=[EMAIL] SYNC_LIVE_PASSWORD=… bun run test
//
// Uses a folder of its own, deleted at the end with its emails. Emails
// are imported; nothing is sent.

import type {
  AccountId,
  EmailId,
  FetchLike,
  JmapClient,
} from "@nebulamail/jmap-client";
import { JmapClient as Client, MailboxId } from "@nebulamail/jmap-client";
import { InfiniteQueryObserver, QueryClient } from "@tanstack/react-query";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { z } from "zod";

const env = {
  password: process.env.SYNC_LIVE_PASSWORD,
  url: process.env.SYNC_LIVE_URL,
  user: process.env.SYNC_LIVE_USER,
};
const configured = Object.values(env).every(
  (value) => value !== undefined && value !== ""
);

const MethodCalls = z.object({
  methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
});

/** Every JMAP method the client sent, in order. */
const sent: string[] = [];

const basicFetch =
  (user: string, pass: string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("Authorization", `Basic ${btoa(`${user}:${pass}`)}`);
    if (typeof init?.body === "string") {
      const parsed = MethodCalls.safeParse(JSON.parse(init.body));
      if (parsed.success) {
        sent.push(...parsed.data.methodCalls.map(([name]) => name));
      }
    }
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

const { emailListQuery } = await import("./queries");
const { createMailSync, jmapSyncServer } = await import("./sync");

const connected = (): JmapClient => {
  if (live.client === null) {
    throw new Error("not connected");
  }
  return live.client;
};

describe.skipIf(!configured)("live: incremental sync with queryChanges", () => {
  const token = `sync-live-${Date.now().toString(36)}`;
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  const state: {
    accountId: AccountId | null;
    folder: MailboxId | null;
    stop: () => void;
  } = {
    accountId: null,
    folder: null,
    stop: () => {
      // Set once the list is observed.
    },
  };

  const accountId = (): AccountId => {
    if (state.accountId === null) {
      throw new Error("no account");
    }
    return state.accountId;
  };

  const folder = (): MailboxId => {
    if (state.folder === null) {
      throw new Error("no folder");
    }
    return state.folder;
  };

  /** An email in the folder; `minutesAgo` sets `receivedAt` (the order). */
  const importEmail = async (
    subject: string,
    minutesAgo: number,
    replyTo?: string
  ): Promise<{ readonly id: EmailId; readonly messageId: string }> => {
    const client = connected();
    const messageId = `${crypto.randomUUID()}@sync-live.example`;
    const raw = [
      "From: Test <[EMAIL]>",
      `To: ${env.user ?? ""}`,
      `Subject: ${subject}`,
      `Message-ID: <${messageId}>`,
      ...(replyTo === undefined
        ? []
        : [`In-Reply-To: <${replyTo}>`, `References: <${replyTo}>`]),
      "",
      "Test email.",
      "",
    ].join("\r\n");
    const { blobId } = await client.upload(accountId(), new Blob([raw]), {
      type: "message/rfc822",
    });
    const [result] = await client.request((b) => [
      b.call("Email/import", {
        accountId: accountId(),
        emails: {
          e: {
            blobId,
            mailboxIds: { [folder()]: true },
            receivedAt: new Date(Date.now() - minutesAgo * 60_000)
              .toISOString()
              .replace(/\.\d+Z$/u, "Z"),
          },
        },
      }),
    ]);
    const created = result.created?.e;
    if (created === undefined || created === null) {
      throw new Error(`import failed: ${JSON.stringify(result.notCreated)}`);
    }
    return { id: created.id, messageId };
  };

  /** The rows the cache shows, by id. */
  const cachedIds = (): EmailId[] =>
    (
      queryClient.getQueryData(emailListQuery(folder()).queryKey)?.pages ?? []
    ).flatMap((page) => page.emails.map((row) => row.id));

  /** What the server says the list is now, one row per thread. */
  const serverIds = async (): Promise<EmailId[]> => {
    const [query] = await connected().request((b) => [
      b.call("Email/query", {
        accountId: accountId(),
        collapseThreads: true,
        filter: { inMailbox: folder() },
        sort: [{ isAscending: false, property: "receivedAt" }],
      }),
    ]);
    return query.ids;
  };

  const sync = {
    current: null as ReturnType<typeof createMailSync> | null,
  };

  /** Push said something changed: sync, and say which methods it used. */
  const changed = async (): Promise<string[]> => {
    const from = sent.length;
    await sync.current?.changed(accountId());
    await vi.waitFor(() => {
      expect(queryClient.isFetching()).toBe(0);
    });
    return sent.slice(from);
  };

  beforeAll(async () => {
    const client = await Client.connect({
      fetch: basicFetch(env.user ?? "", env.password ?? ""),
      getAccessToken: async () => await Promise.resolve("basic"),
      sessionUrl: `${env.url ?? ""}/.well-known/jmap`,
    });
    live.client = client;
    state.accountId = client.primaryAccountId();
    const [created] = await client.request((b) => [
      b.call("Mailbox/set", {
        accountId: accountId(),
        create: { box: { name: token } },
      }),
    ]);
    const box = created.created?.box;
    if (box === undefined || box === null) {
      throw new Error(`no folder: ${JSON.stringify(created.notCreated)}`);
    }
    state.folder = MailboxId.parse(box.id);
  });

  afterAll(async () => {
    state.stop();
    if (live.client !== null && state.folder !== null) {
      await live.client.request((b) => [
        b.call("Mailbox/set", {
          accountId: accountId(),
          destroy: [folder()],
          onDestroyRemoveEmails: true,
        }),
      ]);
    }
  });

  // One step after another, on the same folder and cache.
  const emails: Record<string, { id: EmailId; messageId: string }> = {};
  const email = (name: string): { id: EmailId; messageId: string } => {
    const found = emails[name];
    if (found === undefined) {
      throw new Error(`no email ${name}`);
    }
    return found;
  };

  it("loads the list with its query state", async () => {
    emails.a = await importEmail(`${token} A`, 10);
    emails.b = await importEmail(`${token} B`, 5);
    const observer = new InfiniteQueryObserver(
      queryClient,
      emailListQuery(folder())
    );
    state.stop = observer.subscribe(() => {
      // The test reads the cache.
    });
    sync.current = createMailSync(queryClient, jmapSyncServer(connected()));
    await sync.current.reset();
    await vi.waitFor(() => {
      expect(cachedIds()).toStrictEqual([email("b").id, email("a").id]);
    });
    expect(
      queryClient.getQueryData(emailListQuery(folder()).queryKey)?.pages[0]
        ?.queryState
    ).toBeTruthy();
  });

  it("follows a reply that takes its thread's row, without asking the ids", async () => {
    emails.reply = await importEmail(`Re: ${token} A`, 1, email("a").messageId);
    const methods = await changed();
    expect(cachedIds()).toStrictEqual(await serverIds());
    expect(cachedIds()).toStrictEqual([email("reply").id, email("b").id]);
    expect(methods).toContain("Email/queryChanges");
    expect(methods).not.toContain("Email/query");
  });

  it("brings the thread's row back when the reply goes", async () => {
    await connected().request((builder) => [
      builder.call("Email/set", {
        accountId: accountId(),
        destroy: [email("reply").id],
      }),
    ]);
    const methods = await changed();
    expect(cachedIds()).toStrictEqual(await serverIds());
    expect(cachedIds()).toStrictEqual([email("b").id, email("a").id]);
    expect(methods).not.toContain("Email/query");
  });

  it("puts a new thread on top", async () => {
    emails.c = await importEmail(`${token} C`, 0);
    const methods = await changed();
    expect(cachedIds()).toStrictEqual(await serverIds());
    expect(cachedIds()).toStrictEqual([
      email("c").id,
      email("b").id,
      email("a").id,
    ]);
    expect(methods).not.toContain("Email/query");
  });

  it("fetches a row that was read again, in its place", async () => {
    await connected().request((builder) => [
      builder.call("Email/set", {
        accountId: accountId(),
        update: { [email("b").id]: { "keywords/$seen": true } },
      }),
    ]);
    const methods = await changed();
    expect(cachedIds()).toStrictEqual(await serverIds());
    const rows =
      queryClient.getQueryData(emailListQuery(folder()).queryKey)?.pages[0]
        ?.emails ?? [];
    expect(rows.find((row) => row.id === email("b").id)?.unread).toBeFalsy();
    expect(methods).not.toContain("Email/query");
  });
});
