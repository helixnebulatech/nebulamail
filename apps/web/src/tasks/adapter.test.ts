// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import {
  AccountId,
  CalendarEventId,
  CalendarId,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { CalendarSummary } from "../calendar/model";
import { methodError, scriptedJmap } from "../test/scripted-jmap";
import type { Handler } from "../test/scripted-jmap";
import { calendarTaskAdapter, shareBudget } from "./adapter";

const calendar = (accountId: string, id: string): CalendarSummary => ({
  accountId: AccountId.parse(accountId),
  color: "#123456",
  id: CalendarId.parse(id),
  isDefault: false,
  key: `${accountId}/${id}`,
  mayDelete: true,
  mayWrite: true,
  name: id,
  owner: null,
  sortOrder: 0,
  visible: true,
});

interface Stored {
  readonly id: string;
  readonly "@type": "Task" | "Event";
  readonly calendarIds: Record<string, boolean>;
  readonly recurrenceRule?: { readonly frequency: string };
  readonly title: string;
}

/**
 * Account `a`: 600 tasks among 100 events, two of them repeating (one in
 * c1, one in c2), each with two occurrences in the window. Account `b`:
 * 10 tasks, or failing.
 */
const accounts = (): Record<string, Stored[]> => {
  const a: Stored[] = [];
  for (let index = 0; index < 700; index += 1) {
    const isEvent = index % 7 === 3;
    a.push({
      "@type": isEvent ? "Event" : "Task",
      calendarIds: { [index === 11 ? "c2" : "c1"]: true },
      id: `a${String(index).padStart(3, "0")}`,
      title: `a${index}`,
      ...(index === 0 || index === 11
        ? { recurrenceRule: { frequency: "weekly" } }
        : {}),
    });
  }
  const b: Stored[] = Array.from({ length: 10 }, (_, index) => ({
    "@type": "Task" as const,
    calendarIds: { c9: true },
    id: `b${index}`,
    title: `b${index}`,
  }));
  return { a, b };
};

const serve = (failing: ReadonlySet<string> = new Set()) => {
  const data = accounts();
  const occurrences = (accountId: string, calendarId: string) =>
    (data[accountId] ?? []).flatMap((item) =>
      item.recurrenceRule !== undefined && item.calendarIds[calendarId] === true
        ? [
            { baseEventId: item.id, id: `${item.id}-1` },
            { baseEventId: item.id, id: `${item.id}-2` },
          ]
        : []
    );
  let lastIds: string[] = [];
  const query: Handler = (args, callId) => {
    const accountId = String(args.accountId);
    if (failing.has(accountId)) {
      return [methodError("forbidden", callId)];
    }
    const inCalendar =
      typeof args.filter === "object" &&
      args.filter !== null &&
      "inCalendar" in args.filter
        ? String(args.filter.inCalendar)
        : null;
    if (inCalendar !== null) {
      lastIds = [
        ...occurrences(accountId, inCalendar).map((item) => item.id),
        "some-event",
      ];
      return [
        [
          "CalendarEvent/query",
          {
            accountId,
            canCalculateChanges: false,
            ids: lastIds,
            position: 0,
            queryState: "q",
          },
          callId,
        ],
      ];
    }
    const all = data[accountId] ?? [];
    const position = Number(args.position);
    lastIds = all
      .slice(position, position + Number(args.limit))
      .map((item) => item.id);
    return [
      [
        "CalendarEvent/query",
        {
          accountId,
          canCalculateChanges: false,
          ids: lastIds,
          position,
          queryState: "q",
          total: all.length,
        },
        callId,
      ],
    ];
  };
  const get: Handler = (args, callId) => {
    const accountId = String(args.accountId);
    const all = data[accountId] ?? [];
    const ids =
      "#ids" in args || !Array.isArray(args.ids)
        ? lastIds
        : args.ids.map(String);
    const list = ids.flatMap((id) => {
      const stored = all.find((item) => item.id === id);
      if (stored !== undefined) {
        return [stored];
      }
      const occurrence = all
        .flatMap((item) =>
          occurrences(accountId, Object.keys(item.calendarIds)[0] ?? "")
        )
        .find((item) => item.id === id);
      if (occurrence !== undefined) {
        return [
          {
            "@type": "Task",
            ...occurrence,
            recurrenceId: "2026-11-09T00:00:00",
          },
        ];
      }
      return id === "some-event" ? [{ "@type": "Event", id }] : [];
    });
    return [
      [
        "CalendarEvent/get",
        { accountId, list, notFound: [], state: `${accountId}-state` },
        callId,
      ],
    ];
  };
  return scriptedJmap({
    handlers: { "CalendarEvent/get": get, "CalendarEvent/query": query },
  });
};

const load = async (failing: ReadonlySet<string> = new Set()) => {
  const scripted = serve(failing);
  const client = await scripted.connect();
  const adapter = calendarTaskAdapter({
    calendars: async () =>
      await Promise.resolve([
        calendar("a", "c1"),
        calendar("a", "c2"),
        calendar("b", "c9"),
      ]),
    client: async () => await Promise.resolve(client),
    savedLists: () => [],
  });
  return {
    scripted,
    snapshot: await adapter.load({
      display: "UTC",
      now: Date.parse("2026-11-08T12:00:00Z"),
    }),
  };
};

describe("calendarTaskAdapter.load", () => {
  it("reads every account and batch in few requests", async () => {
    const { scripted } = await load();
    // a: 2 pages of ids, 1 request for 600 tasks (2 calls), the two
    // calendars' occurrences at once (2), their objects (1); b: 1 + 1.
    expect(scripted.usings).toHaveLength(8);
    // Full objects (by ids, in the user's zone); accounts run side by side.
    const fullGets = scripted.calls.filter(
      ([name, args]) =>
        name === "CalendarEvent/get" &&
        Array.isArray(args.ids) &&
        args.timeZone !== undefined
    );
    expect(
      fullGets
        .map(([, args]) => (Array.isArray(args.ids) ? args.ids.length : 0))
        .toSorted((left, right) => left - right)
    ).toStrictEqual([4, 10, 100, 500]);
  });

  it("keeps a fixed order and every account's state", async () => {
    const { snapshot } = await load();
    const ids = snapshot.tasks.map((task) => task.id);
    // Occurrences first (calendar by calendar), then the rest, account by account.
    expect(ids.slice(0, 4)).toStrictEqual([
      "a000-1",
      "a000-2",
      "a011-1",
      "a011-2",
    ]);
    expect(ids.slice(-10)).toStrictEqual(
      Array.from({ length: 10 }, (_, index) => `b${index}`)
    );
    expect([snapshot.tasks.length, snapshot.stored.size]).toStrictEqual([
      4 + 598 + 10,
      610,
    ]);
    expect(snapshot.states).toStrictEqual(
      new Map([
        ["a", "a-state"],
        ["b", "b-state"],
      ])
    );
  });

  it("leaves out a shared account that fails", async () => {
    const { snapshot } = await load(new Set(["b"]));
    expect(snapshot.tasks.every((task) => task.accountId === "a")).toBeTruthy();
    expect(snapshot.states.has("b")).toBeFalsy();
  });

  it("fails when the user's own account fails", async () => {
    await expect(load(new Set(["a"]))).rejects.toThrow(/forbidden/u);
  });
});

const share = (accountId: string, count: number) => ({
  accountId: AccountId.parse(accountId),
  ids: Array.from({ length: count }, (_, index) =>
    CalendarEventId.parse(`${accountId}${index}`)
  ),
  state: "s",
});

describe(shareBudget, () => {
  it("gives each account what the ones before it left", () => {
    const result = shareBudget(
      [share("a", 6), share("b", 6), share("c", 1)],
      10
    );
    expect(result.shares.map((item) => item.ids.length)).toStrictEqual([6, 4]);
    expect(result.truncated).toBeTruthy();
  });

  it("skips accounts that failed without spending the budget", () => {
    const result = shareBudget([share("a", 3), null, share("c", 3)], 10);
    expect(result.shares.map((item) => item.accountId)).toStrictEqual([
      "a",
      "c",
    ]);
    expect(result.truncated).toBeFalsy();
  });

  it("counts reaching the limit exactly as truncated, as before", () => {
    expect(shareBudget([share("a", 10)], 10).truncated).toBeTruthy();
    expect(shareBudget([share("a", 9)], 10).truncated).toBeFalsy();
  });
});
