// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Drawing a range to create an event.

import { parseDate, parseDateTime } from "@internationalized/date";
import {
  AccountId,
  CalendarEventId,
  CalendarId,
} from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import { DEFAULT_MINUTES, rangeMinutes } from "./editing";
import { drawnRange, localAt } from "./layout";
import type { CalendarSummary } from "./model";
import { accountRef, ownAccountOf, refOf } from "./targets";

describe(drawnRange, () => {
  it("snaps outwards to 15 minutes, either way up", () => {
    expect(drawnRange(16 * 60 + 5, 17 * 60 + 25)).toStrictEqual({
      from: 16 * 60,
      to: 17 * 60 + 30,
    });
    expect(drawnRange(17 * 60 + 25, 16 * 60 + 5)).toStrictEqual({
      from: 16 * 60,
      to: 17 * 60 + 30,
    });
  });

  it("is never empty and stays within the day", () => {
    expect(drawnRange(600, 600)).toStrictEqual({ from: 600, to: 615 });
    expect(drawnRange(1439, 1440)).toStrictEqual({ from: 1425, to: 1440 });
    expect(drawnRange(-30, 20)).toStrictEqual({ from: 0, to: 30 });
  });
});

describe(localAt, () => {
  it("writes wall times, midnight at the end as the next day", () => {
    const day = parseDate("2026-10-31");
    expect(localAt(day, 9 * 60 + 5)).toBe("2026-10-31T09:05");
    expect(localAt(day, 1440)).toBe("2026-11-01T00:00");
  });
});

describe(rangeMinutes, () => {
  const start = parseDateTime("2026-10-25T01:00");

  it("measures a dragged range in wall minutes", () => {
    // Even across Madrid's change of time that night: the grid is wall time.
    expect(rangeMinutes(start, "2026-10-25T02:30", false)).toBe(90);
    expect(rangeMinutes(start, "2026-10-25T03:00:00", false)).toBe(120);
  });

  it("falls back to the default length", () => {
    expect(rangeMinutes(start, null, false)).toBe(DEFAULT_MINUTES);
    expect(rangeMinutes(start, "2026-10-25T00:30", false)).toBe(
      DEFAULT_MINUTES
    );
    expect(rangeMinutes(start, "nonsense", false)).toBe(DEFAULT_MINUTES);
    expect(rangeMinutes(start, "2026-10-25T05:00", true)).toBe(DEFAULT_MINUTES);
  });
});

const summary = (accountId: string, owner: string | null): CalendarSummary => ({
  accountId: AccountId.parse(accountId),
  color: "#2353C9",
  id: CalendarId.parse("b"),
  isDefault: owner === null,
  key: `${accountId}/b`,
  mayDelete: false,
  mayWrite: true,
  name: "x",
  owner,
  sortOrder: 0,
  visible: true,
});

describe(refOf, () => {
  const calendars = [summary("c", null), summary("d", "bob@nebulamail.test")];

  it("writes the user's own account as null (URLs stay as they were)", () => {
    expect(ownAccountOf(calendars)).toBe("c");
    expect(accountRef(AccountId.parse("c"), calendars)).toBeNull();
    expect(
      refOf(
        {
          accountId: AccountId.parse("d"),
          baseId: CalendarEventId.parse("e"),
          id: CalendarEventId.parse("e"),
        },
        calendars
      )
    ).toStrictEqual({ accountId: "d", baseId: "e", id: "e" });
  });
});
