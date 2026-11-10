// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { feedSse, initialSseState, parseSse } from "./sse";
import type { SseEvent, SseState } from "./sse";

/** Feeds chunks one by one, collecting all events. */
const feedAll = (
  chunks: readonly string[]
): {
  events: SseEvent[];
  state: SseState;
} => {
  let state = initialSseState;
  const events: SseEvent[] = [];
  for (const chunk of chunks) {
    const result = feedSse(state, chunk);
    ({ state } = result);
    events.push(...result.events);
  }
  return { events, state };
};

const BOM = String.fromCodePoint(0xfe_ff);

const STREAM =
  'event: state\nid: 1\ndata: {"a":\ndata: 1}\n\n: ping comment\n\nevent: ping\ndata: {"interval":30}\n\n';

describe(parseSse, () => {
  it("joins multi-line data and keeps event types and ids", () => {
    expect(parseSse(STREAM)).toStrictEqual([
      { data: '{"a":\n1}', lastEventId: "1", type: "state" },
      { data: '{"interval":30}', lastEventId: "1", type: "ping" },
    ]);
  });

  it("ignores comments and blank-only blocks", () => {
    expect(parseSse(":hi\n\n\n:again\n\n")).toStrictEqual([]);
  });

  it.each([
    ["CRLF", "data: x\r\n\r\n"],
    ["CR", "data: x\r\r"],
    ["LF", "data: x\n\n"],
  ])("accepts %s line endings", (_name, text) => {
    expect(parseSse(text)).toStrictEqual([
      { data: "x", lastEventId: "", type: "message" },
    ]);
  });

  it("follows the spec for field parsing details", () => {
    expect(
      parseSse("data\n\ndata:  two spaces\n\nfoo: bar\ndata:x\nid: a\0b\n\n")
    ).toStrictEqual([
      { data: "", lastEventId: "", type: "message" },
      { data: " two spaces", lastEventId: "", type: "message" },
      { data: "x", lastEventId: "", type: "message" },
    ]);
  });

  it("does not dispatch an unterminated final event", () => {
    expect(parseSse("data: incomplete\n")).toStrictEqual([]);
  });
});

describe(feedSse, () => {
  it("gives the same events however the stream is chunked", () => {
    const expected = parseSse(STREAM);
    for (let size = 1; size <= STREAM.length; size += 1) {
      const chunks: string[] = [];
      for (let start = 0; start < STREAM.length; start += size) {
        chunks.push(STREAM.slice(start, start + size));
      }
      expect(feedAll(chunks).events).toStrictEqual(expected);
    }
  });

  it("treats a CRLF split across chunks as one line break", () => {
    const { events } = feedAll(["data: a\r", "\ndata: b\r", "\n\r", "\n"]);
    expect(events).toStrictEqual([
      { data: "a\nb", lastEventId: "", type: "message" },
    ]);
  });

  it("strips a leading BOM only at the start of the stream", () => {
    const { events } = feedAll([BOM, "data: a\n\n", `${BOM}data: b\n\n`]);
    expect(events.map((event) => event.data)).toStrictEqual(["a"]);
  });

  it("records retry and keeps the last event id", () => {
    const { state } = feedAll([
      "retry: 5000\nretry: x\nid: 7\n",
      "data: a\n\n",
    ]);
    expect(state.retryMs).toBe(5000);
    expect(state.lastEventId).toBe("7");
  });

  it("keeps an incomplete line pending", () => {
    const first = feedSse(initialSseState, "data: hel");
    expect(first.events).toStrictEqual([]);
    expect(feedSse(first.state, "lo\n\n").events[0]?.data).toBe("hello");
  });
});
