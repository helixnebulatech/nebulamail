// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, expectTypeOf, it } from "vitest";
import { z } from "zod";

import { JmapClient } from "./client";
import { AccountId, AddressBookId, BlobId, ContactCardId } from "./ids";
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

const usingOf = (body: unknown): string[] =>
  z.object({ using: z.array(z.string()) }).parse(body).using;

describe("ContactCard", () => {
  it("queries and gets cards with the contacts capability", async () => {
    const { client, fake } = await connect([
      [
        "ContactCard/query",
        {
          accountId: "d",
          canCalculateChanges: true,
          ids: ["b"],
          position: 0,
          queryState: "sjq",
        },
        "c0",
      ],
      [
        "ContactCard/get",
        {
          accountId: "d",
          // As Stalwart answers (checked live).
          list: [
            {
              emails: {
                e1: { "@type": "EmailAddress", address: "carla@x.com" },
              },
              id: "b",
              name: { "@type": "Name", full: "Carla Gómez" },
            },
          ],
          notFound: [],
          state: "sjq",
        },
        "c1",
      ],
    ]);
    const [, cards] = await client.request((b) => {
      const query = b.call("ContactCard/query", { accountId, limit: 10 });
      const get = b.call("ContactCard/get", {
        "#ids": query.ref("/ids"),
        accountId,
        properties: ["name", "emails"],
      });
      return [query, get] as const;
    });
    const [card] = cards.list;
    expectTypeOf(card?.emails).not.toBeAny();
    expect(card?.name?.full).toBe("Carla Gómez");
    expect(card?.emails?.e1?.address).toBe("carla@x.com");
    expect(usingOf(fake.calls[1]?.body)).toContain(
      "urn:ietf:params:jmap:contacts"
    );
  });

  it("fills in requested properties Stalwart leaves out with null", async () => {
    const { client } = await connect([
      [
        "ContactCard/get",
        {
          accountId: "d",
          // Stalwart: no `uid` and no `phones` although both were asked.
          list: [{ id: "b", name: { full: "Carla" } }],
          notFound: [],
          state: "s",
        },
        "c0",
      ],
    ]);
    const [cards] = await client.request((b) => [
      b.call("ContactCard/get", {
        accountId,
        ids: [ContactCardId.parse("b")],
        properties: ["uid", "name", "phones"],
      }),
    ]);
    expect(cards.list[0]).toMatchObject({ phones: null, uid: null });
  });

  it("keeps unknown JSContact properties and parses the typed ones", async () => {
    const { client } = await connect([
      [
        "ContactCard/get",
        {
          accountId: "d",
          list: [
            {
              addressBookIds: { b: true },
              anniversaries: {
                k1: {
                  date: { "@type": "PartialDate", day: 17, month: 5 },
                  kind: "birth",
                },
              },
              id: "c",
              links: { l1: { uri: "https://example.com" } },
              media: { m1: { kind: "photo", uri: "data:image/png;base64,AA" } },
              phones: { p1: { features: { mobile: true }, number: "+34 1" } },
            },
          ],
          notFound: [],
          state: "s",
        },
        "c0",
      ],
    ]);
    const [cards] = await client.request((b) => [
      b.call("ContactCard/get", { accountId, ids: [ContactCardId.parse("c")] }),
    ]);
    const [card] = cards.list;
    expect(card?.phones?.p1?.number).toBe("+34 1");
    expect(card?.anniversaries?.k1?.date).toMatchObject({ day: 17 });
    expect(card).toMatchObject({
      links: { l1: { uri: "https://example.com" } },
    });
    expect(card?.name).toBeNull();
  });
});

describe("ContactCard/set and AddressBook/get", () => {
  it("creates a card in an address book", async () => {
    const { client, fake } = await connect([
      [
        "AddressBook/get",
        {
          accountId: "d",
          list: [
            {
              description: null,
              id: "b",
              isDefault: true,
              isSubscribed: true,
              myRights: {
                mayDelete: true,
                mayRead: true,
                mayShare: true,
                mayWrite: true,
              },
              name: "Personal",
              shareWith: {},
              sortOrder: 0,
            },
          ],
          notFound: [],
          state: "s",
        },
        "c0",
      ],
      [
        "ContactCard/set",
        {
          accountId: "d",
          created: { new: { id: "x" } },
          newState: "s2",
          oldState: "s",
        },
        "c1",
      ],
    ]);
    const book = AddressBookId.parse("b");
    const [books, set] = await client.request((b) => [
      b.call("AddressBook/get", { accountId }),
      b.call("ContactCard/set", {
        accountId,
        create: {
          new: {
            "@type": "Card",
            addressBookIds: { [book]: true },
            emails: { e1: { address: "ana@example.com" } },
            kind: "individual",
            name: { full: "Ana" },
            uid: "urn:uuid:1",
            version: "1.0",
          },
        },
      }),
    ]);
    expect(books.list[0]?.isDefault).toBeTruthy();
    expect(set.created.new?.id).toBe("x");
    expect(set.notUpdated).toStrictEqual({});
    expect(usingOf(fake.calls[1]?.body)).toContain(
      "urn:ietf:params:jmap:contacts"
    );
  });
});

describe("ContactCard/parse", () => {
  it("parses vCard blobs with contacts:parse", async () => {
    const blob = BlobId.parse("Gabc");
    const { client, fake } = await connect([
      [
        "ContactCard/parse",
        {
          accountId: "d",
          parsed: {
            Gabc: {
              "@type": "Card",
              emails: { k1: { address: "dora@example.com" } },
              name: { full: "Dora" },
              vCard: { properties: [] },
            },
          },
        },
        "c0",
      ],
    ]);
    const [parsed] = await client.request((b) => [
      b.call("ContactCard/parse", { accountId, blobIds: [blob] }),
    ]);
    expect(parsed.parsed[blob]?.name?.full).toBe("Dora");
    expect(parsed.notFound).toStrictEqual([]);
    expect(parsed.notParsable).toStrictEqual([]);
    expect(usingOf(fake.calls[1]?.body)).toStrictEqual(
      expect.arrayContaining([
        "urn:ietf:params:jmap:contacts",
        "urn:ietf:params:jmap:contacts:parse",
      ])
    );
  });
});
