// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { parseDate } from "@internationalized/date";
import { describe, expect, it } from "vitest";

import { formatWhen, rangeTitleShort, timingOf } from "./format";

const ZONE = "Europe/Madrid";

describe(formatWhen, () => {
  it("shows every day of an all-day event of several days", () => {
    const text = formatWhen(
      {
        allDay: true,
        end: Date.UTC(2026, 10, 7, 23),
        endDay: "2026-11-08",
        firstDay: "2026-11-03",
        start: Date.UTC(2026, 10, 2, 23),
      },
      ZONE,
      "es"
    );
    expect(text).toContain("3");
    expect(text).toContain("7");
  });
});

describe(timingOf, () => {
  it("finds the days of an all-day reminder in the zone", () => {
    expect(
      timingOf(
        {
          allDay: true,
          end: Date.UTC(2026, 10, 7, 23),
          start: Date.UTC(2026, 10, 2, 23),
        },
        ZONE
      )
    ).toMatchObject({ endDay: "2026-11-08", firstDay: "2026-11-03" });
  });

  it("counts a day with a clock change as one", () => {
    // 25 October 2026 has 25 hours in Madrid.
    expect(
      timingOf(
        {
          allDay: true,
          end: Date.UTC(2026, 9, 25, 23),
          start: Date.UTC(2026, 9, 24, 22),
        },
        ZONE
      )
    ).toMatchObject({ endDay: "2026-10-26", firstDay: "2026-10-25" });
  });

  it("leaves a timed one as it is", () => {
    expect(timingOf({ allDay: false, end: 2, start: 1 }, ZONE)).toStrictEqual({
      allDay: false,
      end: 2,
      start: 1,
    });
  });
});

describe(rangeTitleShort, () => {
  const anchor = parseDate("2026-11-08");
  const range = {
    end: parseDate("2026-11-15"),
    start: parseDate("2026-11-09"),
  };

  it("fits a phone's bar: no year on a week, a short day", () => {
    const week = rangeTitleShort("week", anchor, range, "en-US");
    expect(week).toMatch(/Nov 9\s*–\s*14/u);
    expect(week).not.toContain("2026");
    expect(rangeTitleShort("day", anchor, range, "en-US")).toBe("Sun, Nov 8");
    expect(rangeTitleShort("month", anchor, range, "en-US")).toBe("Nov 2026");
  });
});
