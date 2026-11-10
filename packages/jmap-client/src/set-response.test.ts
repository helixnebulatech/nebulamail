// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// /set answers as servers really send them.

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import { AccountId, EmailId, IdentityId } from "./ids";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  sessionThen,
  tokens,
} from "./test-support";

describe("set responses", () => {
  it("reads the refusals when the server leaves out newState (Stalwart 1.0.1)", async () => {
    const description =
      "Server rejected MAIL-FROM: 501 5.5.4 Requested release time exceeds maximum of 2026-11-09T20:06:18Z.";
    const fake = fakeFetch(
      sessionThen(() =>
        methodResponses([
          // As Stalwart 1.0.1 answers a refused HOLDUNTIL (checked live).
          [
            "EmailSubmission/set",
            {
              accountId: "d",
              notCreated: {
                send: { description, type: "forbiddenMailFrom" },
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

    const [result] = await client.request((b) => [
      b.call("EmailSubmission/set", {
        accountId: AccountId.parse("d"),
        create: {
          send: {
            emailId: EmailId.parse("m1"),
            identityId: IdentityId.parse("i1"),
          },
        },
      }),
    ]);

    expect(result.newState).toBeNull();
    expect(result.notCreated.send).toStrictEqual({
      description,
      type: "forbiddenMailFrom",
    });
  });
});
