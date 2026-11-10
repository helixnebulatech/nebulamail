// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { EmailId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import { methodError, scriptedJmap } from "../test/scripted-jmap";
import type { Invocation } from "../test/scripted-jmap";
import {
  anyExchange,
  firstContactEligible,
  isFirstContact,
} from "./sender-new";

const addr = (local: string): string => `${local}@example.com`;
const SELF = EmailId.parse("m-self");
const BEFORE = "2026-11-03T10:00:00Z";

type Hit = Parameters<typeof anyExchange>[0][number];

/** A scripted server whose search finds `hits`. */
const server = (hits: readonly Hit[]) =>
  scriptedJmap({
    handlers: {
      "Email/get": (_args, callId): readonly Invocation[] => [
        [
          "Email/get",
          { accountId: "a", list: hits, notFound: [], state: "1" },
          callId,
        ],
      ],
      "Email/query": (_args, callId): readonly Invocation[] => [
        [
          "Email/query",
          {
            accountId: "a",
            canCalculateChanges: false,
            ids: hits.map((hit) => hit.id),
            position: 0,
            queryState: "1",
          },
          callId,
        ],
      ],
    },
  });

const hit = (id: string, fields: Partial<Hit>): Hit => ({
  id: EmailId.parse(id),
  ...fields,
});

describe(anyExchange, () => {
  it("counts only exact addresses in From, To or Cc, never this email", () => {
    const bob = addr("bob");
    expect(
      [
        hit("m1", { from: [{ email: bob.toUpperCase() }] }),
        hit("m1", { to: [{ email: bob }] }),
        // The word search found something else.
        hit("m1", { from: [{ email: `${bob}.evil.test` }] }),
        hit("m-self", { from: [{ email: bob }] }),
      ].map((one) => anyExchange([one], bob, SELF))
    ).toStrictEqual([true, true, false, false]);
  });
});

describe(isFirstContact, () => {
  it("asks once for earlier mail from, to or copying the address", async () => {
    const scripted = server([]);
    const client = await scripted.connect();
    expect([
      await isFirstContact(addr("bob"), BEFORE, SELF, client),
    ]).toStrictEqual([true]);
    const query = scripted.calls.find(([name]) => name === "Email/query");
    expect(query?.[1]).toMatchObject({
      filter: {
        conditions: [
          { before: BEFORE },
          {
            conditions: [
              { from: addr("bob") },
              { to: addr("bob") },
              { cc: addr("bob") },
            ],
            operator: "OR",
          },
        ],
        operator: "AND",
      },
      limit: 10,
    });
  });

  it("an earlier message means it isn't the first time", async () => {
    const scripted = server([
      {
        cc: null,
        from: [{ email: addr("bob") }],
        id: EmailId.parse("m-old"),
        to: [],
      },
    ]);
    expect([
      await isFirstContact(addr("bob"), BEFORE, SELF, await scripted.connect()),
    ]).toStrictEqual([false]);
  });

  it("can't tell on an error or an odd address", async () => {
    const scripted = scriptedJmap({
      handlers: {
        "Email/query": (_args, callId) => [
          methodError("unsupportedFilter", callId),
        ],
      },
    });
    const client = await scripted.connect();
    await expect(
      isFirstContact(addr("bob"), BEFORE, SELF, client)
    ).resolves.toBeNull();
    const odd = await Promise.all(
      ["", "no-at", `${"a".repeat(300)}@example.com`, "a b@example.com"].map(
        async (address) => await isFirstContact(address, BEFORE, SELF, client)
      )
    );
    expect(odd).toStrictEqual([null, null, null, null]);
    await expect(
      isFirstContact(addr("bob"), "yesterday", SELF, client)
    ).resolves.toBeNull();
  });
});

describe(firstContactEligible, () => {
  const email = {
    accountId: null,
    draft: false,
    from: [{ email: addr("ana") }],
  };
  const own = [addr("me")];

  it("checks a single outside sender in our own account", () => {
    expect(firstContactEligible(email, own, true)).toBeTruthy();
  });

  it("never with the setting off (so no search is made)", () => {
    expect(firstContactEligible(email, own, false)).toBeFalsy();
  });

  it("not for our own mail, drafts, shared accounts or several From", () => {
    expect(
      firstContactEligible(
        { ...email, from: [{ email: addr("me") }] },
        own,
        true
      )
    ).toBeFalsy();
    expect(
      firstContactEligible({ ...email, draft: true }, own, true)
    ).toBeFalsy();
    expect(
      firstContactEligible({ ...email, accountId: "b" }, own, true)
    ).toBeFalsy();
    expect(
      firstContactEligible(
        { ...email, from: [{ email: addr("ana") }, { email: addr("bob") }] },
        own,
        true
      )
    ).toBeFalsy();
    expect(firstContactEligible(email, [], true)).toBeFalsy();
  });
});
