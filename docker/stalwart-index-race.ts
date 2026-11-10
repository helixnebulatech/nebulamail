// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Reproduces a Stalwart bug
// right after a new email is stored, `Email/query` and
// `Email/queryChanges` may answer with the new state but without the email
// in its place yet (newest first: it should be at index 0).
//
//   bun run dev:index-race             # 20 rounds against the dev Stalwart
//   bun run dev:index-race --rounds=200
//
// Uses bob's account and a folder of its own ("Index race"), created and
// deleted here; the emails are imported (nothing is sent). Each round
// imports one email and, at once, asks in one request:
//   1. `Email/queryChanges` since the state before the import;
//   2. `Email/query` (newest first, limit 1).
// A round is wrong when the new email isn't at index 0 in either answer.
// Then it waits and asks again, to show the index catches up. The same
// rounds run without and with `collapseThreads`.
//
// Seen on an old Stalwart (2026-10-30): a few wrong rounds out of 20, more
// under load (`queryChanges` said index 80 of 81 for the newest email).
// Fixed since 1.0.1. Kept to check new versions.

import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import { z } from "zod";

const ROOT = path.join(import.meta.dirname, "..");
const STALWART_URL = "http://127.0.0.1:8080";
const USER = "bob@nebulamail.test";
const FOLDER = "Index race";
const USING = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"];
const SETTLE_MS = 2000;

const roundsArg = process.argv
  .slice(2)
  .find((arg) => arg.startsWith("--rounds="))
  ?.slice("--rounds=".length);
const ROUNDS = z.coerce
  .number()
  .int()
  .min(1)
  .max(1000)
  .parse(roundsArg ?? 20);

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

/** Runs JMAP calls; a method error comes back as `{ error: type }`. */
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
  return methodResponses.map(([name, result]) =>
    name === "error" ? { error: result.type } : result
  );
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
});
const Mailboxes = z.object({
  list: z.array(z.object({ id: z.string(), name: z.string() })),
});

const createFolder = async (): Promise<string> => {
  const [found] = await jmap([
    ["Mailbox/get", { accountId, properties: ["name"] }],
  ]);
  const existing = Mailboxes.parse(found).list.find(
    (box) => box.name === FOLDER
  );
  if (existing !== undefined) {
    return existing.id;
  }
  const [created] = await jmap([
    ["Mailbox/set", { accountId, create: { box: { name: FOLDER } } }],
  ]);
  const {
    created: { box },
  } = Created.parse(created);
  if (box === undefined) {
    throw new Error("The folder wasn't created");
  }
  return box.id;
};

const folder = await createFolder();
const imported: string[] = [];

const importEmail = async (subject: string): Promise<string> => {
  const raw = [
    "From: Index race <race@index-race.example>",
    `To: ${USER}`,
    `Subject: ${subject}`,
    `Message-ID: <${crypto.randomUUID()}@index-race.example>`,
    `Date: ${new Date().toUTCString()}`,
    "",
    "Test email for the index race check.",
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
  const [result] = await jmap([
    [
      "Email/import",
      {
        accountId,
        emails: {
          one: { blobId, keywords: {}, mailboxIds: { [folder]: true } },
        },
      },
    ],
  ]);
  const email = Created.parse(result).created.one;
  if (email === undefined) {
    throw new Error("The email wasn't imported");
  }
  imported.push(email.id);
  return email.id;
};

interface Tally {
  queryChanges: number;
  query: number;
  /** Still wrong after waiting SETTLE_MS. */
  afterWait: number;
  errors: string[];
}

const round = async (collapseThreads: boolean, tally: Tally) => {
  const query = {
    accountId,
    collapseThreads,
    filter: { inMailbox: folder },
    sort: [{ isAscending: false, property: "receivedAt" }],
  };
  const [before] = await jmap([["Email/query", { ...query, limit: 1 }]]);
  const { queryState } = Query.parse(before);
  const id = await importEmail(`Race ${imported.length + 1}`);
  const ask = async () =>
    await jmap([
      ["Email/queryChanges", { ...query, sinceQueryState: queryState }],
      ["Email/query", { ...query, limit: 1 }],
    ]);
  const [changes, now] = await ask();
  if (changes?.error === undefined) {
    const added = QueryChanges.parse(changes).added.find(
      (item) => item.id === id
    );
    if (added?.index !== 0) {
      tally.queryChanges += 1;
    }
  } else {
    tally.errors.push(JSON.stringify(changes.error));
  }
  if (Query.parse(now).ids[0] !== id) {
    tally.query += 1;
  }
  await sleep(SETTLE_MS);
  const [, later] = await ask();
  if (Query.parse(later).ids[0] !== id) {
    tally.afterWait += 1;
  }
};

try {
  for (const collapseThreads of [false, true]) {
    const tally: Tally = {
      afterWait: 0,
      errors: [],
      query: 0,
      queryChanges: 0,
    };
    for (let index = 0; index < ROUNDS; index += 1) {
      // In order: each round needs the previous one's email in place.
      // oxlint-disable-next-line no-await-in-loop
      await round(collapseThreads, tally);
    }
    const errors =
      tally.errors.length > 0
        ? `, errors: ${[...new Set(tally.errors)].join(", ")}`
        : "";
    console.log(
      `collapseThreads=${String(collapseThreads)}: of ${ROUNDS} rounds, queryChanges wrong ${tally.queryChanges}, Email/query wrong ${tally.query}, still wrong after ${SETTLE_MS} ms ${tally.afterWait}${errors}`
    );
  }
} finally {
  await jmap([
    ["Email/set", { accountId, destroy: imported }],
    ["Mailbox/set", { accountId, destroy: [folder] }],
  ]);
}
