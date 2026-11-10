// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Reproduces a Stalwart bug
// with `collapseThreads: true`, `Email/queryChanges`
// doesn't report the old row of a thread as removed when a newer email of
// that thread takes its place, nor brings it back when that email goes.
// A client that applies the changes (RFC 8620 §5.6) shows the
// conversation twice, or loses it.
//
//   bun run dev:querychanges-threads
//
// Uses bob's account and a folder of its own ("QueryChanges threads"),
// created and deleted here; emails are imported (nothing is sent). Steps,
// newest first, one row per thread:
//   1. import A (older) and B (newer), two threads: the query gives B, A;
//   2. import a reply to A (newest): the query gives reply, B. Expected
//      changes: A removed, reply added at 0. Seen on 1.0.1: only "reply
//      added at 0", so the client keeps A as well (reply, B, A);
//   3. delete the reply: the query gives B, A. Expected: reply removed, A
//      added at 1. Seen on 1.0.1: only "reply removed" (client: B).
// Without `collapseThreads` the same steps come out right.

import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { z } from "zod";

const ROOT = path.join(import.meta.dirname, "..");
const STALWART_URL = "http://127.0.0.1:8080";
const USER = "bob@nebulamail.test";
const FOLDER = "QueryChanges threads";
const USING = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"];

const password = (() => {
  const file = path.join(ROOT, "docker/.env");
  const line = existsSync(file)
    ? readFileSync(file, "utf-8")
        .split("\n")
        .find((entry) => entry.startsWith("TEST_BOB_PASSWORD="))
    : undefined;
  if (line === undefined) {
    throw new Error("No TEST_BOB_PASSWORD in docker/.env: run dev:setup");
  }
  return line.slice("TEST_BOB_PASSWORD=".length).trim();
})();
const authorization = `Basic ${btoa(`${USER}:${password}`)}`;

const Responses = z.object({
  methodResponses: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});

type Call = readonly [string, Record<string, unknown>];

/** Runs JMAP calls and fails on any method error. */
const jmap = async (
  calls: readonly Call[]
): Promise<Record<string, unknown>[]> => {
  const response = await fetch(`${STALWART_URL}/jmap/`, {
    body: JSON.stringify({
      methodCalls: calls.map(([name, callArgs], index) => [
        name,
        callArgs,
        String(index),
      ]),
      using: USING,
    }),
    headers: { authorization, "content-type": "application/json" },
    method: "POST",
  });
  if (!response.ok) {
    throw new Error(`JMAP HTTP ${response.status}: ${await response.text()}`);
  }
  const { methodResponses } = Responses.parse(await response.json());
  return methodResponses.map(([name, result]) => {
    if (name === "error") {
      throw new Error(`JMAP error: ${JSON.stringify(result)}`);
    }
    return result;
  });
};

const accountId = await (async () => {
  const session = await fetch(`${STALWART_URL}/jmap/session`, {
    headers: { authorization },
  });
  return z
    .object({
      primaryAccounts: z.object({ "urn:ietf:params:jmap:mail": z.string() }),
    })
    .parse(await session.json()).primaryAccounts["urn:ietf:params:jmap:mail"];
})();

const Created = z.object({
  created: z.record(z.string(), z.object({ id: z.string() })),
});
const Query = z.object({ ids: z.array(z.string()), queryState: z.string() });
const QueryChanges = z.object({
  added: z.array(z.object({ id: z.string(), index: z.number() })),
  newQueryState: z.string(),
  removed: z.array(z.string()),
});

const [folderResult] = await jmap([
  ["Mailbox/set", { accountId, create: { box: { name: FOLDER } } }],
]);
const {
  created: { box: createdFolder },
} = Created.parse(folderResult);
if (createdFolder === undefined) {
  throw new Error(`Couldn't create "${FOLDER}" (does it exist already?)`);
}
const folder = createdFolder.id;

/** An email in the folder; `minutesAgo` sets `receivedAt` (the order). */
const importEmail = async (
  subject: string,
  minutesAgo: number,
  replyTo?: string
): Promise<{ readonly id: string; readonly messageId: string }> => {
  const messageId = `${crypto.randomUUID()}@querychanges.example`;
  const raw = [
    "From: Test <test@querychanges.example>",
    `To: ${USER}`,
    `Subject: ${subject}`,
    `Message-ID: <${messageId}>`,
    ...(replyTo === undefined
      ? []
      : [`In-Reply-To: <${replyTo}>`, `References: <${replyTo}>`]),
    "",
    "Test email.",
    "",
  ].join("\r\n");
  const upload = await fetch(`${STALWART_URL}/jmap/upload/${accountId}/`, {
    body: raw,
    headers: { authorization, "content-type": "message/rfc822" },
    method: "POST",
  });
  const { blobId } = z
    .object({ blobId: z.string() })
    .parse(await upload.json());
  const receivedAt = new Date(Date.now() - minutesAgo * 60_000)
    .toISOString()
    .replace(/\.\d+Z$/u, "Z");
  const [result] = await jmap([
    [
      "Email/import",
      {
        accountId,
        emails: {
          one: {
            blobId,
            keywords: {},
            mailboxIds: { [folder]: true },
            receivedAt,
          },
        },
      },
    ],
  ]);
  const {
    created: { one },
  } = Created.parse(result);
  if (one === undefined) {
    throw new Error("The email wasn't imported");
  }
  return { id: one.id, messageId };
};

const run = async (collapseThreads: boolean): Promise<boolean> => {
  const query = {
    accountId,
    collapseThreads,
    filter: { inMailbox: folder },
    sort: [{ isAscending: false, property: "receivedAt" }],
  };
  const older = await importEmail("Thread A", 30);
  const newer = await importEmail("Thread B", 20);
  const names = new Map([
    [older.id, "A"],
    [newer.id, "B"],
  ]);
  const show = (ids: readonly string[]): string =>
    ids.map((id) => names.get(id) ?? id).join(", ");

  const [first] = await jmap([["Email/query", query]]);
  let { ids: client, queryState } = Query.parse(first);
  console.log(`\ncollapseThreads: ${String(collapseThreads)}`);
  console.log(`  start: ${show(client)}`);
  let right = true;

  /** Applies queryChanges as RFC 8620 §5.6 says and compares. */
  const check = async (label: string): Promise<void> => {
    const [changes, now] = await jmap([
      ["Email/queryChanges", { ...query, sinceQueryState: queryState }],
      ["Email/query", query],
    ]);
    const { added, newQueryState, removed } = QueryChanges.parse(changes);
    const next = client.filter((id) => !removed.includes(id));
    for (const item of added.toSorted((a, b) => a.index - b.index)) {
      next.splice(item.index, 0, item.id);
    }
    const server = Query.parse(now).ids;
    const same = next.join(",") === server.join(",");
    right &&= same;
    console.log(
      `  ${label}: removed [${show(removed)}], added [${added
        .map((item) => `${names.get(item.id) ?? item.id} at ${item.index}`)
        .join(", ")}]\n    client after applying: ${show(next)}\n` +
        `    Email/query now:       ${show(server)}  ${same ? "OK" : "WRONG"}`
    );
    client = server;
    queryState = newQueryState;
  };

  const reply = await importEmail("Re: Thread A", 10, older.messageId);
  names.set(reply.id, "reply to A");
  await check("reply to A arrives");
  await jmap([["Email/set", { accountId, destroy: [reply.id] }]]);
  await check("reply deleted");
  await jmap([["Email/set", { accountId, destroy: [older.id, newer.id] }]]);
  return right;
};

try {
  const results = [await run(false), await run(true)];
  console.log(
    results.every(Boolean)
      ? "\nAll right: the bug isn't there."
      : "\nWrong: queryChanges doesn't match Email/query (see above)."
  );
} finally {
  await jmap([
    [
      "Mailbox/set",
      { accountId, destroy: [folder], onDestroyRemoveEmails: true },
    ],
  ]);
}
