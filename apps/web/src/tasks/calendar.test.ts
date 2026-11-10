// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { tasksByDay, tasksOn } from "./calendar";
import { makeTask as task } from "./test-support";

const zone = "UTC";

const at = (time: string) => ({
  due: {
    kind: "datetime" as const,
    local: `2026-10-29T${time}:00`,
    timeZone: zone,
  },
  dueAt: Date.parse(`2026-10-29T${time}:00Z`),
});

const onDay = (date: string) => ({
  due: { date, kind: "date" as const },
  dueAt: Date.parse(`${date}T00:00:00Z`),
});

describe(tasksByDay, () => {
  it("puts dated tasks on their day, all-day apart from timed", () => {
    const byDay = tasksByDay(
      [
        task("late", at("17:00")),
        task("early", at("08:00")),
        task("done", { completed: true, ...onDay("2026-10-29") }),
        task("high", { priority: "high", ...onDay("2026-10-29") }),
        task("other-day", onDay("2026-11-20")),
        task("undated"),
      ],
      ["2026-10-29", "2026-10-30"],
      zone
    );
    const day = tasksOn(byDay, "2026-10-29");
    expect(day.allDay.map((item) => item.id)).toStrictEqual(["high", "done"]);
    expect(day.timed.map((item) => item.id)).toStrictEqual(["early", "late"]);
    expect(tasksOn(byDay, "2026-10-30")).toStrictEqual({
      allDay: [],
      timed: [],
    });
    expect(byDay.has("2026-11-20")).toBeFalsy();
  });
});
