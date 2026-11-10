// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { setTimeout } from "node:timers/promises";

import { describe, expect, expectTypeOf, it, vi } from "vitest";

import { JmapClient } from "./client";
import { JmapError } from "./errors";
import { AccountId, EmailId, IdentityId, MailboxId } from "./ids";
import type { ThreadId } from "./ids";
import { keywordPatch, mailboxPatch } from "./patch";
import { RequestBuilder } from "./request";
import type { ResultReference, Settled } from "./request";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  rejection,
  sessionThen,
  stalwartSession,
  tokens,
} from "./test-support";
import type { Handler } from "./test-support";

const accountId = AccountId.parse("d");
const inbox = MailboxId.parse("a");

const connect = async (api: Handler, session = stalwartSession()) => {
  const fake = fakeFetch(sessionThen(api, session));
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...tokens(),
  });
  return { calls: fake.calls, client };
};

const queryResponse = {
  accountId: "d",
  canCalculateChanges: true,
  ids: ["m1", "m2"],
  position: 0,
  queryState: "q1",
};

const getResponse = {
  accountId: "d",
  list: [
    {
      from: [{ email: "ana@example.com", name: "Ana" }],
      id: "m1",
      receivedAt: "2026-10-23T08:00:00Z",
      subject: "Hola",
      threadId: "t1",
    },
    {
      from: null,
      id: "m2",
      receivedAt: "2026-10-22T08:00:00Z",
      subject: null,
      threadId: "t2",
      "x-vendor": 1,
    },
  ],
  notFound: [],
  state: "e1",
};

describe("request batching", () => {
  it("sends query + get in one request with a back-reference", async () => {
    const { calls, client } = await connect(() =>
      methodResponses([
        ["Email/query", queryResponse, "c0"],
        ["Email/get", getResponse, "c1"],
      ])
    );
    const [query, emails] = await client.request((b) => {
      const q = b.call("Email/query", {
        accountId,
        collapseThreads: true,
        filter: {
          conditions: [{ inMailbox: inbox }, { hasKeyword: "$flagged" }],
          operator: "AND",
        },
        limit: 50,
        sort: [{ isAscending: false, property: "receivedAt" }],
      });
      const g = b.call("Email/get", {
        "#ids": q.ref("/ids"),
        accountId,
        properties: ["threadId", "subject", "from", "receivedAt"],
      });
      return [q, g] as const;
    });
    expect(calls[1]?.body).toStrictEqual({
      methodCalls: [
        [
          "Email/query",
          expect.objectContaining({ collapseThreads: true, limit: 50 }),
          "c0",
        ],
        [
          "Email/get",
          {
            "#ids": { name: "Email/query", path: "/ids", resultOf: "c0" },
            accountId: "d",
            properties: ["threadId", "subject", "from", "receivedAt"],
          },
          "c1",
        ],
      ],
      using: ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
    });
    expect(query.ids).toStrictEqual(["m1", "m2"]);
    expect(emails.list.map((email) => email.subject)).toStrictEqual([
      "Hola",
      null,
    ]);
  });

  it("narrows Email/get items to the requested properties", async () => {
    const { client } = await connect(() =>
      methodResponses([["Email/get", getResponse, "c0"]])
    );
    const emails = await client.request((b) =>
      b.call("Email/get", {
        accountId,
        ids: [EmailId.parse("m1")],
        properties: ["threadId", "subject"],
      })
    );
    const [first] = emails.list;
    expectTypeOf(first).toExtend<
      { id: EmailId; subject: string | null; threadId: ThreadId } | undefined
    >();
    expectTypeOf<NonNullable<typeof first>>().not.toHaveProperty("htmlBody");
    expect(first?.threadId).toBe("t1");
  });

  it("fills in a preview the server left out (Stalwart, empty body)", async () => {
    const { client } = await connect(() =>
      methodResponses([["Email/get", getResponse, "c0"]])
    );
    const emails = await client.request((b) =>
      b.call("Email/get", {
        accountId,
        ids: [EmailId.parse("m1")],
        properties: ["preview", "subject"],
      })
    );

    expect(emails.list.map((email) => email.preview)).toStrictEqual(["", ""]);
  });

  it("still rejects other requested properties that are missing", async () => {
    const { client } = await connect(() =>
      methodResponses([["Email/get", getResponse, "c0"]])
    );

    await expect(
      client.request((b) =>
        b.call("Email/get", {
          accountId,
          ids: [EmailId.parse("m1")],
          properties: ["size"],
        })
      )
    ).rejects.toThrow("invalid server response");
  });

  it("rejects invalid paths, wrong id types and unknown arguments at compile time", () => {
    const b = new RequestBuilder();
    const q = b.call("Email/query", { accountId });
    // @ts-expect-error -- "/idz" is not a path of an Email/query response.
    q.ref("/idz");
    // @ts-expect-error -- EmailIds can't be used as Thread ids.
    b.call("Thread/get", { "#ids": q.ref("/ids"), accountId });
    // @ts-expect-error -- `limt` is not an Email/get argument.
    b.call("Email/get", { "#ids": q.ref("/ids"), accountId, limt: 1 });
    expect(() =>
      // @ts-expect-error -- `ids` and `#ids` are mutually exclusive.
      b.call("Email/get", { "#ids": q.ref("/ids"), accountId, ids: [] })
    ).toThrow(/both "ids" and "#ids"/u);
    const threads = b.call("Email/get", {
      "#ids": q.ref("/ids"),
      accountId,
      properties: ["threadId"],
    });
    expectTypeOf(threads.ref("/list/*/threadId")).toEqualTypeOf<
      ResultReference<ThreadId[]>
    >();
    expect(b.chunks(16)[0]?.request.methodCalls).toHaveLength(4);
  });

  it("types Email/set drafts, updates and destroys", () => {
    const b = new RequestBuilder();
    const message = EmailId.parse("m1");
    const set = b.call("Email/set", {
      accountId,
      create: {
        draft: {
          bodyValues: { body: { value: "Hola" } },
          from: [{ email: "jane@example.com", name: "Jane" }],
          keywords: { $draft: true, $seen: true },
          mailboxIds: { [inbox]: true },
          subject: "Hola",
          textBody: [{ partId: "body", type: "text/plain" }],
          to: [{ email: "ana@example.com" }],
        },
      },
      destroy: [EmailId.parse("m2")],
      update: {
        [message]: { ...keywordPatch({ $seen: true }), "mailboxIds/a": null },
      },
    });
    expectTypeOf(set.ref("/created/draft/id")).toEqualTypeOf<
      ResultReference<EmailId>
    >();
    expect(b.chunks(16)[0]?.request.using).toContain(
      "urn:ietf:params:jmap:mail"
    );
  });

  it("supports object shapes and settled results", async () => {
    const { client } = await connect(() =>
      methodResponses([
        ["Email/query", queryResponse, "c0"],
        ["error", { type: "unsupportedFilter" }, "c1"],
      ])
    );
    const results = await client.requestSettled((b) => ({
      first: b.call("Email/query", { accountId, filter: { text: "x" } }),
      second: b.call("Email/query", { accountId }),
    }));
    expect(results.first.ok).toBeTruthy();
    expect(results.second).toMatchObject({
      error: { detail: { kind: "method", type: "unsupportedFilter" } },
      ok: false,
    });
    expectTypeOf(results.second).toExtend<Settled<unknown>>();
  });

  it("throws method errors as typed JmapErrors", async () => {
    const { client } = await connect(() =>
      methodResponses([
        ["error", { description: "nope", type: "invalidArguments" }, "c0"],
      ])
    );
    const error = await rejection(
      client.request((b) => b.call("Mailbox/get", { accountId }))
    );
    expect(error.detail).toStrictEqual({
      arguments: { description: "nope", type: "invalidArguments" },
      callId: "c0",
      description: "nope",
      kind: "method",
      method: "Mailbox/get",
      type: "invalidArguments",
    });
  });

  it("turns request-level problem documents into typed errors", async () => {
    const { client } = await connect(() =>
      Response.json(
        {
          limit: "maxSizeRequest",
          status: 400,
          type: "urn:ietf:params:jmap:error:limit",
        },
        { status: 400 }
      )
    );
    const error = await rejection(
      client.request((b) => b.call("Mailbox/get", { accountId }))
    );
    expect(error.detail).toMatchObject({
      kind: "request",
      limit: "maxSizeRequest",
      type: "urn:ietf:params:jmap:error:limit",
    });
  });

  it("fails when the server omits a requested property", async () => {
    const { client } = await connect(() =>
      methodResponses([
        [
          "Email/get",
          { ...getResponse, list: [{ id: "m1", threadId: "t1" }] },
          "c0",
        ],
      ])
    );
    const error = await rejection(
      client.request((b) =>
        b.call("Email/get", { accountId, properties: ["subject"] })
      )
    );
    expect(error.detail).toStrictEqual({
      issues: ["list.0.subject: missing"],
      kind: "invalidResponse",
    });
  });

  it("splits into several HTTP requests above maxCallsInRequest", async () => {
    const { calls, client } = await connect(
      (_call, index) =>
        methodResponses([
          ["Email/query", queryResponse, `c${String(index - 1)}`],
        ]),
      stalwartSession({}, { maxCallsInRequest: 1 })
    );
    await client.request((b) => [
      b.call("Email/query", { accountId }),
      b.call("Email/query", { accountId }),
    ]);
    expect(calls).toHaveLength(3);
    const crossing = await rejection(
      client.request((b) => {
        const q = b.call("Email/query", { accountId });
        return b.call("Email/get", { "#ids": q.ref("/ids"), accountId });
      })
    );
    expect(crossing.detail).toMatchObject({
      kind: "limit",
      limit: "maxCallsInRequest",
    });
  });

  it("rejects references from another request", async () => {
    const { client } = await connect(() =>
      methodResponses([["Email/query", queryResponse, "c0"]])
    );
    const [, leaked] = await client.request((b) => {
      const q = b.call("Email/query", { accountId });
      return [q, q.ref("/ids")] as const;
    });
    const error = await rejection(
      client.request((b) => b.call("Email/get", { "#ids": leaked, accountId }))
    );
    expect(error.detail.kind).toBe("usage");
  });

  it("refreshes the session after sessionState changes", async () => {
    const { calls, client } = await connect(() =>
      methodResponses([["Email/query", queryResponse, "c0"]], "s2")
    );
    await client.request((b) => b.call("Email/query", { accountId }));
    await client.request((b) => b.call("Email/query", { accountId }));
    expect(calls.map((call) => call.url)).toStrictEqual([
      SESSION_URL,
      "https://mail.example.com/jmap/",
      SESSION_URL,
      "https://mail.example.com/jmap/",
    ]);
  });

  it("tells when the request goes out (onSend), never for a failed session refresh", async () => {
    let sessions = 0;
    const fake = fakeFetch(async (call) => {
      if (call.url === SESSION_URL) {
        sessions += 1;
        if (sessions > 1) {
          throw new TypeError("Failed to fetch");
        }
        return await Promise.resolve(Response.json(stalwartSession()));
      }
      // A new sessionState: the next request refreshes the session first.
      return methodResponses([["Email/query", queryResponse, "c0"]], "s2");
    });
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    const sends: string[] = [];

    await client.request((b) => b.call("Email/query", { accountId }), {
      onSend: () => {
        sends.push("first");
      },
    });
    const error: unknown = await client
      .request((b) => b.call("Email/query", { accountId }), {
        onSend: () => {
          sends.push("second");
        },
      })
      .catch((error_: unknown) => error_);

    expect(error).toBeInstanceOf(JmapError);
    expect(sends).toStrictEqual(["first"]);
  });

  it("exposes the implicit Email/set of EmailSubmission/set", async () => {
    const setBase = { accountId: "d", newState: "n", oldState: "o" };
    const { calls, client } = await connect(() =>
      methodResponses([
        [
          "EmailSubmission/set",
          { ...setBase, created: { send: { id: "s1" } } },
          "c0",
        ],
        ["Email/set", { ...setBase, updated: { m1: null } }, "c0"],
      ])
    );
    const sent = await client.request((b) =>
      b.call("EmailSubmission/set", {
        accountId,
        create: {
          send: { emailId: "#draft", identityId: IdentityId.parse("i1") },
        },
        onSuccessUpdateEmail: {
          "#send": {
            ...mailboxPatch({
              add: [MailboxId.parse("sent")],
              remove: [inbox],
            }),
            ...keywordPatch({ $draft: false, $seen: true }),
          },
        },
      })
    );
    expect(sent.emailSet?.updated).toStrictEqual({ m1: null });
    expect(calls[1]?.body).toMatchObject({
      methodCalls: [
        [
          "EmailSubmission/set",
          {
            onSuccessUpdateEmail: {
              "#send": {
                "keywords/$draft": null,
                "keywords/$seen": true,
                "mailboxIds/a": null,
                "mailboxIds/sent": true,
              },
            },
          },
          "c0",
        ],
      ],
      using: [
        "urn:ietf:params:jmap:core",
        "urn:ietf:params:jmap:mail",
        "urn:ietf:params:jmap:submission",
      ],
    });
  });

  it("never fails a send because of its implicit Email/set", async () => {
    const setBase = { accountId: "d", newState: "n", oldState: "o" };
    const submission = [
      "EmailSubmission/set",
      { ...setBase, created: { send: { id: "s1" } } },
      "c0",
    ] as const;
    const implicits = [
      [
        "Email/set",
        { ...setBase, notUpdated: { m1: { type: "serverFail" } } },
        "c0",
      ],
      ["error", { type: "serverFail" }, "c0"],
      ["Email/set", { updated: "nope" }, "c0"],
    ] as const;
    const results = await Promise.all(
      implicits.map(async (implicit) => {
        const { client } = await connect(() =>
          methodResponses([[...submission], [...implicit]])
        );
        const sent = await client.request((b) =>
          b.call("EmailSubmission/set", {
            accountId,
            create: {
              send: { emailId: "#draft", identityId: IdentityId.parse("i1") },
            },
          })
        );
        return {
          created: Object.keys(sent.created),
          emailSetFailed: sent.emailSetFailed ?? false,
          notUpdated: Object.keys(sent.emailSet?.notUpdated ?? {}),
        };
      })
    );

    expect(results).toStrictEqual([
      { created: ["send"], emailSetFailed: false, notUpdated: ["m1"] },
      { created: ["send"], emailSetFailed: true, notUpdated: [] },
      { created: ["send"], emailSetFailed: true, notUpdated: [] },
    ]);
  });

  it("sends MDN/send with its capability and reads sent, notSent and the implicit Email/set", async () => {
    const setBase = { accountId: "d", newState: "n", oldState: "o" };
    const { calls, client } = await connect(() =>
      methodResponses([
        [
          "MDN/send",
          {
            accountId: "d",
            notSent: { other: { type: "mdnAlreadySent" } },
            sent: { k1: { finalRecipient: "rfc822;jane@example.com" } },
          },
          "c0",
        ],
        ["Email/set", { ...setBase, updated: { m1: null } }, "c0"],
      ])
    );
    const result = await client.request((b) =>
      b.call("MDN/send", {
        accountId,
        identityId: IdentityId.parse("i1"),
        onSuccessUpdateEmail: { "#k1": keywordPatch({ $mdnsent: true }) },
        send: {
          k1: {
            disposition: {
              actionMode: "manual-action",
              sendingMode: "mdn-sent-manually",
              type: "displayed",
            },
            forEmailId: EmailId.parse("m1"),
          },
        },
      })
    );
    expect(result.sent.k1).toStrictEqual({
      finalRecipient: "rfc822;jane@example.com",
    });
    expect(result.notSent.other?.type).toBe("mdnAlreadySent");
    expect(result.emailSet?.updated).toStrictEqual({ m1: null });
    expect(calls[1]?.body).toMatchObject({
      using: [
        "urn:ietf:params:jmap:core",
        "urn:ietf:params:jmap:mail",
        "urn:ietf:params:jmap:mdn",
      ],
    });
  });

  it("queries submissions by email id", async () => {
    const { calls, client } = await connect(() =>
      methodResponses([
        ["EmailSubmission/query", { ...queryResponse, ids: ["s1"] }, "c0"],
      ])
    );
    const result = await client.request((b) =>
      b.call("EmailSubmission/query", {
        accountId,
        filter: { emailIds: [EmailId.parse("m1")] },
      })
    );
    expect(result.ids).toStrictEqual(["s1"]);
    expect(calls[1]?.body).toMatchObject({
      using: [
        "urn:ietf:params:jmap:core",
        "urn:ietf:params:jmap:mail",
        "urn:ietf:params:jmap:submission",
      ],
    });
  });
});

const ok = (): Response =>
  methodResponses([
    [
      "Mailbox/get",
      { accountId: "d", list: [], notFound: [], state: "s" },
      "c0",
    ],
  ]);
const ask = async (client: JmapClient) =>
  await client.request((b) => b.call("Mailbox/get", { accountId, ids: [] }));

describe("maxConcurrentRequests and a busy server", () => {
  it("never has more requests out than the session allows", async () => {
    let out = 0;
    let most = 0;
    const { client } = await connect(async () => {
      out += 1;
      most = Math.max(most, out);
      await setTimeout(5);
      out -= 1;
      return ok();
    });
    await Promise.all(
      Array.from({ length: 12 }, async () => await ask(client))
    );
    expect(most).toBe(4);
  });

  it("tries a 429 again: the server did nothing", async () => {
    let tries = 0;
    const { client } = await connect(() => {
      tries += 1;
      return tries === 1 ? new Response("{}", { status: 429 }) : ok();
    });
    await expect(ask(client)).resolves.toBeDefined();
    expect(tries).toBe(2);
  });

  it("stops waiting for a slot when aborted", async () => {
    const gate = Promise.withResolvers<undefined>();
    let started = 0;
    const { client } = await connect(async () => {
      started += 1;
      await gate.promise;
      return ok();
    });
    const busy = Array.from({ length: 4 }, async () => await ask(client));
    const controller = new AbortController();
    const waiting = client.request(
      (b) => b.call("Mailbox/get", { accountId, ids: [] }),
      { signal: controller.signal }
    );
    controller.abort();
    await expect(waiting).rejects.toBeDefined();
    await vi.waitFor(() => {
      expect(started).toBe(4);
    });
    gate.resolve(undefined);
    await Promise.allSettled(busy);
  });

  it("tells when a refreshed session changed", async () => {
    let state = "s1";
    const fake = fakeFetch((call) =>
      call.url === SESSION_URL
        ? Response.json({ ...stalwartSession(), state })
        : ok()
    );
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    const seen: string[] = [];
    const stop = client.onSessionChange((session) => {
      seen.push(session.state);
    });
    await client.refreshSession();
    expect(seen).toStrictEqual([]);
    state = "s2";
    await client.refreshSession();
    expect(seen).toStrictEqual(["s2"]);
    stop();
    state = "s3";
    await client.refreshSession();
    expect(seen).toStrictEqual(["s2"]);
  });
});
