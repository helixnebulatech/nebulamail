// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import { AccountId, BlobId } from "./ids";
import {
  SESSION_URL,
  fakeFetch,
  methodResponses,
  rejection,
  sessionThen,
  tokens,
} from "./test-support";
import type { Handler } from "./test-support";

const accountId = AccountId.parse("d");

const echo = methodResponses([["Core/echo", { hello: true }, "c0"]]);

const setup = async (api: Handler) => {
  const fake = fakeFetch(sessionThen(api));
  const token = tokens();
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    getAccessToken: token.getAccessToken,
    sessionUrl: SESSION_URL,
  });
  return { calls: fake.calls, client, refreshes: token.requests };
};

const busy = (retryAfter: string | null = "0"): Response =>
  new Response('{"type":"about:blank","status":503}', {
    headers:
      retryAfter === null
        ? { "Content-Type": "application/problem+json" }
        : {
            "Content-Type": "application/problem+json",
            "Retry-After": retryAfter,
          },
    status: 503,
  });

describe("503 with Retry-After", () => {
  it("repeats a request of reads", async () => {
    const mailboxes = methodResponses([
      ["Mailbox/get", { accountId, list: [], notFound: [], state: "s" }, "c0"],
    ]);
    const answers = [busy(), mailboxes];
    const { calls, client } = await setup(
      () => answers.shift() ?? mailboxes.clone()
    );
    await client.request((b) => b.call("Mailbox/get", { accountId }));
    expect(calls).toHaveLength(3);
  });

  it("repeats a download", async () => {
    const answers = [busy(), new Response("ok")];
    const { calls, client } = await setup(
      () => answers.shift() ?? new Response("ok")
    );
    const blob = await client.downloadBlob({
      accountId,
      blobId: BlobId.parse("Gblob"),
    });
    await expect(blob.text()).resolves.toBe("ok");
    expect(calls).toHaveLength(3);
  });

  it("doesn't repeat a request that writes", async () => {
    const { calls, client } = await setup(() => busy());
    const error = await rejection(
      client.request((b) => [
        b.call("Mailbox/get", { accountId }),
        b.call("Mailbox/set", { accountId, destroy: [] }),
      ])
    );
    expect(error.kind).toBe("request");
    expect(calls).toHaveLength(2);
  });

  it("gives up after two more tries", async () => {
    const { calls, client } = await setup(() => busy());
    const error = await rejection(
      client.request((b) => b.call("Mailbox/get", { accountId }))
    );
    expect(error.kind).toBe("request");
    expect(calls).toHaveLength(4);
  });

  it("doesn't repeat without Retry-After or with a long one", async () => {
    for (const header of [null, "120", "soon"]) {
      // One server at a time.
      // oxlint-disable-next-line no-await-in-loop
      const { calls, client } = await setup(() => busy(header));
      // oxlint-disable-next-line no-await-in-loop
      await rejection(
        client.request((b) => b.call("Mailbox/get", { accountId }))
      );
      expect(calls).toHaveLength(2);
    }
  });

  it("stops waiting when aborted", async () => {
    const { calls, client } = await setup(() => busy("5"));
    const controller = new AbortController();
    const pending = rejection(
      client.request((b) => b.call("Mailbox/get", { accountId }), {
        signal: controller.signal,
      })
    );
    setTimeout(() => {
      controller.abort();
    }, 10);
    const error = await pending;
    expect(error.kind).toBe("aborted");
    expect(calls).toHaveLength(2);
  });
});

describe("authentication", () => {
  it("refreshes the token once after a 401 and retries", async () => {
    const { calls, client, refreshes } = await setup(({ authorization }) =>
      authorization === "Bearer fresh-token"
        ? echo.clone()
        : new Response(null, { status: 401 })
    );
    const result = await client.request((b) =>
      b.call("Core/echo", { hello: true })
    );
    expect(result).toStrictEqual({ hello: true });
    expect(refreshes).toStrictEqual([false, false, true]);
    expect(calls.slice(1).map((call) => call.authorization)).toStrictEqual([
      "Bearer old-token",
      "Bearer fresh-token",
    ]);
  });

  it("gives up with `unauthorized` after the second 401", async () => {
    const { calls, client } = await setup(
      () => new Response(null, { status: 401 })
    );
    const error = await rejection(
      client.request((b) => b.call("Mailbox/get", { accountId }))
    );
    expect(error.kind).toBe("unauthorized");
    expect(calls).toHaveLength(3);
    expect(JSON.stringify([error.message, error.detail])).not.toMatch(
      /old-token|fresh-token/u
    );
  });

  it("refreshes only once for concurrent 401s", async () => {
    const { client, refreshes } = await setup(({ authorization }) =>
      authorization === "Bearer fresh-token"
        ? echo.clone()
        : new Response(null, { status: 401 })
    );
    await Promise.all([
      client.request((b) => b.call("Core/echo", {})),
      client.request((b) => b.call("Core/echo", {})),
    ]);
    expect(refreshes.filter(Boolean)).toHaveLength(1);
  });

  it("wraps getAccessToken failures without leaking details", async () => {
    const fake = fakeFetch(() => echo.clone());
    const error = await rejection(
      JmapClient.connect({
        fetch: fake.fetch,
        getAccessToken: async () => {
          await Promise.resolve();
          throw new Error("BFF down");
        },
        sessionUrl: SESSION_URL,
      })
    );
    expect(error.kind).toBe("auth");
    expect(fake.calls).toHaveLength(0);
  });

  it("uses safe fetch options for API calls", async () => {
    const { calls, client } = await setup(() => echo.clone());
    await client.request((b) => b.call("Core/echo", {}));
    expect(calls[1]?.init).toMatchObject({
      credentials: "omit",
      method: "POST",
      redirect: "error",
      referrerPolicy: "no-referrer",
    });
  });

  it("maps fetch failures to network errors", async () => {
    const { client } = await setup(() => {
      throw new TypeError("connection reset");
    });
    const error = await rejection(
      client.request((b) => b.call("Core/echo", {}))
    );
    expect(error.kind).toBe("network");
  });
});
