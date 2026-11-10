// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Our generated Sieve against a real Stalwart: the server accepts it, a
// rule really files a message sent over SMTP, and the script reads back as
// rules. Skipped unless these are set (values from docker/.env):
//
//   SIEVE_LIVE_URL=http://localhost:8080 \
//   SIEVE_LIVE_USER=alice@nebulamail.test SIEVE_LIVE_PASSWORD=… \
//   SIEVE_LIVE_SENDER=bob@nebulamail.test SIEVE_LIVE_SENDER_PASSWORD=… \
//   bun run test
//
// It leaves the account as it found it: the previously active script is
// activated again, and the test script, folder and messages are deleted.

import { setTimeout as sleep } from "node:timers/promises";
import { connect } from "node:tls";

import type { AccountId, FetchLike, JmapClient } from "@nebulamail/jmap-client";
import {
  JmapClient as Client,
  MailboxId,
  SieveScriptId,
} from "@nebulamail/jmap-client";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { generateScript } from "./generate";
import type { FilterSet } from "./model";
import { parseFilterScript } from "./parse";

const env = {
  password: process.env.SIEVE_LIVE_PASSWORD,
  sender: process.env.SIEVE_LIVE_SENDER,
  senderPassword: process.env.SIEVE_LIVE_SENDER_PASSWORD,
  url: process.env.SIEVE_LIVE_URL,
  user: process.env.SIEVE_LIVE_USER,
};

const configured = Object.values(env).every(
  (value) => value !== undefined && value !== ""
);

/** Stalwart takes basic auth too; swap the client's Bearer header for it. */
const basicFetch =
  (user: string, password: string): FetchLike =>
  async (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("Authorization", `Basic ${btoa(`${user}:${password}`)}`);
    return await fetch(input, { ...init, headers });
  };

/** Minimal SMTP submission (implicit TLS on 1465, AUTH PLAIN). */
const sendMail = async (message: string): Promise<void> => {
  const socket = connect({
    host: "127.0.0.1",
    port: 1465,
    // The development server's certificate is self-signed.
    rejectUnauthorized: false,
  });
  socket.setEncoding("utf-8");
  const replies: ((line: string) => void)[] = [];
  let buffer = "";
  socket.on("data", (chunk: string) => {
    buffer += chunk;
    let end = buffer.indexOf("\r\n");
    while (end !== -1) {
      const line = buffer.slice(0, end);
      buffer = buffer.slice(end + 2);
      if (/^\d{3} /u.test(line)) {
        replies.shift()?.(line);
      }
      end = buffer.indexOf("\r\n");
    }
  });
  const reply = async (): Promise<string> => {
    const { promise, resolve } = Promise.withResolvers<string>();
    replies.push(resolve);
    return await promise;
  };
  const command = async (text: string): Promise<string> => {
    const answer = reply();
    socket.write(`${text}\r\n`);
    return await answer;
  };
  await reply();
  await command("EHLO sieve-live-test");
  const auth = btoa(`\0${env.sender ?? ""}\0${env.senderPassword ?? ""}`);
  await expect(command(`AUTH PLAIN ${auth}`)).resolves.toMatch(/^235/u);
  await command(`MAIL FROM:<${env.sender ?? ""}>`);
  await command(`RCPT TO:<${env.user ?? ""}>`);
  await command("DATA");
  await expect(
    command(`${message.replaceAll("\n", "\r\n")}\r\n.`)
  ).resolves.toMatch(/^250/u);
  await command("QUIT");
  socket.end();
};

describe.skipIf(!configured)("live Stalwart Sieve", () => {
  const token = `nebula-live-${Date.now().toString(36)}`;
  let client: JmapClient;
  let accountId: AccountId;
  let previouslyActive: SieveScriptId | null = null;
  let scriptId: SieveScriptId | null = null;
  let folderId: MailboxId;

  const upload = async (text: string) =>
    await client.upload(accountId, new Blob([text]), {
      type: "application/sieve",
    });

  beforeAll(async () => {
    client = await Client.connect({
      fetch: basicFetch(env.user ?? "", env.password ?? ""),
      getAccessToken: async () => await Promise.resolve("basic"),
      sessionUrl: `${env.url ?? ""}/.well-known/jmap`,
    });
    accountId = client.primaryAccountId();
    const [scripts, folder] = await client.request((b) => [
      b.call("SieveScript/get", { accountId }),
      b.call("Mailbox/set", {
        accountId,
        create: { f: { name: `Live test ${token}` } },
      }),
    ]);
    previouslyActive =
      scripts.list.find((script) => script.isActive)?.id ?? null;
    folderId = MailboxId.parse(folder.created.f?.id);
  });

  afterAll(async () => {
    if (client === undefined) {
      return;
    }
    const [query] = await client.request((b) => [
      b.call("Email/query", { accountId, filter: { inMailbox: folderId } }),
    ]);
    await client.request((b) => [
      b.call("SieveScript/set", {
        accountId,
        ...(previouslyActive === null
          ? { onSuccessDeactivateScript: true }
          : { onSuccessActivateScript: previouslyActive }),
      }),
    ]);
    await client.request((b) => [
      b.call("Email/set", { accountId, destroy: query.ids }),
      b.call("Mailbox/set", { accountId, destroy: [folderId] }),
      ...(scriptId === null
        ? []
        : [b.call("SieveScript/set", { accountId, destroy: [scriptId] })]),
    ]);
  });

  it("files a message with a generated rule, and reads the script back", async () => {
    const filters: FilterSet = {
      rules: [
        {
          actions: [
            { kind: "markRead" },
            { keyword: "nebula-live", kind: "keyword" },
            { kind: "move", mailboxId: folderId, mailboxName: "wrong name" },
          ],
          conditions: [
            {
              field: "subject",
              header: "",
              kind: "match",
              negate: false,
              op: "contains",
              value: token,
            },
          ],
          enabled: true,
          id: "r1",
          match: "all",
          // Hostile text in every string must not change what runs.
          name: '"; discard; # \\"',
        },
        {
          actions: [{ kind: "discard" }],
          conditions: [],
          enabled: false,
          id: "r2",
          match: "all",
          name: "Never runs",
        },
        {
          // Matches too, after r1 filed it: guarded, so it doesn't reject
          // (RFC 5429 §2.4; generate.ts, `deliveredBefore`).
          actions: [{ kind: "reject", reason: "Must not bounce." }],
          conditions: [],
          enabled: true,
          id: "r3",
          match: "all",
          name: "Reject the rest",
        },
      ],
      // Not the vacation response: it would answer the sender.
      vacation: false,
    };
    const script = generateScript(filters);
    const { blobId } = await upload(script);
    const [validation] = await client.request((b) => [
      b.call("SieveScript/validate", { accountId, blobId }),
    ]);
    expect(validation.error).toBeNull();
    const [set] = await client.request((b) => [
      b.call("SieveScript/set", {
        accountId,
        create: { s: { blobId, name: `nebula-live-${token}` } },
        onSuccessActivateScript: "#s",
      }),
    ]);
    const id = SieveScriptId.parse(set.created.s?.id);
    scriptId = id;

    await sendMail(
      [
        `From: <${env.sender ?? ""}>`,
        `To: <${env.user ?? ""}>`,
        `Subject: Filter test ${token}`,
        `Message-ID: <${token}@sieve-live-test>`,
        `Date: ${new Date().toUTCString()}`,
        "",
        "Hello from the live Sieve test.",
      ].join("\n")
    );

    const filed = async (): Promise<string[]> => {
      const [, get] = await client.request((b) => {
        const query = b.call("Email/query", {
          accountId,
          filter: { inMailbox: folderId },
        });
        return [
          query,
          b.call("Email/get", {
            "#ids": query.ref("/ids"),
            accountId,
            properties: ["keywords", "subject"],
          }),
        ] as const;
      });
      return Object.keys(get.list[0]?.keywords ?? {});
    };
    let keywords: string[] = [];
    for (let attempt = 0; attempt < 30 && keywords.length === 0; attempt += 1) {
      // oxlint-disable-next-line no-await-in-loop -- polling for delivery.
      await sleep(500);
      // oxlint-disable-next-line no-await-in-loop -- polling for delivery.
      keywords = await filed();
    }
    expect(keywords).toStrictEqual(
      expect.arrayContaining(["$seen", "nebula-live"])
    );

    const [scripts] = await client.request((b) => [
      b.call("SieveScript/get", {
        accountId,
        ids: [id],
      }),
    ]);
    const [stored] = scripts.list;
    const response = await client.download({
      accountId,
      blobId: stored?.blobId ?? blobId,
      type: "application/sieve",
    });
    const text = await response.text();
    expect(parseFilterScript(text)?.rules).toHaveLength(3);
  }, 30_000);

  it("reports errors with their line", async () => {
    const { blobId } = await upload(
      'require "fileinto";\nif true {\n  stop\n}\n'
    );
    const [validation] = await client.request((b) => [
      b.call("SieveScript/validate", { accountId, blobId }),
    ]);
    expect(validation.error?.description).toMatch(/line 4/u);
  });
});
