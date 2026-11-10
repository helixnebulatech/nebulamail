// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { JmapClient } from "./client";
import { Slots } from "./slots";
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

const tooMany = (status: number) =>
  Response.json(
    {
      limit: "maxConcurrentRequests",
      status,
      type: "urn:ietf:params:jmap:error:limit",
    },
    { status }
  );

const answered = () => methodResponses([["Core/echo", { hello: true }, "c0"]]);

const connect = async (api: Handler, core = {}) => {
  const fake = fakeFetch(sessionThen(api, stalwartSession({}, core)));
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...tokens(),
  });
  const apiCalls = () => fake.calls.filter((call) => call.url === API_URL);
  return { apiCalls, client };
};

/** `task` once `slots` has room for it. */
const inTurn = async <T>(
  slots: Slots,
  max: number,
  signal: AbortSignal | undefined,
  task: () => Promise<T>
): Promise<T> => {
  await slots.acquire(max, signal);
  try {
    return await task();
  } finally {
    slots.release();
  }
};

const echo = async (client: JmapClient, signal?: AbortSignal) =>
  await client.request(
    (b) => b.call("Core/echo", { hello: true }),
    signal === undefined ? {} : { signal }
  );

describe(Slots, () => {
  it("runs at most `max` tasks, in order of arrival", async () => {
    const slots = new Slots();
    const started: number[] = [];
    const finish: (() => void)[] = [];
    const task = async (index: number) => {
      await inTurn(slots, 2, undefined, async () => {
        started.push(index);
        const { promise, resolve } = Promise.withResolvers<null>();
        finish.push(() => {
          resolve(null);
        });
        await promise;
      });
    };

    const all = [task(0), task(1), task(2), task(3)];
    await vi.waitFor(() => {
      expect(started).toStrictEqual([0, 1]);
    });
    finish[0]?.();
    await vi.waitFor(() => {
      expect(started).toStrictEqual([0, 1, 2]);
    });
    finish[1]?.();
    finish[2]?.();
    await vi.waitFor(() => {
      expect(started).toStrictEqual([0, 1, 2, 3]);
    });
    finish[3]?.();
    await Promise.all(all);
  });

  it("frees the place when a task fails", async () => {
    const slots = new Slots();
    const failed = inTurn(slots, 1, undefined, async () => {
      await Promise.reject(new Error("no"));
    });
    const next = inTurn(
      slots,
      1,
      undefined,
      async () => await Promise.resolve("yes")
    );

    await expect(failed).rejects.toThrow("no");
    await expect(next).resolves.toBe("yes");
  });

  it("drops a waiting task when its signal fires", async () => {
    const slots = new Slots();
    const { promise, resolve } = Promise.withResolvers<null>();
    const running = inTurn(slots, 1, undefined, async () => {
      await promise;
    });
    const controller = new AbortController();
    const dropped = vi.fn<() => Promise<void>>();
    const waiting = inTurn(slots, 1, controller.signal, dropped);
    const after = inTurn(
      slots,
      1,
      undefined,
      async () => await Promise.resolve("after")
    );

    controller.abort();

    const { detail } = await rejection(waiting);

    expect(detail.kind).toBe("aborted");

    resolve(null);
    await running;

    await expect(after).resolves.toBe("after");
    expect(dropped).not.toHaveBeenCalled();
  });
});

describe("a slot handed over as its signal fires", () => {
  it("keeps the others in line", async () => {
    const slots = new Slots();
    await slots.acquire(1, undefined);
    const controller = new AbortController();
    const first = slots.acquire(1, controller.signal);
    const second = slots.acquire(1, undefined);

    slots.release();
    controller.abort();
    await first;
    slots.release();

    await expect(second).resolves.toBeUndefined();
  });
});

describe("maxConcurrentRequests", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("never sends more at once than the session allows", async () => {
    let running = 0;
    let most = 0;
    const { client } = await connect(
      async () => {
        running += 1;
        most = Math.max(most, running);
        await Promise.resolve();
        await Promise.resolve();
        running -= 1;
        return answered();
      },
      { maxConcurrentRequests: 2 }
    );

    await Promise.all(
      [1, 2, 3, 4, 5, 6, 7].map(async () => await echo(client))
    );

    expect(most).toBe(2);
  });

  it.each([
    ["Stalwart", 400],
    ["the bridge", 429],
  ])("repeats a request %s refused as one too many", async (_name, status) => {
    const { apiCalls, client } = await connect((_call, index) =>
      // The session was call 0.
      index <= 2 ? tooMany(status) : answered()
    );

    const result = echo(client);
    await vi.runAllTimersAsync();

    await expect(result).resolves.toStrictEqual({ hello: true });
    expect(apiCalls()).toHaveLength(3);
  });

  it("gives up after a while, with the server's error", async () => {
    const { apiCalls, client } = await connect(() => tooMany(429));

    const result = rejection(echo(client));
    await vi.runAllTimersAsync();

    const { detail } = await result;

    expect(detail).toMatchObject({
      kind: "request",
      limit: "maxConcurrentRequests",
    });
    expect(apiCalls()).toHaveLength(7);
  });

  it("doesn't repeat other limits", async () => {
    const { apiCalls, client } = await connect(() =>
      Response.json(
        {
          limit: "maxSizeRequest",
          status: 400,
          type: "urn:ietf:params:jmap:error:limit",
        },
        { status: 400 }
      )
    );

    const error = await rejection(echo(client));

    expect(error.detail).toMatchObject({ limit: "maxSizeRequest" });
    expect(apiCalls()).toHaveLength(1);
  });

  it("stops waiting to repeat when the signal fires", async () => {
    const { apiCalls, client } = await connect(() => tooMany(429));
    const controller = new AbortController();

    const result = rejection(echo(client, controller.signal));
    await vi.waitFor(() => {
      expect(apiCalls()).toHaveLength(1);
    });
    await vi.advanceTimersByTimeAsync(10);
    controller.abort();

    const { detail } = await result;

    expect(detail.kind).toBe("aborted");
    expect(apiCalls()).toHaveLength(1);
  });
});
