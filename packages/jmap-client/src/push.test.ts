// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { setTimeout as delay } from "node:timers/promises";

import { describe, expect, it } from "vitest";

import { JmapClient } from "./client";
import { AccountId } from "./ids";
import type { StateChange } from "./push";
import { SESSION_URL, fakeFetch, sessionThen, tokens } from "./test-support";
import type { Handler } from "./test-support";

const accountId = AccountId.parse("d");

const stateEvent = (id: string, emailState: string): string =>
  `event: state\nid: ${id}\ndata: ${JSON.stringify({
    "@type": "StateChange",
    changed: { d: { Email: emailState } },
  })}\n\n`;

const sse = (text: string): Response =>
  new Response(text, { headers: { "Content-Type": "text/event-stream" } });

/** A response whose body fails on first read (connection dropped). */
const broken = (): Response =>
  new Response(
    new ReadableStream({
      pull: (controller) => {
        controller.error(new TypeError("terminated"));
      },
    })
  );

const setup = async (api: Handler) => {
  const fake = fakeFetch(sessionThen(api));
  const client = await JmapClient.connect({
    fetch: fake.fetch,
    sessionUrl: SESSION_URL,
    ...tokens(),
  });
  return { calls: fake.calls, client };
};

const take = async (
  iterator: AsyncGenerator<StateChange, void, undefined>,
  count: number
): Promise<StateChange[]> => {
  const out: StateChange[] = [];
  for await (const change of iterator) {
    out.push(change);
    if (out.length === count) {
      break;
    }
  }
  return out;
};

describe("push", () => {
  it("yields typed StateChanges and ignores pings", async () => {
    const { calls, client } = await setup(() =>
      sse(
        `: hi\n\nevent: ping\ndata: {"interval":30}\n\n${stateEvent("1", "e2")}`
      )
    );
    const [change] = await take(
      client.push({ types: ["Email", "Mailbox"] }),
      1
    );
    expect(change?.changed).toStrictEqual({ d: { Email: "e2" } });
    expect(calls[1]?.url).toBe(
      "https://mail.example.com/jmap/eventsource/?types=Email%2CMailbox&closeafter=no&ping=30"
    );
    expect(new Headers(calls[1]?.init.headers).get("accept")).toBe(
      "text/event-stream"
    );
  });

  it("asks for no caching without a Cache-Control header (no CORS preflight)", async () => {
    const { calls, client } = await setup(() => sse(stateEvent("1", "e2")));
    await take(client.push(), 1);
    const init = calls[1]?.init;

    expect(init?.cache).toBe("no-store");
    expect(new Headers(init?.headers).has("cache-control")).toBeFalsy();
  });

  it("reconnects with Last-Event-ID after the stream ends", async () => {
    const { calls, client } = await setup((_call, index) =>
      sse(stateEvent(String(index), `e${String(index)}`))
    );
    const changes = await take(
      client.push({ maxBackoffMs: 5, minBackoffMs: 1 }),
      2
    );
    expect(changes.map((change) => change.changed[accountId])).toStrictEqual([
      { Email: "e1" },
      { Email: "e2" },
    ]);
    expect(new Headers(calls[2]?.init.headers).get("last-event-id")).toBe("1");
  });

  it("retries server errors but stops on fatal client errors", async () => {
    const { calls, client } = await setup((_call, index) =>
      index === 1
        ? new Response(null, { status: 503 })
        : Response.json({ type: "forbidden" }, { status: 403 })
    );
    await expect(
      take(client.push({ maxBackoffMs: 5, minBackoffMs: 1 }), 1)
    ).rejects.toMatchObject({ detail: { kind: "request", status: 403 } });
    expect(calls).toHaveLength(3);
  });

  it("reconnects when a connection drops mid-stream", async () => {
    const { calls, client } = await setup((_call, index) =>
      index === 1 ? broken() : sse(stateEvent("1", "e1"))
    );
    const [change] = await take(
      client.push({ maxBackoffMs: 5, minBackoffMs: 1 }),
      1
    );
    expect(change?.changed[accountId]).toStrictEqual({ Email: "e1" });
    expect(calls).toHaveLength(3);
  });

  it("says when an open connection ends, not when one fails to open", async () => {
    const { client } = await setup((_call, index) =>
      index === 1
        ? new Response(null, { status: 503 })
        : sse(stateEvent(String(index), `e${String(index)}`))
    );
    const events: string[] = [];
    const changes = client.push({
      maxBackoffMs: 5,
      minBackoffMs: 1,
      onConnect: () => {
        events.push("connect");
      },
      onDisconnect: () => {
        events.push("disconnect");
      },
    });
    for await (const change of changes) {
      events.push(`change ${String(change.changed[accountId]?.Email)}`);
      if (events.length === 5) {
        break;
      }
    }
    // The 503 opened nothing: no disconnect for it. Stopping (the
    // `break`) closes the last one.
    expect(events).toStrictEqual([
      "connect",
      "change e2",
      "disconnect",
      "connect",
      "change e3",
      "disconnect",
    ]);
  });

  it("says a connection is live at its first bytes, not when it only opens", async () => {
    let send: ((text: string) => void) | undefined;
    const { client } = await setup(
      () =>
        new Response(
          new ReadableStream<Uint8Array>({
            start: (controller) => {
              send = (text) => {
                controller.enqueue(new TextEncoder().encode(text));
              };
            },
          }),
          { headers: { "Content-Type": "text/event-stream" } }
        )
    );
    const events: string[] = [];
    const first = take(
      client.push({
        onConnect: () => {
          events.push("connect");
        },
        onLive: () => {
          events.push("live");
        },
      }),
      1
    );
    await expect.poll(() => events).toStrictEqual(["connect"]);
    // Headers but no body yet (a proxy holding it back): not live.
    await delay(20);
    expect(events).toStrictEqual(["connect"]);
    send?.('event: ping\ndata: {"interval":30}\n\n');
    await expect.poll(() => events).toStrictEqual(["connect", "live"]);
    send?.(stateEvent("1", "e2"));
    const [change] = await first;
    expect(change?.changed[accountId]).toStrictEqual({ Email: "e2" });
    // Once per connection.
    expect(events).toStrictEqual(["connect", "live"]);
  });

  it("ends quietly when aborted", async () => {
    const controller = new AbortController();
    const { client } = await setup(() => {
      controller.abort();
      return new Response(null, { status: 503 });
    });
    await expect(
      take(client.push({ signal: controller.signal }), 1)
    ).resolves.toStrictEqual([]);
  });
});
