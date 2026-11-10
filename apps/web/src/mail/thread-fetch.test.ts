// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment jsdom

// getThread: the thread and its emails in one request (result reference,
// RFC 8620 §3.7), and the chunked second request when the server refuses
// that many ids at once (RFC 8620 §5.1 `requestTooLarge`).

import { EmailId } from "@nebulamail/jmap-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { methodError, scriptedJmap } from "../test/scripted-jmap";
import type { Handler } from "../test/scripted-jmap";
import { getThread } from "./api";

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

const THREAD = ["m1", "m2", "m3"];

const summary = (id: string) => ({
  from: [{ email: `${id}@example.com`, name: id }],
  hasAttachment: false,
  id,
  keywords: { $seen: true },
  mailboxIds: { inbox: true },
  preview: "",
  receivedAt: "2026-10-23T10:00:00Z",
  subject: id,
  threadId: "t1",
  to: [],
});

/** Email/get: the threadId lookup, or the members by id or reference. */
const emailGet =
  (byReference: "ok" | "tooLarge"): Handler =>
  (args, callId) => {
    const props = args.properties;
    if (Array.isArray(props) && props.length === 1) {
      return [
        [
          "Email/get",
          {
            accountId: "a",
            list: [{ id: "m2", threadId: "t1" }],
            notFound: [],
            state: "1",
          },
          callId,
        ],
      ];
    }
    if ("#ids" in args && byReference === "tooLarge") {
      return [methodError("requestTooLarge", callId)];
    }
    const ids = Array.isArray(args.ids) ? args.ids.map(String) : THREAD;
    return [
      [
        "Email/get",
        { accountId: "a", list: ids.map(summary), notFound: [], state: "1" },
        callId,
      ],
    ];
  };

const threadGet: Handler = (_args, callId) => [
  [
    "Thread/get",
    {
      accountId: "a",
      list: [{ emailIds: THREAD, id: "t1" }],
      notFound: [],
      state: "1",
    },
    callId,
  ],
];

let scripted = scriptedJmap({ handlers: {} });

describe(getThread, () => {
  beforeEach(() => {
    scripted = scriptedJmap({ handlers: {} });
    jmap.connect = scripted.connect;
  });

  it("gets the thread and its emails in one request", async () => {
    scripted.script = {
      handlers: { "Email/get": emailGet("ok"), "Thread/get": threadGet },
    };

    const thread = await getThread(EmailId.parse("m2"));

    expect(thread?.emails.map((email) => email.id)).toStrictEqual(THREAD);
    expect(thread?.total).toBe(3);
    expect(scripted.usings).toHaveLength(1);
    // The members' ids come from Thread/get, by result reference.
    expect(scripted.calls[2]?.[1]["#ids"]).toMatchObject({
      name: "Thread/get",
      path: "/list/*/emailIds",
    });
  });

  it("asks again in chunks when the server refuses that many ids", async () => {
    scripted.script = {
      handlers: { "Email/get": emailGet("tooLarge"), "Thread/get": threadGet },
    };

    const thread = await getThread(EmailId.parse("m2"));

    expect(thread?.emails.map((email) => email.id)).toStrictEqual(THREAD);
    expect(scripted.usings).toHaveLength(2);
  });
});
