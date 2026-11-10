// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, CalendarEventId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { RawTask } from "./model";
import {
  dueOf,
  occurrenceDue,
  taskProgress,
  toPriority,
  toTask,
} from "./model";

const accountId = AccountId.parse("c");
const id = (value: string) => CalendarEventId.parse(value);
const context = { accountId, display: "Europe/Madrid" };

describe(toPriority, () => {
  it("maps RFC 5545 priorities to four levels", () => {
    expect([0, 1, 4, 5, 6, 9, 10, null].map(toPriority)).toStrictEqual([
      "none",
      "high",
      "high",
      "medium",
      "low",
      "low",
      "none",
      "none",
    ]);
  });
});

describe(dueOf, () => {
  it("reads a day or a time in a zone", () => {
    expect(
      dueOf({ due: "2026-11-05T00:00:00", id: id("a"), showWithoutTime: true })
    ).toStrictEqual({ date: "2026-11-05", kind: "date" });
    expect(
      dueOf({
        due: "2026-11-06T17:30:00",
        id: id("b"),
        timeZone: "Europe/Madrid",
      })
    ).toStrictEqual({
      kind: "datetime",
      local: "2026-11-06T17:30:00",
      timeZone: "Europe/Madrid",
    });
    expect(dueOf({ id: id("c") })).toBeNull();
  });
});

// As Stalwart returns them.
const done: RawTask = {
  "@type": "Task",
  calendarIds: { cj: true },
  iCalendar: {
    name: "vtodo",
    properties: [
      ["completed", {}, "unknown", "20261026T090000Z"],
      ["x-apple-sort-order", {}, "unknown", "778899"],
    ],
  },
  id: id("ho"),
  priority: 5,
  progress: "completed",
  relatedTo: { "parent-1": { relation: { parent: true } } },
  title: " From CalDAV ",
  uid: "caldav-1",
  utcStart: "32548-12-04T15:30:07Z",
};

describe(toTask, () => {
  it("maps a stored task", () => {
    expect(toTask(done, undefined, context)).toMatchObject({
      baseId: "ho",
      completed: true,
      completedAt: Date.UTC(2026, 9, 26, 9),
      due: null,
      dueAt: null,
      listKey: "c/cj",
      parentUid: "parent-1",
      priority: "medium",
      recurrenceId: null,
      recurring: false,
      sortOrder: 778_899,
      title: "From CalDAV",
    });
  });

  it("is null for events", () => {
    expect(
      toTask({ "@type": "Event", id: id("e") }, undefined, context)
    ).toBeNull();
    expect(toTask({ id: id("e") }, undefined, context)).toBeNull();
  });

  it("reads alerts: absolute, and offsets from the due or the start", () => {
    const task = toTask(
      {
        "@type": "Task",
        alerts: {
          abs: { trigger: { when: "2026-11-06T16:00:00Z" } },
          end: { trigger: { offset: "-PT15M", relativeTo: "end" } },
          start: { trigger: { offset: "-PT1H" } },
        },
        due: "2026-11-06T17:30:00",
        id: id("t"),
        timeZone: "Europe/Madrid",
      },
      undefined,
      context
    );
    // 17:30 in Madrid is 16:30 UTC; no start: Stalwart counts from the due.
    expect(task?.alerts).toStrictEqual([
      { at: Date.UTC(2026, 10, 6, 16), key: "abs", minutesBefore: 30 },
      { at: Date.UTC(2026, 10, 6, 16, 15), key: "end", minutesBefore: 15 },
      { at: Date.UTC(2026, 10, 6, 15, 30), key: "start", minutesBefore: 60 },
    ]);
  });
});

describe(occurrenceDue, () => {
  const series: RawTask = {
    "@type": "Task",
    due: "2026-11-02T10:00:00",
    id: id("gu"),
    recurrenceRule: { count: 5, frequency: "weekly" },
    start: "2026-11-02T09:00:00",
    timeZone: "Europe/Madrid",
  };

  it("moves the due with the occurrence (Stalwart leaves it alone)", () => {
    // The occurrence of 9 Nov as Stalwart expands it: `due` unchanged.
    const occurrence: RawTask = {
      "@type": "Task",
      baseEventId: id("gu"),
      due: "2026-11-02T10:00:00",
      id: id("h1qrhuaaaagu"),
      recurrenceId: "2026-11-09T09:00:00",
      start: "2026-11-09T09:00:00",
      timeZone: "Europe/Madrid",
      utcStart: "2026-11-09T08:00:00Z",
    };
    expect(occurrenceDue(occurrence, series, "UTC")).toStrictEqual({
      kind: "datetime",
      local: "2026-11-09T10:00:00",
      timeZone: "Europe/Madrid",
    });
    expect(toTask(occurrence, series, context)).toMatchObject({
      baseId: "gu",
      dueAt: Date.UTC(2026, 10, 9, 9),
      recurrenceId: "2026-11-09T09:00:00",
      recurring: true,
    });
  });

  it("works for days, and for a series with only a due", () => {
    const days: RawTask = {
      ...series,
      due: "2026-11-02T00:00:00",
      showWithoutTime: true,
      start: "2026-11-02T00:00:00",
      timeZone: null,
    };
    expect(
      occurrenceDue(
        { id: id("o1"), start: "2026-11-04T00:00:00", timeZone: "Etc/UTC" },
        days,
        "Europe/Madrid"
      )
    ).toStrictEqual({ date: "2026-11-04", kind: "date" });
    const dueOnly: RawTask = { ...series, start: null };
    expect(
      occurrenceDue(
        {
          id: id("o2"),
          start: "2026-11-03T09:00:00",
          timeZone: "Etc/UTC",
          utcStart: "2026-11-03T09:00:00Z",
        },
        dueOnly,
        "UTC"
      )
    ).toStrictEqual({
      kind: "datetime",
      local: "2026-11-03T10:00:00",
      timeZone: "Europe/Madrid",
    });
  });
});

const people = (...values: (string | undefined)[]) => ({
  participants: Object.fromEntries(
    values.map((progress, index) => [
      `p${String(index)}`,
      progress === undefined ? {} : { progress },
    ])
  ),
});

describe(taskProgress, () => {
  // RFC 8984 §5.2.5, in order of evaluation. One without progress isn't
  // "completed"; no participants at all: the default.
  it.each([
    [["completed", "completed"], "completed"],
    [["completed", "failed", "in-process"], "failed"],
    [["completed", "in-process"], "in-process"],
    [["completed", undefined], "needs-action"],
    [[], "needs-action"],
  ])("derives an omitted progress from %j: %s", (values, progress) => {
    expect(taskProgress(people(...values))).toBe(progress);
  });

  it("keeps a progress the task sets", () => {
    expect(
      taskProgress({ ...people("completed"), progress: "In-Process" })
    ).toBe("in-process");
  });

  it("shows a task done when all its participants completed it", () => {
    const task = toTask(
      {
        "@type": "Task",
        id: id("t1"),
        participants: {
          a: { progress: "completed" },
          b: { progress: "completed" },
        },
        title: "Informe",
      },
      undefined,
      context
    );
    expect(task?.completed).toBeTruthy();
    expect(task?.progress).toBe("completed");
  });
});
