// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";
import { z } from "zod";

import { JmapClient } from "./client";
import { AccountId, BlobId } from "./ids";
import { parseSession } from "./session";
import { SieveScriptId, sieveCapability, sieveErrorPosition } from "./sieve";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  sessionThen,
  stalwartSession,
  tokens,
} from "./test-support";
import { makeOriginPolicy } from "./url";

const accountId = AccountId.parse("d");

describe("SieveScript", () => {
  it("validates, creates and activates a script in one request", async () => {
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          ["SieveScript/validate", { accountId: "d", error: null }, "c0"],
          [
            "SieveScript/set",
            {
              accountId: "d",
              // As Stalwart answers (checked live).
              created: {
                s: { blobId: "Gnew", id: "c", isActive: true },
              },
              newState: "suqma",
              oldState: "summa",
            },
            "c1",
          ],
          [
            "SieveScript/get",
            {
              accountId: "d",
              list: [
                { blobId: "Gv", id: "b", isActive: false, name: "vacation" },
                { blobId: "Gnew", id: "c", isActive: true, name: "filters" },
              ],
              notFound: [],
              state: "suqma",
            },
            "c2",
          ],
        ])
      )
    );
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    const blobId = BlobId.parse("Gup");
    const [validate, set, get] = await client.request(
      (b) =>
        [
          b.call("SieveScript/validate", { accountId, blobId }),
          b.call("SieveScript/set", {
            accountId,
            create: { s: { blobId, name: "filters" } },
            onSuccessActivateScript: "#s",
          }),
          b.call("SieveScript/get", { accountId }),
        ] as const
    );
    expect(validate.error).toBeNull();
    expect(set.created.s?.isActive).toBeTruthy();
    expect(get.list.map((script) => script.name)).toStrictEqual([
      "vacation",
      "filters",
    ]);
    const body = z
      .object({
        methodCalls: z.array(z.tuple([z.string(), z.unknown(), z.string()])),
        using: z.array(z.string()),
      })
      .parse(fake.calls[1]?.body);
    expect(body.using).toContain("urn:ietf:params:jmap:sieve");
    expect(body.methodCalls[1]?.[1]).toMatchObject({
      onSuccessActivateScript: "#s",
    });
  });

  // RFC 9661 §2.6 says `invalidSieve`; Stalwart 1.0.1 sends `invalidScript`.
  it.each(["invalidSieve", "invalidScript"])(
    "returns validation errors (%s) instead of throwing",
    async (type) => {
      const fake = fakeFetch(
        sessionThen(() =>
          methodResponses([
            [
              "SieveScript/validate",
              {
                accountId: "d",
                error: {
                  description:
                    'Expected token "\';\'" but found "}" at line 5, column 1.',
                  type,
                },
              },
              "c0",
            ],
          ])
        )
      );
      const client = await JmapClient.connect({
        fetch: fake.fetch,
        sessionUrl: SESSION_URL,
        ...tokens(),
      });
      const result = await client.request((b) =>
        b.call("SieveScript/validate", {
          accountId,
          blobId: BlobId.parse("G1"),
        })
      );
      expect(result.error?.type).toBe(type);
      expect(sieveErrorPosition(result.error?.description)).toStrictEqual({
        column: 1,
        line: 5,
      });
    }
  );

  it("rejects a malformed script object", async () => {
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          [
            "SieveScript/get",
            {
              accountId: "d",
              list: [{ blobId: "G", id: "b", name: "x" }],
              notFound: [],
              state: "1",
            },
            "c0",
          ],
        ])
      )
    );
    const client = await JmapClient.connect({
      fetch: fake.fetch,
      sessionUrl: SESSION_URL,
      ...tokens(),
    });
    await expect(
      client.request((b) => b.call("SieveScript/get", { accountId }))
    ).rejects.toMatchObject({ detail: { kind: "invalidResponse" } });
  });
});

describe(sieveErrorPosition, () => {
  it.each([
    ["Undeclared capability 'regex' at line 1, column 10.", 1, 10],
    ["error at line 12", 12, null],
  ])("reads %s", (text, line, column) => {
    expect(sieveErrorPosition(text)).toStrictEqual({ column, line });
  });

  it("is null without a position", () => {
    expect(sieveErrorPosition("bad")).toBeNull();
    expect(sieveErrorPosition(null)).toBeNull();
  });
});

describe(sieveCapability, () => {
  it("reads the account capability", () => {
    const base = stalwartSession();
    const url = new URL(SESSION_URL);
    const session = parseSession(
      {
        ...base,
        accounts: {
          d: {
            accountCapabilities: {
              "urn:ietf:params:jmap:sieve": {
                maxNumberRedirects: 1,
                maxSizeScript: 102_400,
                sieveExtensions: ["fileinto", "imap4flags"],
              },
            },
            isPersonal: true,
            isReadOnly: false,
            name: "jane",
          },
        },
      },
      url,
      makeOriginPolicy(url, false)
    );
    expect(sieveCapability(session, accountId)?.maxNumberRedirects).toBe(1);
    expect(SieveScriptId.safeParse("a b").success).toBeFalsy();
  });
});
