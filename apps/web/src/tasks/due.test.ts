// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { dueForChoice, dueLabel } from "./due";
import { makeTask as task } from "./test-support";

const zone = "Europe/Madrid";
// Thursday 29 October 2026, 12:00 in Madrid.
const now = Date.UTC(2026, 9, 29, 11);
const midnight = (day: string) => Date.parse(`${day}T00:00:00+01:00`);

const onDay = (day: string, completed = false) =>
  task("t", {
    completed,
    due: { date: day, kind: "date" },
    dueAt: midnight(day),
  });

describe(dueLabel, () => {
  it("names the days around today", () => {
    expect(dueLabel(onDay("2026-10-28"), now, zone)).toMatchObject({
      overdue: true,
      when: "yesterday",
    });
    expect(dueLabel(onDay("2026-10-29"), now, zone)).toMatchObject({
      overdue: false,
      time: null,
      when: "today",
    });
  });

  it("names the days of the coming week and later ones", () => {
    expect(dueLabel(onDay("2026-10-30"), now, zone)?.when).toBe("tomorrow");
    expect(dueLabel(onDay("2026-11-04"), now, zone)?.when).toBe("this-week");
    expect(dueLabel(onDay("2026-11-05"), now, zone)?.when).toBe("date");
    expect(dueLabel(onDay("2026-10-20"), now, zone)).toMatchObject({
      overdue: true,
      when: "date",
    });
  });

  it("says when the year isn't this one", () => {
    expect(dueLabel(onDay("2027-01-05"), now, zone)?.otherYear).toBeTruthy();
  });

  it("is overdue at a past time today, not when done", () => {
    const at = Date.parse("2026-10-29T09:00:00+01:00");
    const timed = task("t", {
      due: { kind: "datetime", local: "2026-10-29T09:00:00", timeZone: zone },
      dueAt: at,
    });
    expect(dueLabel(timed, now, zone)).toMatchObject({
      overdue: true,
      time: at,
      when: "today",
    });
    expect(
      dueLabel({ ...timed, completed: true }, now, zone)?.overdue
    ).toBeFalsy();
  });

  it("is not overdue once done", () => {
    expect(dueLabel(onDay("2026-10-20", true), now, zone)?.overdue).toBeFalsy();
  });

  it("is nothing without a due", () => {
    expect(dueLabel(task("t"), now, zone)).toBeNull();
  });
});

describe(dueForChoice, () => {
  it("picks today, tomorrow and next Monday", () => {
    expect(dueForChoice("today", null, now, zone)).toStrictEqual({
      date: "2026-10-29",
      kind: "date",
    });
    expect(dueForChoice("tomorrow", null, now, zone)).toStrictEqual({
      date: "2026-10-30",
      kind: "date",
    });
    expect(dueForChoice("next-week", null, now, zone)).toStrictEqual({
      date: "2026-11-02",
      kind: "date",
    });
    expect(dueForChoice("none", null, now, zone)).toBeNull();
  });

  it("keeps the time of a due with one", () => {
    expect(
      dueForChoice(
        "tomorrow",
        { kind: "datetime", local: "2026-10-01T17:30:00", timeZone: zone },
        now,
        zone
      )
    ).toStrictEqual({
      kind: "datetime",
      local: "2026-10-30T17:30:00",
      timeZone: zone,
    });
  });

  it("goes to the following Monday from a Monday", () => {
    const monday = Date.UTC(2026, 10, 2, 11);
    expect(dueForChoice("next-week", null, monday, zone)).toStrictEqual({
      date: "2026-11-09",
      kind: "date",
    });
  });
});
