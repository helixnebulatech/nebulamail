// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { z } from "zod";

import { JmapClient } from "./client";
import {
  HIDDEN_RETRY_AFTER_MS,
  SERVER_UNAVAILABLE_WAIT_MS,
  isSafeToRepeat,
  pause,
  spread,
  unavailableMethodWaitMs,
  unavailableWaitMs,
} from "./retry";
import {
  API_URL,
  SESSION_URL,
  fakeFetch,
  methodResponses,
  rejection,
  sessionThen,
  stalwartSession,
  tokens,
} from "./test-support";
import type { Handler } from "./test-support";

const unavailable = (retryAfter?: string, status = 503): Response =>
  new Response(null, {
    status,
    ...(retryAfter === undefined
      ? {}
      : { headers: { "Retry-After": retryAfter } }),
  });

const request = (...names: readonly string[]): string =>
  JSON.stringify({
    methodCalls: names.map((name, index) => [name, {}, `c${index}`]),
    using: [],
  });

describe(unavailableWaitMs, () => {
  it("reads seconds and dates", () => {
    const now = Date.parse("2026-10-31T10:00:00Z");
    expect(unavailableWaitMs(unavailable("5"), now)).toBe(5000);
    expect(
      unavailableWaitMs(unavailable("Sat, 31 Oct 2026 10:00:03 GMT"), now)
    ).toBe(3000);
    expect(
      unavailableWaitMs(unavailable("Sat, 31 Oct 2026 09:00:00 GMT"), now)
    ).toBe(0);
  });

  it("reads the obsolete date formats too (RFC 9110 §5.6.7)", () => {
    const now = Date.parse("2026-10-31T10:00:00Z");
    expect(
      unavailableWaitMs(unavailable("Saturday, 31-Oct-26 10:00:04 GMT"), now)
    ).toBe(4000);
    expect(
      unavailableWaitMs(unavailable("Sat Oct 31 10:00:02 2026"), now)
    ).toBe(2000);
    expect(unavailableWaitMs(unavailable("-1"), now)).toBeNull();
  });

  it("assumes the usual wait when CORS hides the header", () => {
    const hidden = unavailable();
    Object.defineProperty(hidden, "type", { value: "cors" });
    expect(unavailableWaitMs(hidden)).toBe(HIDDEN_RETRY_AFTER_MS);
  });

  it("doesn't wait for anything else", () => {
    expect(unavailableWaitMs(unavailable())).toBeNull();
    expect(unavailableWaitMs(unavailable("-1"))).toBeNull();
    expect(unavailableWaitMs(unavailable("11"))).toBeNull();
    expect(unavailableWaitMs(unavailable("9".repeat(400)))).toBeNull();
    expect(unavailableWaitMs(unavailable("5", 429))).toBeNull();
  });
});

describe(isSafeToRepeat, () => {
  it("repeats a GET and reads", () => {
    expect(isSafeToRepeat(undefined, undefined)).toBeTruthy();
    expect(isSafeToRepeat("GET", undefined)).toBeTruthy();
    expect(
      isSafeToRepeat(
        "POST",
        request("Email/query", "Email/get", "Email/changes")
      )
    ).toBeTruthy();
    expect(
      isSafeToRepeat("POST", request("ContactCard/parse", "Email/queryChanges"))
    ).toBeTruthy();
  });

  it("never repeats a write", () => {
    for (const name of [
      "Email/set",
      "Email/copy",
      "Email/import",
      "EmailSubmission/set",
      "Blob/upload",
      "CalendarEvent/counter",
      "MDN/send",
    ]) {
      expect(isSafeToRepeat("POST", request("Email/get", name))).toBeFalsy();
    }
  });

  it("doesn't guess with other bodies", () => {
    expect(isSafeToRepeat("POST", undefined)).toBeFalsy();
    expect(isSafeToRepeat("POST", new Uint8Array(1))).toBeFalsy();
    expect(isSafeToRepeat("POST", "not json")).toBeFalsy();
    expect(isSafeToRepeat("POST", "{}")).toBeFalsy();
    expect(isSafeToRepeat("POST", request())).toBeFalsy();
  });
});

describe(pause, () => {
  it("fails at once when already aborted", async () => {
    await expect(pause(10_000, AbortSignal.abort())).rejects.toMatchObject({
      detail: { kind: "aborted" },
    });
  });

  it("resolves after the wait", async () => {
    await expect(pause(1, undefined)).resolves.toBeUndefined();
  });
});

const CallsSchema = z.object({
  methodCalls: z.tuple([z.tuple([z.string(), z.unknown(), z.string()])]),
});

const outOfMemory = ["error", { type: "serverUnavailable" }, "c0"] as const;

describe(unavailableMethodWaitMs, () => {
  it("waits before repeating reads", () => {
    expect(
      unavailableMethodWaitMs(
        request("Email/query", "Email/get"),
        [
          ["Email/query", {}, "c0"],
          ["error", { type: "serverUnavailable" }, "c1"],
        ],
        0
      )
    ).toBe(SERVER_UNAVAILABLE_WAIT_MS);
  });

  it("gives up after two tries", () => {
    expect(
      unavailableMethodWaitMs(request("Email/get"), [outOfMemory], 2)
    ).toBeNull();
  });

  it("never repeats a write", () => {
    expect(
      unavailableMethodWaitMs(
        request("Email/get", "Email/set"),
        [outOfMemory],
        0
      )
    ).toBeNull();
  });

  it("doesn't repeat other errors", () => {
    expect(
      unavailableMethodWaitMs(
        request("Email/get"),
        [["error", { type: "serverFail" }, "c0"]],
        0
      )
    ).toBeNull();
    expect(
      unavailableMethodWaitMs(
        request("Email/get"),
        [["Email/get", {}, "c0"]],
        0
      )
    ).toBeNull();
  });
});

const connect = async (api: Handler, maxConcurrentRequests = 4) => {
  const fake = fakeFetch(
    sessionThen(api, stalwartSession({}, { maxConcurrentRequests }))
  );
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...tokens(),
  });
  const apiCalls = () => fake.calls.filter((call) => call.url === API_URL);
  const accountId = client.primaryAccountId("urn:ietf:params:jmap:mail");
  const read = async (signal?: AbortSignal) =>
    await client.request(
      (b) => b.call("Mailbox/get", { accountId, ids: null }),
      signal === undefined ? {} : { signal }
    );
  return { apiCalls, client, read };
};

const echoed = (call: { body: unknown }) => {
  const {
    methodCalls: [[name, , callId]],
  } = CallsSchema.parse(call.body);
  return name === "Mailbox/get"
    ? methodResponses([
        [name, { accountId: "d", list: [], notFound: [], state: "s" }, callId],
      ])
    : methodResponses([[name, { hello: true }, callId]]);
};
const unavailableNow = () => methodResponses([outOfMemory]);

describe(spread, () => {
  it("waits at least what was asked, and up to half of it more", () => {
    expect(
      [0, 0.5, 0.999].map((value) => spread(5000, () => value))
    ).toStrictEqual([5000, 6250, 7497]);
  });
});

describe("serverUnavailable in a method", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    // The waits exactly as asked (spread adds nothing).
    vi.spyOn(Math, "random").mockReturnValue(0);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("repeats a read after the wait", async () => {
    const { apiCalls, read } = await connect((call, index) =>
      index <= 1 ? unavailableNow() : echoed(call)
    );
    const result = read();
    await vi.advanceTimersByTimeAsync(0);

    expect(apiCalls()).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(SERVER_UNAVAILABLE_WAIT_MS - 1);

    expect(apiCalls()).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(1);

    await expect(result).resolves.toBeDefined();
    expect(apiCalls()).toHaveLength(2);
  });

  it("gives the error after two retries", async () => {
    const { apiCalls, read } = await connect(() => unavailableNow());
    const result = rejection(read());
    await vi.advanceTimersByTimeAsync(SERVER_UNAVAILABLE_WAIT_MS * 3);
    const { detail } = await result;

    expect(detail).toMatchObject({ kind: "method", type: "serverUnavailable" });
    expect(apiCalls()).toHaveLength(3);
  });

  it("never repeats a write", async () => {
    const { apiCalls, client } = await connect(() => unavailableNow());
    const accountId = client.primaryAccountId("urn:ietf:params:jmap:mail");
    const { detail } = await rejection(
      client.request((b) => b.call("Email/set", { accountId, destroy: [] }))
    );

    expect(detail).toMatchObject({ type: "serverUnavailable" });
    expect(apiCalls()).toHaveLength(1);
  });

  it("stops waiting when the signal fires", async () => {
    const { apiCalls, read } = await connect(() => unavailableNow());
    const controller = new AbortController();
    const result = rejection(read(controller.signal));
    await vi.advanceTimersByTimeAsync(0);

    expect(apiCalls()).toHaveLength(1);

    controller.abort();
    const { detail } = await result;

    expect(detail.kind).toBe("aborted");
    expect(apiCalls()).toHaveLength(1);
  });

  it("holds no slot while it waits", async () => {
    let first = true;
    const { apiCalls, client, read } = await connect((call) => {
      if (first) {
        first = false;
        return unavailableNow();
      }
      return echoed(call);
    }, 1);
    const slow = read();
    await vi.advanceTimersByTimeAsync(0);

    expect(apiCalls()).toHaveLength(1);

    // The session allows one at a time; this one goes during the wait.
    await expect(
      client.request((b) => b.call("Core/echo", { hello: true }))
    ).resolves.toBeDefined();
    await vi.advanceTimersByTimeAsync(SERVER_UNAVAILABLE_WAIT_MS);
    await expect(slow).resolves.toBeDefined();
  });
});
