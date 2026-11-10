// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { parseDate } from "@internationalized/date";
import { CalendarId } from "@nebulamail/jmap-client";
import { describe, expect, it, vi } from "vitest";

import type { Invocation } from "../test/scripted-jmap";
import { scriptedJmap } from "../test/scripted-jmap";
import { expandDays, expandWindows, nextPosition } from "./paging";

const jmap = vi.hoisted(() => ({
  connect: null as null | (() => Promise<unknown>),
}));

vi.mock(import("../mail/client"), async () => {
  const { JmapClient } = await import("@nebulamail/jmap-client");
  return {
    getClient: async () => {
      const client = await jmap.connect?.();
      if (!(client instanceof JmapClient)) {
        throw new TypeError("no scripted client");
      }
      return client;
    },
    resetClient: vi.fn<() => void>(),
  };
});

const { calendarContents } = await import("./api");

describe(nextPosition, () => {
  it("follows the total when there is one", () => {
    expect(nextPosition({ ids: [1, 2], position: 0, total: 5 }, 10)).toBe(2);
    expect(nextPosition({ ids: [1], position: 4, total: 5 }, 10)).toBeNull();
  });

  it("without a total, a short page is the last only under the server's limit", () => {
    // RFC 8620 §5.5: the server capped the limit at 2 and says so.
    expect(nextPosition({ ids: [1, 2], limit: 2, position: 0 }, 10)).toBe(2);
    expect(nextPosition({ ids: [1], limit: 2, position: 2 }, 10)).toBeNull();
    expect(nextPosition({ ids: [1, 2], position: 0 }, 10)).toBeNull();
    expect(nextPosition({ ids: [], position: 0, total: 9 }, 10)).toBeNull();
  });
});

describe(expandDays, () => {
  // Stalwart 1.0.1 says P52W1D (365 days); a day short for DST.
  it.each([
    ["P52W1D", 364],
    ["P7D", 6],
    ["PT36H", null],
    ["P1Y", null],
    [undefined, null],
    ["nonsense", null],
  ])("reads %j as %j days", (duration, days) => {
    expect(expandDays(duration)).toBe(days);
  });
});

describe(expandWindows, () => {
  const day = parseDate;

  it("splits a range into windows no longer than allowed", () => {
    expect(
      expandWindows(day("2026-01-01"), day("2026-01-08"), 3).map(
        ([from, to]) => `${from.toString()}/${to.toString()}`
      )
    ).toStrictEqual([
      "2026-01-01/2026-01-04",
      "2026-01-04/2026-01-07",
      "2026-01-07/2026-01-08",
    ]);
    expect(
      expandWindows(day("2026-01-01"), day("2026-01-08"), null)
    ).toHaveLength(1);
  });
});

describe(calendarContents, () => {
  it("counts every object when the server caps the page (RFC 8620 §5.5)", async () => {
    // Five tasks; the server never gives more than two ids per page.
    const all = ["t1", "t2", "t3", "t4", "t5"];
    // The get's `#ids` refer to the query in the same request.
    let page: string[] = [];
    const scripted = scriptedJmap({
      handlers: {
        "CalendarEvent/get": (_args, callId): Invocation[] => {
          const ids = page;
          return [
            [
              "CalendarEvent/get",
              {
                accountId: "a",
                list: ids.map((id) => ({ "@type": "Task", id })),
                notFound: [],
                state: "1",
              },
              callId,
            ],
          ];
        },
        "CalendarEvent/query": (args, callId): Invocation[] => {
          const position =
            typeof args.position === "number" ? args.position : 0;
          page = all.slice(position, position + 2);
          return [
            [
              "CalendarEvent/query",
              {
                accountId: "a",
                canCalculateChanges: false,
                ids: page,
                limit: 2,
                position,
                queryState: "q",
              },
              callId,
            ],
          ];
        },
      },
    });
    const client = await scripted.connect();
    jmap.connect = async () => await Promise.resolve(client);
    const contents = await calendarContents({
      accountId: client.primaryAccountId("urn:ietf:params:jmap:calendars"),
      id: CalendarId.parse("cal1"),
    });
    expect(contents).toStrictEqual({ complete: true, others: 0, tasks: 5 });
  });
});
