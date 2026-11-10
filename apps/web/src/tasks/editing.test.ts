// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import {
  AccountId,
  CalendarEventId,
  CalendarId,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  alertsPatch,
  completeChange,
  duePatch,
  estimatePatch,
  inversePatch,
  mapPatch,
  newTaskProperties,
  parentPatch,
  recurrencePatch,
  sortOrderPatch,
  undoOf,
} from "./editing";
import type { RawTask, Task } from "./model";
import { estimatedMinutesOf, toTask } from "./model";

const accountId = AccountId.parse("c");
const id = (value: string) => CalendarEventId.parse(value);
const display = "Europe/Madrid";
const context = { accountId, display };
const now = Date.UTC(2026, 9, 27, 10);
const options = { now };
/** `now` as RFC 8984 §1.4.4 UTCDateTime (progressUpdated). */
const STAMP = "2026-10-27T10:00:00Z";

const mapped = (raw: RawTask, series?: RawTask): Task => {
  const task = toTask(raw, series, context);
  if (task === null) {
    throw new Error("not a task");
  }
  return task;
};

describe(mapPatch, () => {
  it("patches keys of a map that is there", () => {
    expect(mapPatch("alerts", { a: {} }, { a: null, b: 1 })).toStrictEqual({
      "alerts/a": null,
      "alerts/b": 1,
    });
  });

  it("writes the whole map when it isn't there or keys need escaping", () => {
    expect(mapPatch("relatedTo", undefined, { p: 1, q: null })).toStrictEqual({
      relatedTo: { p: 1 },
    });
    expect(
      mapPatch("relatedTo", { "a/b": 1, c: 2 }, { "a/b": null })
    ).toStrictEqual({ relatedTo: { c: 2 } });
    expect(
      mapPatch("relatedTo", { x: 1 }, { x: null, "y~": null })
    ).toStrictEqual({
      relatedTo: null,
    });
  });
});

describe(newTaskProperties, () => {
  it("makes a task with a day, a priority, a parent and links", () => {
    expect(
      newTaskProperties(
        {
          calendarId: CalendarId.parse("l1"),
          due: { date: "2026-11-05", kind: "date" },
          links: [{ href: "mid:abc@example.com" }],
          notes: "  ",
          parentUid: "parent-1",
          priority: "high",
          title: " Pay ",
        },
        { display, uid: "u1" }
      )
    ).toStrictEqual({
      "@type": "Task",
      calendarIds: { l1: true },
      due: "2026-11-05T00:00:00",
      links: {
        l1: {
          "@type": "Link",
          href: "mid:abc@example.com",
          rel: "describedby",
        },
      },
      priority: 1,
      progress: "needs-action",
      relatedTo: {
        "parent-1": { "@type": "Relation", relation: { parent: true } },
      },
      showWithoutTime: true,
      title: "Pay",
      uid: "u1",
    });
  });

  it("gives a single task absolute alerts, a repeating one offsets", () => {
    const due = {
      kind: "datetime",
      local: "2026-11-06T17:30:00",
      timeZone: "Europe/Madrid",
    } as const;
    const single = newTaskProperties(
      {
        alerts: [{ key: "a", minutesBefore: 30 }],
        calendarId: CalendarId.parse("l1"),
        due,
        title: "Call",
      },
      { display, uid: "u2" }
    );
    expect(single.alerts).toStrictEqual({
      a: {
        "@type": "Alert",
        action: "display",
        trigger: { "@type": "AbsoluteTrigger", when: "2026-11-06T16:00:00Z" },
      },
    });
    const repeating = newTaskProperties(
      {
        alerts: [{ key: "a", minutesBefore: 30 }],
        calendarId: CalendarId.parse("l1"),
        due,
        recurrenceRule: { frequency: "weekly" },
        title: "Call",
      },
      { display, uid: "u3" }
    );
    expect(repeating).toMatchObject({
      alerts: {
        a: {
          trigger: {
            "@type": "OffsetTrigger",
            offset: "-PT30M",
            relativeTo: "end",
          },
        },
      },
      recurrenceRule: { frequency: "weekly" },
      start: "2026-11-06T17:30:00",
      timeZone: "Europe/Madrid",
    });
  });
});

const plain: RawTask = {
  "@type": "Task",
  calendarIds: { l1: true },
  id: id("t1"),
  title: "Plain",
  uid: "t1@uid",
};

describe("completing", () => {
  it("sets progress and COMPLETED, writing iCalendar when there is none", () => {
    expect(completeChange(mapped(plain), plain, true, options)).toStrictEqual({
      accountId,
      update: {
        t1: {
          iCalendar: {
            name: "vtodo",
            properties: [["completed", {}, "date-time", "20261027T100000Z"]],
          },
          progress: "completed",
          progressUpdated: STAMP,
        },
      },
    });
  });

  it("keeps the other iCalendar properties and removes COMPLETED again", () => {
    const done: RawTask = {
      ...plain,
      iCalendar: {
        properties: [
          ["x-apple-sort-order", {}, "unknown", "5"],
          ["completed", {}, "unknown", "20261026T090000Z"],
        ],
      },
      percentComplete: 100,
      progress: "completed",
    };
    expect(
      completeChange(mapped(done), done, false, options).update
    ).toStrictEqual({
      t1: {
        "iCalendar/properties": [["x-apple-sort-order", {}, "unknown", "5"]],
        percentComplete: 0,
        progress: "needs-action",
        progressUpdated: STAMP,
      },
    });
  });

  const series: RawTask = {
    ...plain,
    due: "2026-11-02T10:00:00",
    id: id("gu"),
    recurrenceRule: { count: 5, frequency: "weekly" },
    start: "2026-11-02T10:00:00",
    timeZone: "Europe/Madrid",
  };
  const occurrence = (rid: string): RawTask => ({
    "@type": "Task",
    baseEventId: id("gu"),
    id: id(`occ-${rid.slice(0, 10)}`),
    recurrenceId: rid,
    start: rid,
    utcStart: "2026-11-09T09:00:00Z",
  });

  it("completes one occurrence in recurrenceOverrides", () => {
    const task = mapped(occurrence("2026-11-09T10:00:00"), series);
    expect(completeChange(task, series, true, options).update).toStrictEqual({
      gu: {
        recurrenceOverrides: {
          // With its start (support.stalw.art/t/2007).
          "2026-11-09T10:00:00": {
            progress: "completed",
            progressUpdated: STAMP,
            start: "2026-11-09T10:00:00",
          },
        },
      },
    });
    const withOverrides: RawTask = {
      ...series,
      recurrenceOverrides: {
        "2026-11-02T10:00:00": { progress: "completed" },
        "2026-11-09T10:00:00": { progress: "completed", updated: "x" },
      },
    };
    expect(
      completeChange(task, withOverrides, false, options).update
    ).toStrictEqual({
      gu: { "recurrenceOverrides/2026-11-09T10:00:00": null },
    });
    expect(
      completeChange(
        mapped(occurrence("2026-11-16T10:00:00"), withOverrides),
        withOverrides,
        true,
        options
      ).update
    ).toStrictEqual({
      gu: {
        "recurrenceOverrides/2026-11-16T10:00:00": {
          progress: "completed",
          progressUpdated: STAMP,
          start: "2026-11-16T10:00:00",
        },
      },
    });
  });

  it("adds the start to an existing override that has none", () => {
    const moved: RawTask = {
      ...series,
      recurrenceOverrides: {
        "2026-11-09T10:00:00": { title: "Renamed" },
      },
    };
    expect(
      completeChange(
        mapped(occurrence("2026-11-09T10:00:00"), moved),
        moved,
        true,
        options
      ).update
    ).toStrictEqual({
      gu: {
        "recurrenceOverrides/2026-11-09T10:00:00/progress": "completed",
        "recurrenceOverrides/2026-11-09T10:00:00/progressUpdated": STAMP,
        "recurrenceOverrides/2026-11-09T10:00:00/start": "2026-11-09T10:00:00",
      },
    });
  });

  it("completes one day of an all-day series with an override, like any other", () => {
    const days: RawTask = {
      ...series,
      due: "2026-11-02T00:00:00",
      showWithoutTime: true,
      start: "2026-11-02T00:00:00",
      timeZone: null,
    };
    const task = mapped(
      { ...occurrence("2026-11-03T00:00:00"), start: "2026-11-03T00:00:00" },
      days
    );
    // Stalwart and the bridge keep the series.
    expect(completeChange(task, days, true, options)).toStrictEqual({
      accountId,
      update: {
        gu: {
          recurrenceOverrides: {
            "2026-11-03T00:00:00": {
              progress: "completed",
              progressUpdated: STAMP,
              start: "2026-11-03T00:00:00",
            },
          },
        },
      },
    });
  });
});

describe("undo", () => {
  it("restores each path, deletes what was created, recreates the deleted", () => {
    const raw: RawTask = {
      ...plain,
      alerts: { a: { trigger: {} } },
      priority: 5,
    };
    const patch = { "alerts/a": null, priority: 1, title: "New" };
    expect(inversePatch(raw, patch)).toStrictEqual({
      "alerts/a": { trigger: {} },
      priority: 5,
      title: "Plain",
    });
    const undo = undoOf(
      { accountId, destroy: [id("t1")], update: { [id("t1")]: patch } },
      new Map([[id("t1"), { ...raw, utcStart: "2026-11-05T00:00:00Z" }]]),
      { copy: id("new1") }
    );
    expect(undo.destroy).toStrictEqual(["new1"]);
    expect(undo.create?.["undo-t1"]).toStrictEqual({
      "@type": "Task",
      alerts: { a: { trigger: {} } },
      calendarIds: { l1: true },
      priority: 5,
      title: "Plain",
      uid: "t1@uid",
    });
  });
});

describe(parentPatch, () => {
  it("swaps the parent and keeps other relations", () => {
    const raw: RawTask = {
      ...plain,
      relatedTo: {
        next: { relation: { next: true } },
        old: { relation: { parent: true } },
      },
    };
    expect(parentPatch(raw, "new")).toStrictEqual({
      "relatedTo/new": { "@type": "Relation", relation: { parent: true } },
      "relatedTo/old": null,
    });
    expect(parentPatch(plain, "p")).toStrictEqual({
      relatedTo: { p: { "@type": "Relation", relation: { parent: true } } },
    });
    expect(parentPatch(raw, null)).toStrictEqual({ "relatedTo/old": null });
  });
});

describe(duePatch, () => {
  it("moves a repeating task's start and its alerts with the due", () => {
    const raw: RawTask = {
      ...plain,
      alerts: {
        a: {
          acknowledged: "2026-10-01T00:00:00Z",
          action: "display",
          trigger: { offset: "-PT15M", relativeTo: "end" },
        },
      },
      due: "2026-11-02T10:00:00",
      recurrenceRule: { frequency: "daily" },
      start: "2026-11-02T10:00:00",
      timeZone: "Europe/Madrid",
    };
    const patch = duePatch(
      mapped(raw),
      raw,
      {
        kind: "datetime",
        local: "2026-11-03T09:00:00",
        timeZone: "Europe/Madrid",
      },
      display
    );
    expect(patch).toStrictEqual({
      due: "2026-11-03T09:00:00",
      showWithoutTime: false,
      start: "2026-11-03T09:00:00",
      timeZone: "Europe/Madrid",
    });
  });

  it("moves alerts tied to the due (to the day at 9:00), and a day drops the zone", () => {
    const raw: RawTask = {
      ...plain,
      alerts: {
        a: { action: "display", trigger: { when: "2026-11-06T16:00:00Z" } },
      },
      due: "2026-11-06T17:30:00",
      timeZone: "Europe/Madrid",
    };
    expect(
      duePatch(mapped(raw), raw, { date: "2026-11-07", kind: "date" }, display)
    ).toStrictEqual({
      "alerts/a": {
        action: "display",
        // 90 minutes before 17:30 → on the day at 9:00 (the default).
        trigger: { "@type": "AbsoluteTrigger", when: "2026-11-07T08:00:00Z" },
      },
      due: "2026-11-07T00:00:00",
      showWithoutTime: true,
      timeZone: null,
    });
  });

  it("ends the repetition when the due goes", () => {
    const raw: RawTask = {
      ...plain,
      due: "2026-11-02T00:00:00",
      recurrenceRule: { frequency: "daily" },
      showWithoutTime: true,
      start: "2026-11-02T00:00:00",
    };
    expect(duePatch(mapped(raw), raw, null, display)).toStrictEqual({
      due: null,
      recurrenceOverrides: null,
      recurrenceRule: null,
      start: null,
    });
  });
});

describe(alertsPatch, () => {
  it("adds, removes and keeps an alert's other properties", () => {
    const raw: RawTask = {
      ...plain,
      alerts: {
        gone: { trigger: { when: "2026-11-01T00:00:00Z" } },
        kept: {
          acknowledged: "x",
          trigger: { "@type": "AbsoluteTrigger", when: "2026-11-06T16:00:00Z" },
        },
      },
      due: "2026-11-06T17:30:00",
      timeZone: "Europe/Madrid",
    };
    const patch = alertsPatch(
      raw,
      [
        { key: "kept", minutesBefore: 30 },
        { key: "new", at: Date.UTC(2026, 10, 6, 8) },
      ],
      {
        kind: "datetime",
        local: "2026-11-06T17:30:00",
        timeZone: "Europe/Madrid",
      },
      display
    );
    expect(patch).toStrictEqual({
      "alerts/gone": null,
      "alerts/new": {
        "@type": "Alert",
        action: "display",
        trigger: { "@type": "AbsoluteTrigger", when: "2026-11-06T08:00:00Z" },
      },
    });
  });
});

describe(recurrencePatch, () => {
  it("anchors the start on the due and turns alerts into offsets", () => {
    const raw: RawTask = {
      ...plain,
      alerts: {
        a: { action: "display", trigger: { when: "2026-11-06T16:00:00Z" } },
      },
      due: "2026-11-06T17:30:00",
      timeZone: "Europe/Madrid",
    };
    expect(
      recurrencePatch(mapped(raw), raw, { frequency: "weekly" }, display)
    ).toStrictEqual({
      "alerts/a": {
        action: "display",
        trigger: {
          "@type": "OffsetTrigger",
          offset: "-PT30M",
          relativeTo: "end",
        },
      },
      recurrenceRule: { frequency: "weekly" },
      start: "2026-11-06T17:30:00",
    });
    expect(
      recurrencePatch(mapped(plain), plain, { frequency: "weekly" }, display)
    ).toBeNull();
  });
});

const offset = (value: string) => ({
  action: "display",
  trigger: { "@type": "OffsetTrigger", offset: value, relativeTo: "end" },
});

describe("reminders of a day task", () => {
  const dayTask: RawTask = {
    "@type": "Task",
    calendarIds: { l1: true },
    due: "2026-11-06T00:00:00",
    id: id("d1"),
    showWithoutTime: true,
    title: "Day",
    uid: "d1@uid",
  };

  it("writes the day before at 9:00 as an absolute time and reads it back", () => {
    const patch = alertsPatch(
      dayTask,
      [{ key: "a", minutesBefore: 900 }],
      { date: "2026-11-06", kind: "date" },
      display
    );
    expect(patch).toStrictEqual({
      alerts: {
        a: {
          "@type": "Alert",
          action: "display",
          trigger: { "@type": "AbsoluteTrigger", when: "2026-11-05T08:00:00Z" },
        },
      },
    });
    const task = mapped({
      ...dayTask,
      alerts: {
        a: {
          action: "display",
          trigger: { "@type": "AbsoluteTrigger", when: "2026-11-05T08:00:00Z" },
        },
      },
    });
    expect(task.alerts.map((alert) => alert.minutesBefore)).toStrictEqual([
      900,
    ]);
  });

  it("writes offsets from the due for a repeating one (-PT15H, PT9H)", () => {
    const series: RawTask = {
      ...dayTask,
      recurrenceRule: { frequency: "weekly" },
      start: "2026-11-06T00:00:00",
    };
    const patch = alertsPatch(
      series,
      [
        { key: "a", minutesBefore: 900 },
        { key: "b", minutesBefore: -540 },
        { key: "c", minutesBefore: 2340 },
      ],
      { date: "2026-11-06", kind: "date" },
      display
    );
    expect(patch).toStrictEqual({
      alerts: {
        a: {
          "@type": "Alert",
          action: "display",
          trigger: {
            "@type": "OffsetTrigger",
            offset: "-PT15H",
            relativeTo: "end",
          },
        },
        b: {
          "@type": "Alert",
          action: "display",
          trigger: {
            "@type": "OffsetTrigger",
            offset: "PT9H",
            relativeTo: "end",
          },
        },
        c: {
          "@type": "Alert",
          action: "display",
          trigger: {
            "@type": "OffsetTrigger",
            offset: "-P1DT15H",
            relativeTo: "end",
          },
        },
      },
    });
    const task = mapped({
      ...series,
      alerts: { a: offset("-PT15H"), b: offset("PT9H"), c: offset("-P1DT15H") },
    });
    expect(task.alerts.map((alert) => alert.minutesBefore)).toStrictEqual([
      900, -540, 2340,
    ]);
  });

  it("keeps the days when a day gets a time", () => {
    const raw: RawTask = {
      ...dayTask,
      alerts: {
        a: { action: "display", trigger: { when: "2026-11-05T08:00:00Z" } },
      },
    };
    expect(
      duePatch(
        mapped(raw),
        raw,
        { kind: "datetime", local: "2026-11-06T17:00:00", timeZone: display },
        display
      )
    ).toMatchObject({
      // 1 day before at 9:00 → 1 day before 17:00.
      "alerts/a": { trigger: { when: "2026-11-05T16:00:00Z" } },
    });
  });
});

describe("manual order", () => {
  it("writes X-APPLE-SORT-ORDER, keeping other properties", () => {
    const bare: RawTask = { "@type": "Task", id: id("o1"), title: "o" };
    expect(sortOrderPatch(bare, 2048)).toStrictEqual({
      iCalendar: {
        name: "vtodo",
        properties: [["x-apple-sort-order", {}, "integer", 2048]],
      },
    });
    const withIcal: RawTask = {
      ...bare,
      iCalendar: {
        name: "vtodo",
        properties: [
          ["x-apple-sort-order", {}, "integer", 5],
          ["class", {}, "text", "PRIVATE"],
        ],
      },
    };
    expect(sortOrderPatch(withIcal, 7)).toStrictEqual({
      "iCalendar/properties": [
        ["class", {}, "text", "PRIVATE"],
        ["x-apple-sort-order", {}, "integer", 7],
      ],
    });
    expect(mapped({ ...withIcal }).sortOrder).toBe(5);
  });
});

describe("progressUpdated (RFC 8984 §5.2.6)", () => {
  const task: RawTask = {
    "@type": "Task",
    due: "2026-11-02T10:00:00",
    id: id("gu"),
    progress: "needs-action",
    recurrenceRule: { frequency: "weekly" },
    start: "2026-11-02T10:00:00",
    timeZone: "Europe/Madrid",
    title: "Weekly",
  };
  const occurrence = mapped(
    {
      "@type": "Task",
      baseEventId: id("gu"),
      id: id("occ"),
      recurrenceId: "2026-11-09T10:00:00",
      start: "2026-11-09T10:00:00",
      utcStart: "2026-11-09T09:00:00Z",
    },
    task
  );

  it("reopening an occurrence with other changes dates it in its override", () => {
    const series: RawTask = {
      ...task,
      recurrenceOverrides: {
        "2026-11-09T10:00:00": {
          progress: "completed",
          progressUpdated: "2026-11-01T08:00:00Z",
          start: "2026-11-09T10:00:00",
          title: "Renamed",
        },
      },
    };
    expect(
      completeChange(occurrence, series, false, options).update
    ).toStrictEqual({
      gu: {
        "recurrenceOverrides/2026-11-09T10:00:00/progress": "needs-action",
        "recurrenceOverrides/2026-11-09T10:00:00/progressUpdated": STAMP,
      },
    });
  });

  it("undo puts back the one there was, and never erases one it didn't see", () => {
    const seen: RawTask = { ...task, progressUpdated: "2026-11-01T08:00:00Z" };
    const done = completeChange(mapped(seen), seen, true, options);
    expect(
      undoOf(done, new Map([[seen.id, seen]]), {}).update?.[seen.id]
    ).toMatchObject({
      progress: "needs-action",
      progressUpdated: "2026-11-01T08:00:00Z",
    });
    // Stalwart 1.0.1 doesn't return it when asked by name.
    const hidden = inversePatch(task, {
      progress: "completed",
      progressUpdated: STAMP,
    });
    expect(hidden).toStrictEqual({ progress: "needs-action" });
    expect(
      inversePatch(task, {
        "recurrenceOverrides/2026-11-09T10:00:00/progressUpdated": STAMP,
      })
    ).toStrictEqual({});
  });
});

describe("estimated time (RFC 8984 §5.2.3)", () => {
  it("writes a positive Duration in hours and minutes (§1.4.6)", () => {
    expect(estimatePatch(90)).toStrictEqual({ estimatedDuration: "PT1H30M" });
    expect(estimatePatch(60)).toStrictEqual({ estimatedDuration: "PT1H" });
    expect(estimatePatch(45)).toStrictEqual({ estimatedDuration: "PT45M" });
    expect(estimatePatch(1500)).toStrictEqual({
      estimatedDuration: "PT25H",
    });
    expect(estimatePatch(10 ** 9)).toStrictEqual({
      estimatedDuration: "PT999H",
    });
  });

  it("removes it with null when there is nothing positive", () => {
    for (const minutes of [null, 0, -30, Number.NaN, 0.2]) {
      expect(estimatePatch(minutes)).toStrictEqual({ estimatedDuration: null });
    }
  });

  it("reads any positive Duration, and nothing else, as minutes", () => {
    const cases: [string | null | undefined, number | null][] = [
      ["PT1H30M", 90],
      ["PT45M", 45],
      ["P1D", 1440],
      ["P1W", 10_080],
      ["P1DT2H", 1560],
      ["PT0S", null],
      ["-PT1H", null],
      ["1h", null],
      ["", null],
      [null, null],
      [undefined, null],
    ];
    expect(cases.map(([value]) => estimatedMinutesOf(value))).toStrictEqual(
      cases.map(([, minutes]) => minutes)
    );
    expect(
      mapped({ ...plain, estimatedDuration: "PT2H" }).estimatedMinutes
    ).toBe(120);
    expect(mapped(plain).estimatedMinutes).toBeNull();
  });
});
