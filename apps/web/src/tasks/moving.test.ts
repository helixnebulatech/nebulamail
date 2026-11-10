// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import type { TaskDue } from "./model";
import { droppedDue, keyNudge, nudgedDue, shiftedSeriesDue } from "./moving";

const zone = "Europe/Madrid";
const day = (date: string): TaskDue => ({ date, kind: "date" });
const at = (local: string, timeZone: string | null = zone): TaskDue => ({
  kind: "datetime",
  local,
  timeZone,
});

describe(droppedDue, () => {
  it("moves a day or a time to another day of the month", () => {
    expect(
      droppedDue(
        day("2026-11-06"),
        { date: "2026-11-09", kind: "day" },
        zone,
        "2026-11-06"
      )
    ).toStrictEqual(day("2026-11-09"));
    expect(
      droppedDue(
        at("2026-11-06T17:30:00"),
        { date: "2026-11-04", kind: "day" },
        zone,
        "2026-11-06"
      )
    ).toStrictEqual(at("2026-11-04T17:30:00"));
  });

  it("makes a day of a time in the all-day row, and a time of a day", () => {
    expect(
      droppedDue(
        at("2026-11-06T17:30:00"),
        { date: "2026-11-07", kind: "all-day" },
        zone,
        "2026-11-06"
      )
    ).toStrictEqual(day("2026-11-07"));
    expect(
      droppedDue(
        day("2026-11-06"),
        { date: "2026-11-06", kind: "time", minutes: 615 },
        zone,
        "2026-11-06"
      )
    ).toStrictEqual(at("2026-11-06T10:15:00"));
  });

  it("keeps the task's own zone (or floating) for a time", () => {
    expect(
      droppedDue(
        at("2026-11-06T09:00:00", "America/New_York"),
        { date: "2026-11-06", kind: "time", minutes: 16 * 60 },
        zone,
        "2026-11-06"
      )
    ).toStrictEqual(at("2026-11-06T10:00:00", "America/New_York"));
    expect(
      droppedDue(
        at("2026-11-06T09:00:00", null),
        { date: "2026-11-08", kind: "time", minutes: 60 },
        zone,
        "2026-11-06"
      )
    ).toStrictEqual(at("2026-11-08T01:00:00", null));
  });
});

describe(shiftedSeriesDue, () => {
  it("shifts the series as much as the occurrence moved", () => {
    expect(
      shiftedSeriesDue(day("2026-11-02"), day("2026-11-09"), day("2026-11-11"))
    ).toStrictEqual(day("2026-11-04"));
    expect(
      shiftedSeriesDue(
        at("2026-11-02T09:00:00"),
        at("2026-11-09T09:00:00"),
        at("2026-11-10T11:30:00")
      )
    ).toStrictEqual(at("2026-11-03T11:30:00"));
    expect(
      shiftedSeriesDue(
        day("2026-11-02"),
        day("2026-11-09"),
        at("2026-11-09T08:00:00")
      )
    ).toStrictEqual(at("2026-11-02T08:00:00"));
  });
});

describe(keyNudge, () => {
  it("reads Alt + arrows", () => {
    expect(
      keyNudge({ altKey: true, key: "ArrowRight", shiftKey: false })
    ).toStrictEqual({ days: 1, minutes: 0 });
    expect(
      keyNudge({ altKey: true, key: "ArrowUp", shiftKey: false })
    ).toStrictEqual({ days: 0, minutes: -15 });
    expect(
      keyNudge({ altKey: false, key: "ArrowUp", shiftKey: false })
    ).toBeNull();
    expect(
      keyNudge({ altKey: true, key: "ArrowUp", shiftKey: true })
    ).toBeNull();
  });
});

describe(nudgedDue, () => {
  it("moves times by 15 minutes and days by a day or a week", () => {
    expect(
      nudgedDue(at("2026-11-06T23:50:00"), { days: 0, minutes: 15 })
    ).toStrictEqual(at("2026-11-07T00:05:00"));
    expect(nudgedDue(day("2026-11-06"), { days: 1, minutes: 0 })).toStrictEqual(
      day("2026-11-07")
    );
    expect(
      nudgedDue(day("2026-11-06"), { days: 0, minutes: -15 })
    ).toStrictEqual(day("2026-10-30"));
  });
});
