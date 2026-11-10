// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// queryEmails: the list page and the other emails of its threads in one
// request (result reference, RFC 8620 §3.7, as in RFC 8621 §4.10), and the
// second request of before when the server refuses that many ids at once
// (RFC 8620 §5.1 `requestTooLarge`). The rows are the same either way.

import { MailboxId } from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { methodError, scriptedJmap } from "../test/scripted-jmap";
import type { Handler } from "../test/scripted-jmap";
import { listEmails } from "./api";

const jmap = vi.hoisted(() => ({
  connect: null as null | (() => Promise<unknown>),
}));

vi.mock(import("./client"), async () => {
  const { JmapClient } = await import("@nebulamail/jmap-client");
  return {
    getClient: async () => {
      const client = await jmap.connect?.();
      if (!(client instanceof JmapClient)) {
        throw new TypeError("no scripted client");
      }
      return client;
    },
    resetClient: vi.fn<() => void>(),
  };
});

/** Two rows: t1 has three emails (m3 is the newest, the row), t2 one. */
const THREADS: Record<string, string[]> = {
  t1: ["m1", "m2", "m3"],
  t2: ["n1"],
};
const PAGE = ["m3", "n1"];

const email = (id: string) => ({
  from: [{ email: `${id}@example.com`, name: id }],
  hasAttachment: false,
  id,
  // m1 is the only unread one: the t1 row must know it.
  keywords: id === "m1" ? {} : { $seen: true },
  mailboxIds: { inbox: true },
  preview: "",
  receivedAt: `2026-10-23T10:0${id.at(-1) ?? "0"}:00Z`,
  subject: id,
  threadId: id.startsWith("m") ? "t1" : "t2",
  to: [],
});

const referenced = (args: Record<string, unknown>): string | null => {
  const ref = args["#ids"];
  return typeof ref === "object" && ref !== null && "name" in ref
    ? String(ref.name)
    : null;
};

const emailGet =
  (members: "ok" | "tooLarge"): Handler =>
  (args, callId) => {
    const byThreads = referenced(args) === "Thread/get";
    if (byThreads && members === "tooLarge") {
      return [methodError("requestTooLarge", callId)];
    }
    let ids: string[] = PAGE;
    if (byThreads) {
      ids = Object.values(THREADS).flat();
    } else if (Array.isArray(args.ids)) {
      ids = args.ids.map(String);
    }
    return [
      [
        "Email/get",
        { accountId: "a", list: ids.map(email), notFound: [], state: "1" },
        callId,
      ],
    ];
  };

const handlers = (members: "ok" | "tooLarge") => ({
  "Email/get": emailGet(members),
  "Email/query": ((_args, callId) => [
    [
      "Email/query",
      {
        accountId: "a",
        canCalculateChanges: true,
        ids: PAGE,
        position: 0,
        queryState: "q1",
        total: 2,
      },
      callId,
    ],
  ]) satisfies Handler,
  "Thread/get": ((_args, callId) => [
    [
      "Thread/get",
      {
        accountId: "a",
        list: Object.entries(THREADS).map(([id, emailIds]) => ({
          emailIds,
          id,
        })),
        notFound: [],
        state: "1",
      },
      callId,
    ],
  ]) satisfies Handler,
});

const rowsOf = async () => {
  const page = await listEmails(MailboxId.parse("inbox"));
  return page.emails.map((row) => ({
    id: row.id,
    members: row.members.map((member) => [member.id, member.unread]),
  }));
};

const EXPECTED = [
  {
    id: "m3",
    members: [
      ["m1", true],
      ["m2", false],
      ["m3", false],
    ],
  },
  { id: "n1", members: [["n1", false]] },
];

let scripted = scriptedJmap({ handlers: {} });

describe(listEmails, () => {
  beforeEach(() => {
    scripted = scriptedJmap({ handlers: {} });
    jmap.connect = scripted.connect;
  });

  it("gets the page and its threads' emails in one request", async () => {
    scripted.script = { handlers: handlers("ok") };

    await expect(rowsOf()).resolves.toStrictEqual(EXPECTED);
    expect(scripted.usings).toHaveLength(1);
    expect(scripted.calls[3]?.[1]["#ids"]).toMatchObject({
      name: "Thread/get",
      path: "/list/*/emailIds",
    });
  });

  it("asks for the members apart when the server refuses that many", async () => {
    scripted.script = { handlers: handlers("tooLarge") };

    await expect(rowsOf()).resolves.toStrictEqual(EXPECTED);
    expect(scripted.usings).toHaveLength(2);
    // Only the emails the page didn't have, as before.
    expect(scripted.calls.at(-1)?.[1].ids).toStrictEqual(["m1", "m2"]);
  });
});
