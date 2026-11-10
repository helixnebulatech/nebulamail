// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

import { describe, expect, it } from "vitest";

import { CALENDAR_METHODS } from "./calendars";
import {
  StoredTaskSchema,
  completedAt,
  completedProperty,
  formatICalUtc,
  icalProperties,
  icalValue,
  isTask,
  parentUids,
  parseICalUtc,
  taskUtc,
  withICalProperty,
} from "./tasks";

// A task as Stalwart returns it after a CalDAV client wrote it
//COMPLETED and X-APPLE-SORT-ORDER left as iCalendar.
const stored = {
  "@type": "Task",
  calendarIds: { cj: true },
  due: "2026-10-28T00:00:00",
  iCalendar: {
    convertedProperties: { due: { parameters: { value: "DATE" } } },
    name: "vtodo",
    properties: [
      ["completed", {}, "unknown", "20261026T090000Z"],
      ["x-apple-sort-order", {}, "unknown", "778899"],
    ],
  },
  id: "ho",
  percentComplete: 100,
  priority: 5,
  progress: "completed",
  relatedTo: { "parent-1": { relation: { parent: true } } },
  showWithoutTime: true,
  title: "From CalDAV",
  uid: "caldav-1",
  utcEnd: "32548-12-04T15:30:07Z",
  utcStart: "32548-12-04T15:30:07Z",
};

describe("stored tasks", () => {
  it("reads a stored task and keeps unknown properties", () => {
    const task = StoredTaskSchema.parse({ ...stored, "x:vendor": 1 });
    expect(task).toMatchObject({ due: "2026-10-28T00:00:00", priority: 5 });
    expect(Reflect.get(task, "x:vendor")).toBe(1);
  });

  it("doesn't fail on odd iCalendar entries or relations", () => {
    const task = StoredTaskSchema.parse({
      ...stored,
      iCalendar: {
        properties: [["completed"], 3, ...stored.iCalendar.properties],
      },
      relatedTo: "nonsense",
    });
    expect(icalProperties(task)).toHaveLength(2);
    expect(task.relatedTo).toBeNull();
  });

  it("is what CalendarEvent/get parses for a task", () => {
    const response = CALENDAR_METHODS["CalendarEvent/get"].parse(
      {},
      { accountId: "c", list: [stored], notFound: [], state: "s1" }
    );
    expect(response.list[0]).toMatchObject({
      iCalendar: { name: "vtodo" },
      progress: "completed",
      relatedTo: { "parent-1": { relation: { parent: true } } },
    });
  });
});

describe(isTask, () => {
  it("knows tasks by their type", () => {
    expect(isTask(stored)).toBeTruthy();
    expect(isTask({ "@type": "Event" })).toBeFalsy();
    expect(isTask({})).toBeFalsy();
  });
});

describe("iCalendar properties", () => {
  it("reads a value by name in any case", () => {
    expect(icalValue(stored, "X-APPLE-SORT-ORDER")).toBe("778899");
    expect(icalValue(stored, "class")).toBeNull();
    expect(icalValue({}, "completed")).toBeNull();
  });

  it("replaces or removes one property and keeps the rest", () => {
    const odd = { iCalendar: { properties: [["Completed", {}, "x", "1"], 7] } };
    expect(withICalProperty(odd, "completed", null)).toStrictEqual([7]);
    expect(
      withICalProperty(stored, "completed", completedProperty(0))
    ).toStrictEqual([
      ["x-apple-sort-order", {}, "unknown", "778899"],
      ["completed", {}, "date-time", "19700101T000000Z"],
    ]);
    expect(withICalProperty({}, "completed", null)).toStrictEqual([]);
  });
});

describe("UTC date-times", () => {
  it("formats the iCalendar form Stalwart writes as is", () => {
    expect(formatICalUtc(Date.UTC(2026, 9, 27, 10, 5, 9))).toBe(
      "20261027T100509Z"
    );
  });

  it("parses both forms and dates", () => {
    const ms = Date.UTC(2026, 9, 27, 10);
    expect(parseICalUtc("20261027T100000Z")).toBe(ms);
    expect(parseICalUtc("2026-10-27T10:00:00Z")).toBe(ms);
    expect(parseICalUtc("20261027")).toBe(Date.UTC(2026, 9, 27));
    expect(parseICalUtc("2026-13-01T00:00:00Z")).toBeNull();
    expect(parseICalUtc(42)).toBeNull();
  });

  it("reads the completion date", () => {
    expect(completedAt(stored)).toBe(Date.UTC(2026, 9, 26, 9));
    expect(completedAt({})).toBeNull();
  });

  it("reads utcStart without Stalwart's sentinel for undated tasks", () => {
    expect(taskUtc("32548-12-04T15:30:07Z")).toBeNull();
    expect(taskUtc("2026-11-05T00:00:00Z")).toBe(Date.UTC(2026, 10, 5));
    expect(taskUtc(null)).toBeNull();
    expect(taskUtc("soon")).toBeNull();
  });
});

describe(parentUids, () => {
  it("lists only parent relations", () => {
    expect(
      parentUids({
        relatedTo: {
          a: { relation: { parent: true } },
          b: { relation: { child: true } },
          c: {},
        },
      })
    ).toStrictEqual(["a"]);
    expect(parentUids({})).toStrictEqual([]);
  });
});
