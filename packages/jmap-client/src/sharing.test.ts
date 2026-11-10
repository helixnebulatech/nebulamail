// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// Quota (RFC 9425), Principal (RFC 9670) and Mailbox.shareWith, with the
// shapes Stalwart answers (checked live).

import { describe, expect, it } from "vitest";
import { z } from "zod";

import { CAPABILITIES } from "./capabilities";
import { JmapClient } from "./client";
import { AccountId, BlobId, MailboxId, PrincipalId } from "./ids";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  sessionThen,
  tokens,
} from "./test-support";

const accountId = AccountId.parse("d");

const connect = async (responses: Parameters<typeof methodResponses>[0]) => {
  const fake = fakeFetch(sessionThen(() => methodResponses(responses)));
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...tokens(),
  });
  return { client, fake };
};

const RequestBody = z.object({
  methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
  using: z.array(z.string()),
});

const RIGHTS = {
  mayAddItems: false,
  mayCreateChild: false,
  mayDelete: false,
  mayReadItems: true,
  mayRemoveItems: false,
  mayRename: false,
  maySetKeywords: false,
  maySetSeen: false,
  mayShare: false,
  maySubmit: false,
};

describe("Quota/get", () => {
  it("reads the account quota", async () => {
    const { client, fake } = await connect([
      [
        "Quota/get",
        {
          accountId: "d",
          list: [
            {
              description: "Test account",
              hardLimit: 104_857_600,
              id: "a",
              name: "alice@nebulamail.test",
              resourceType: "octets",
              scope: "account",
              softLimit: null,
              types: ["Email", "ContactCard"],
              used: 5_429_839,
              warnLimit: null,
            },
          ],
          notFound: [],
          state: "n",
        },
        "c0",
      ],
    ]);
    const [quotas] = await client.request((b) => [
      b.call("Quota/get", { accountId, ids: null }),
    ]);
    expect(quotas.list[0]?.hardLimit).toBe(104_857_600);
    expect(RequestBody.parse(fake.calls[1]?.body).using).toContain(
      CAPABILITIES.quota
    );
    // RFC 9425 §4.1: without mail in `using`, no `Email` type is shown.
    expect(RequestBody.parse(fake.calls[1]?.body).using).toContain(
      CAPABILITIES.mail
    );
  });

  it("fills in the optional limits other servers leave out", async () => {
    const { client } = await connect([
      [
        "Quota/get",
        {
          accountId: "d",
          list: [
            {
              hardLimit: 10,
              id: "a",
              name: "q",
              resourceType: "count",
              scope: "account",
              types: ["Email"],
              used: 1,
            },
          ],
          notFound: [],
          state: "n",
        },
        "c0",
      ],
    ]);
    const [quotas] = await client.request((b) => [
      b.call("Quota/get", { accountId, ids: null }),
    ]);
    expect(quotas.list[0]?.warnLimit).toBeNull();
  });
});

describe("Principal", () => {
  it("queries and gets principals", async () => {
    const { client, fake } = await connect([
      [
        "Principal/query",
        {
          accountId: "d",
          canCalculateChanges: true,
          ids: ["d"],
          position: 0,
          queryState: "n",
        },
        "c0",
      ],
      [
        "Principal/get",
        {
          accountId: "d",
          list: [
            {
              email: "bob@nebulamail.test",
              id: "d",
              name: "bob@nebulamail.test",
              type: "individual",
            },
          ],
          notFound: [],
          state: "n",
        },
        "c1",
      ],
    ]);
    const [, principals] = await client.request((b) => {
      const query = b.call("Principal/query", {
        accountId,
        filter: { email: "bob@nebulamail.test" },
      });
      const get = b.call("Principal/get", {
        "#ids": query.ref("/ids"),
        accountId,
      });
      return [query, get] as const;
    });
    expect(principals.list[0]).toMatchObject({
      description: null,
      email: "bob@nebulamail.test",
    });
    expect(RequestBody.parse(fake.calls[1]?.body).using).toContain(
      CAPABILITIES.principals
    );
  });
});

describe("Mailbox.shareWith", () => {
  it("adds and removes one principal with a patch path", async () => {
    const { client, fake } = await connect([
      [
        "Mailbox/set",
        {
          accountId: "d",
          newState: "s2",
          oldState: "s1",
          updated: { k: null },
        },
        "c0",
      ],
    ]);
    const box = MailboxId.parse("k");
    const bob = PrincipalId.parse("d");
    const eve = PrincipalId.parse("e");
    await client.request((b) => {
      b.using(CAPABILITIES.mailShare);
      return [
        b.call("Mailbox/set", {
          accountId,
          update: {
            [box]: { [`shareWith/${bob}`]: RIGHTS, [`shareWith/${eve}`]: null },
          },
        }),
      ];
    });
    const body = RequestBody.parse(fake.calls[1]?.body);
    expect(body.using).toContain(CAPABILITIES.mailShare);
    expect(body.methodCalls[0]?.[1]).toMatchObject({
      update: { k: { "shareWith/d": RIGHTS, "shareWith/e": null } },
    });
  });

  it("reads shareWith, and null when a server leaves it out", async () => {
    const { client } = await connect([
      [
        "Mailbox/get",
        {
          accountId: "d",
          list: [
            { id: "k", name: "Shared", shareWith: { d: RIGHTS } },
            { id: "l", name: "Other server" },
          ],
          notFound: [],
          state: "s",
        },
        "c0",
      ],
    ]);
    const [boxes] = await client.request((b) => [
      b.call("Mailbox/get", { accountId, properties: ["name", "shareWith"] }),
    ]);
    expect(
      boxes.list[0]?.shareWith?.[PrincipalId.parse("d")]?.mayReadItems
    ).toBeTruthy();
    expect(boxes.list[1]?.shareWith).toBeNull();
  });
});

describe("Blob/copy", () => {
  it("copies blobs of a shared account into ours (RFC 8620 §6.3)", async () => {
    const { client, fake } = await connect([
      [
        "Blob/copy",
        {
          accountId: "d",
          copied: { Gb1: "Gn1" },
          fromAccountId: "c",
          notCopied: { Gb2: { type: "notFound" } },
        },
        "c0",
      ],
    ]);
    const [result] = await client.request((b) => [
      b.call("Blob/copy", {
        accountId,
        blobIds: [BlobId.parse("Gb1"), BlobId.parse("Gb2")],
        fromAccountId: AccountId.parse("c"),
      }),
    ]);
    expect(result.copied).toStrictEqual({ Gb1: "Gn1" });
    expect(result.notCopied.Gb2?.type).toBe("notFound");
    const body = RequestBody.parse(fake.calls[1]?.body);
    expect(body.using).toContain(CAPABILITIES.core);
    expect(body.methodCalls[0]?.[0]).toBe("Blob/copy");
  });

  it("reads null maps as empty", async () => {
    const { client } = await connect([
      [
        "Blob/copy",
        { accountId: "d", copied: null, fromAccountId: "c", notCopied: null },
        "c0",
      ],
    ]);
    const [result] = await client.request((b) => [
      b.call("Blob/copy", {
        accountId,
        blobIds: [],
        fromAccountId: AccountId.parse("c"),
      }),
    ]);
    expect(result.copied).toStrictEqual({});
    expect(result.notCopied).toStrictEqual({});
  });
});
