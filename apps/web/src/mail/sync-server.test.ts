// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// `jmapSyncServer.delta` with a real client on a fake JMAP endpoint: what
// it asks and how it reads the answers (Email/queryChanges, RFC 8620 §5.6,
// and Email/query when that can't be used).

import type { EmailFilter } from "@nebulamail/jmap-client";
import {
  AccountId,
  EmailId,
  JmapClient,
  MailboxId,
  State,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";
import { z } from "zod";

import { session } from "../../e2e/fake-jmap";
import type { Baseline, ListWindow } from "./sync";
import { jmapSyncServer } from "./sync";

const RequestSchema = z.object({
  methodCalls: z.array(
    z.tuple([z.string(), z.record(z.string(), z.unknown()), z.string()])
  ),
});
type Handler = (args: Record<string, unknown>) => readonly [string, unknown];

const ACCOUNT = AccountId.parse("a");
const FILTER: EmailFilter = { inMailbox: MailboxId.parse("inbox") };
const SINCE: Baseline = { email: State.parse("e1"), thread: State.parse("t1") };

const ids = (...values: string[]): EmailId[] =>
  values.map((value) => EmailId.parse(value));

const changes =
  (newState: string): Handler =>
  () => [
    "changes",
    {
      accountId: ACCOUNT,
      created: [],
      destroyed: [],
      hasMoreChanges: false,
      newState,
      oldState: "x",
      updated: [],
    },
  ];

const BASE: Record<string, Handler> = {
  "Email/changes": changes("e2"),
  "Thread/changes": changes("t2"),
};

/** A client whose API answers with these handlers; it records each batch. */
const connect = async (
  handlers: Record<string, Handler>
): Promise<{ client: JmapClient; batches: string[][] }> => {
  const batches: string[][] = [];
  const client = await JmapClient.connect({
    fetch: async (_input, init) => {
      if (init?.method !== "POST") {
        return await Promise.resolve(Response.json(session()));
      }
      const body = RequestSchema.parse(
        JSON.parse(typeof init.body === "string" ? init.body : "{}")
      );
      batches.push(body.methodCalls.map(([name]) => name));
      return await Promise.resolve(
        Response.json({
          methodResponses: body.methodCalls.map(([name, args, id]) => {
            const handler = handlers[name];
            if (handler === undefined) {
              return ["error", { type: "unknownMethod" }, id];
            }
            const [kind, result] = handler(args);
            return [kind === "changes" ? name : kind, result, id];
          }),
          sessionState: "s",
        })
      );
    },
    getAccessToken: async () => await Promise.resolve("token"),
    sessionUrl: "http://jmap.test/.well-known/jmap",
  });
  return { batches, client };
};

const window = (queryState: string | null): ListWindow => ({
  filter: FILTER,
  ids: ids("e2", "e1"),
  limit: 50,
  queryState: queryState === null ? null : State.parse(queryState),
});

const queryAnswer: Handler = () => [
  "Email/query",
  {
    accountId: ACCOUNT,
    canCalculateChanges: true,
    ids: ["e9", "e2"],
    position: 0,
    queryState: "q9",
    total: 7,
  },
];

describe("jmapSyncServer.delta", () => {
  it("applies Email/queryChanges to the rows the list shows", async () => {
    const asked: Record<string, unknown>[] = [];
    const { batches, client } = await connect({
      ...BASE,
      "Email/queryChanges": (args) => {
        asked.push(args);
        return [
          "Email/queryChanges",
          {
            accountId: ACCOUNT,
            added: [{ id: "e3", index: 0 }],
            newQueryState: "q2",
            oldQueryState: "q1",
            removed: ["e1"],
            total: 2,
          },
        ];
      },
    });

    const delta = await jmapSyncServer(client).delta(ACCOUNT, SINCE, [
      window("q1"),
    ]);

    expect(delta?.windows).toStrictEqual([
      { ids: ids("e3", "e2"), queryState: State.parse("q2"), total: 2 },
    ]);
    expect(delta?.baseline).toStrictEqual({
      email: State.parse("e2"),
      thread: State.parse("t2"),
    });
    expect(asked[0]).toMatchObject({
      calculateTotal: true,
      collapseThreads: true,
      filter: FILTER,
      sinceQueryState: "q1",
    });
    // One request: no Email/query.
    expect(batches).toStrictEqual([
      ["Email/changes", "Thread/changes", "Email/queryChanges"],
    ]);
  });

  it("asks the ids again when the server can't work the changes out", async () => {
    const { batches, client } = await connect({
      ...BASE,
      "Email/query": queryAnswer,
      "Email/queryChanges": () => ["error", { type: "cannotCalculateChanges" }],
    });

    const delta = await jmapSyncServer(client).delta(ACCOUNT, SINCE, [
      window("q1"),
    ]);

    expect(delta?.windows).toStrictEqual([
      { ids: ids("e9", "e2"), queryState: State.parse("q9"), total: 7 },
    ]);
    expect(batches).toStrictEqual([
      ["Email/changes", "Thread/changes", "Email/queryChanges"],
      ["Email/query"],
    ]);
  });

  it("asks the ids of a list without a query state, and keeps the order", async () => {
    const { batches, client } = await connect({
      ...BASE,
      "Email/query": queryAnswer,
      "Email/queryChanges": () => [
        "Email/queryChanges",
        {
          accountId: ACCOUNT,
          added: [],
          newQueryState: "q2",
          oldQueryState: "q1",
          removed: [],
          total: 2,
        },
      ],
    });

    const delta = await jmapSyncServer(client).delta(ACCOUNT, SINCE, [
      window(null),
      window("q1"),
    ]);

    expect(delta?.windows).toStrictEqual([
      { ids: ids("e9", "e2"), queryState: State.parse("q9"), total: 7 },
      { ids: ids("e2", "e1"), queryState: State.parse("q2"), total: 2 },
    ]);
    expect(batches).toStrictEqual([
      ["Email/changes", "Thread/changes", "Email/queryChanges"],
      ["Email/query"],
    ]);
  });

  it("doesn't trust an answer from another state, or without the size", async () => {
    const { client } = await connect({
      ...BASE,
      "Email/query": queryAnswer,
      "Email/queryChanges": (args) => [
        "Email/queryChanges",
        {
          accountId: ACCOUNT,
          added: [],
          newQueryState: "q2",
          oldQueryState: args.sinceQueryState === "q1" ? "q0" : "q5",
          removed: [],
          ...(args.sinceQueryState === "q1" ? { total: 2 } : {}),
        },
      ],
    });

    const delta = await jmapSyncServer(client).delta(ACCOUNT, SINCE, [
      window("q1"),
      window("q5"),
    ]);

    expect(delta?.windows.map((item) => item?.queryState)).toStrictEqual([
      State.parse("q9"),
      State.parse("q9"),
    ]);
  });

  it("starts over when Email/changes is refused", async () => {
    const { client } = await connect({
      ...BASE,
      "Email/changes": () => ["error", { type: "cannotCalculateChanges" }],
    });

    await expect(
      jmapSyncServer(client).delta(ACCOUNT, SINCE, [window("q1")])
    ).resolves.toBeNull();
  });
});
