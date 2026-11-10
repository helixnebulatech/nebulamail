// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { parseDateTime } from "@internationalized/date";
import { AccountId, CalendarId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import {
  calendarOfForm,
  changedForm,
  defaultAlertsMap,
  newAlertId,
  newCalendarSettings,
  newEventStart,
  prefillAlerts,
  readDefaultAlerts,
  settingsOf,
  settingsPatch,
} from "./defaults";
import { newForm } from "./editing";
import type { CalendarSummary } from "./model";

const display = (offset: string, extra: object = {}) => ({
  "@type": "Alert",
  action: "display",
  trigger: { "@type": "OffsetTrigger", offset, relativeTo: "start" },
  ...extra,
});

const EMAIL = {
  "@type": "Alert",
  action: "email",
  trigger: { "@type": "OffsetTrigger", offset: "-PT1H", relativeTo: "start" },
};

const calendar = (
  id: string,
  extra: Partial<CalendarSummary> = {}
): CalendarSummary => ({
  accountId: AccountId.parse("c"),
  color: "#123456",
  id: CalendarId.parse(id),
  isDefault: id === "b",
  key: `c/${id}`,
  mayDelete: true,
  mayWrite: true,
  name: id,
  owner: null,
  sortOrder: 0,
  visible: true,
  ...extra,
});

describe(readDefaultAlerts, () => {
  it("turns start-relative display alerts into minutes", () => {
    expect(
      readDefaultAlerts({
        a: display("-PT15M"),
        b: display("-P1DT9H"),
        c: display("PT0S"),
        // Stalwart fills in what's missing: no type, action or relativeTo.
        d: { trigger: { offset: "-PT1H" } },
        e: display("-PT15M"),
      })
    ).toStrictEqual({ minutes: [15, 1980, 0, 60], others: {} });
  });

  it("keeps aside what the app can't show", () => {
    const end = display("-PT5M");
    end.trigger.relativeTo = "end";
    const absolute = {
      "@type": "Alert",
      trigger: { "@type": "AbsoluteTrigger", when: "2026-11-01T10:00:00Z" },
    };
    const read = readDefaultAlerts({
      a: display("-PT10M"),
      absolute,
      end,
      m: EMAIL,
    });
    expect(read?.minutes).toStrictEqual([10]);
    expect(Object.keys(read?.others ?? {})).toStrictEqual([
      "absolute",
      "end",
      "m",
    ]);
  });

  it("skips junk and says null when there is no map", () => {
    expect(readDefaultAlerts({ x: 1, y: { trigger: "no" } })).toStrictEqual({
      minutes: [],
      others: {},
    });
    expect(readDefaultAlerts(null)).toBeNull();
    expect(readDefaultAlerts(undefined)).toBeNull();
    expect(readDefaultAlerts([display("-PT1M")])).toBeNull();
  });
});

/** Ids k1, k2… in order, for exact expectations. */
const counter = () => {
  let next = 0;
  return () => {
    next += 1;
    return `k${String(next)}`;
  };
};

describe(newAlertId, () => {
  it("is a JMAP id and new each time", () => {
    const ids = new Set(Array.from({ length: 50 }, () => newAlertId()));
    expect(ids.size).toBe(50);
    for (const id of ids) {
      expect(id).toMatch(/^a[0-9a-f]{16}$/u);
    }
  });
});

describe(defaultAlertsMap, () => {
  it("writes the kept ones, then one display alert per value, all with new ids", () => {
    expect(
      defaultAlertsMap([10, 1440, 10], { a1: EMAIL }, counter())
    ).toStrictEqual({
      k1: EMAIL,
      k2: display("-PT10M"),
      k3: display("-P1D"),
    });
    expect(defaultAlertsMap([])).toStrictEqual({});
    expect(defaultAlertsMap([-5], {}, counter())).toStrictEqual({
      k1: display("PT5M"),
    });
  });

  it("never reuses an id (Stalwart 1.0: unique in the account)", () => {
    const first = Object.keys(defaultAlertsMap([10, 30]));
    const second = Object.keys(defaultAlertsMap([10, 30]));
    expect(new Set([...first, ...second]).size).toBe(4);
  });

  it("reads back what it writes", () => {
    expect(
      readDefaultAlerts(defaultAlertsMap([0, 15, 900]))?.minutes
    ).toStrictEqual([0, 15, 900]);
  });
});

describe(settingsPatch, () => {
  const stored = calendar("work", {
    defaultAlertsWithTime: { minutes: [15], others: { m: EMAIL } },
    defaultAlertsWithoutTime: { minutes: [], others: {} },
    timeZone: "Europe/Madrid",
  });

  it("sends only what changed", () => {
    const before = settingsOf(stored);
    expect(before).toStrictEqual({
      timeZone: "Europe/Madrid",
      withTime: [15],
      withoutTime: [],
    });
    expect(settingsPatch(stored, before, before)).toStrictEqual({});
    expect(
      settingsPatch(stored, before, { ...before, timeZone: null })
    ).toStrictEqual({ timeZone: null });
    expect(
      settingsPatch(stored, before, { ...before, timeZone: "America/New_York" })
    ).toStrictEqual({ timeZone: "America/New_York" });
  });

  it("writes both maps whole with new ids, {} to clear, keeping alerts it can't show", () => {
    const before = settingsOf(stored);
    expect(
      settingsPatch(
        stored,
        before,
        { ...before, withTime: [], withoutTime: [900] },
        counter()
      )
    ).toStrictEqual({
      defaultAlertsWithTime: { k1: EMAIL },
      defaultAlertsWithoutTime: { k2: display("-PT15H") },
    });
    // One map changed: the other goes too (its old ids could clash).
    expect(
      settingsPatch(stored, before, { ...before, withoutTime: [0] }, counter())
    ).toStrictEqual({
      defaultAlertsWithTime: { k1: EMAIL, k2: display("-PT15M") },
      defaultAlertsWithoutTime: { k3: display("PT0S") },
    });
  });

  it("never sends a zone the browser doesn't know", () => {
    const before = settingsOf(stored);
    expect(
      settingsPatch(stored, before, { ...before, timeZone: "Not/AZone" })
    ).toStrictEqual({});
  });

  it("a new calendar: no zone, the usual 10 minutes, nothing empty sent", () => {
    const before = settingsOf(null);
    expect(before).toStrictEqual({
      timeZone: null,
      withTime: [10],
      withoutTime: [],
    });
    expect(settingsPatch(null, before, before, counter())).toStrictEqual({
      defaultAlertsWithTime: { k1: display("-PT10M") },
    });
    expect(
      settingsPatch(null, before, {
        timeZone: "Asia/Tokyo",
        withTime: [],
        withoutTime: [],
      })
    ).toStrictEqual({ timeZone: "Asia/Tokyo" });
  });
});

describe(prefillAlerts, () => {
  it("uses the calendar's defaults, with or without time", () => {
    const work = calendar("work", {
      defaultAlertsWithTime: { minutes: [30, 5], others: {} },
      defaultAlertsWithoutTime: { minutes: [900], others: {} },
    });
    expect(prefillAlerts(work, false)).toStrictEqual([30, 5]);
    expect(prefillAlerts(work, true)).toStrictEqual([900]);
  });

  it("empty defaults mean no reminders; unknown ones the fallback", () => {
    const none = calendar("b", {
      defaultAlertsWithTime: { minutes: [], others: { m: EMAIL } },
      defaultAlertsWithoutTime: { minutes: [], others: {} },
    });
    expect(prefillAlerts(none, false)).toStrictEqual([]);
    expect(prefillAlerts(calendar("b"), false)).toStrictEqual([10]);
    expect(prefillAlerts(calendar("b"), true)).toStrictEqual([]);
    expect(prefillAlerts(undefined, false)).toStrictEqual([10]);
  });
});

describe(changedForm, () => {
  const home = calendar("b", {
    defaultAlertsWithTime: { minutes: [10], others: {} },
    defaultAlertsWithoutTime: { minutes: [], others: {} },
  });
  const work = calendar("work", {
    defaultAlertsWithTime: { minutes: [30, 5], others: {} },
    defaultAlertsWithoutTime: { minutes: [900], others: {} },
  });
  const shared = calendar("work", {
    accountId: AccountId.parse("bob"),
    defaultAlertsWithTime: { minutes: [60], others: {} },
    key: "bob/work",
    owner: "Bob",
  });
  const calendars = [home, work, shared];
  const form = {
    ...newForm(home.id, parseDateTime("2026-10-26T10:00"), 60, "Europe/Madrid"),
    alerts: [10],
  };

  it("follows the calendar and all day while untouched", () => {
    const moved = changedForm(form, { calendarId: work.id }, calendars, true);
    expect(moved.alerts).toStrictEqual([30, 5]);
    const allDay = changedForm(moved, { allDay: true }, calendars, true);
    expect(allDay.alerts).toStrictEqual([900]);
    const theirs = changedForm(
      form,
      { accountId: shared.accountId, calendarId: shared.id },
      calendars,
      true
    );
    expect(theirs.alerts).toStrictEqual([60]);
    expect(calendarOfForm(calendars, theirs)).toBe(shared);
  });

  it("leaves the reminders alone once edited, or for other changes", () => {
    expect(
      changedForm(form, { calendarId: work.id }, calendars, false).alerts
    ).toStrictEqual([10]);
    expect(
      changedForm({ ...form, alerts: [1] }, { title: "x" }, calendars, true)
        .alerts
    ).toStrictEqual([1]);
  });
});

describe(newEventStart, () => {
  const at = parseDateTime("2026-10-26T10:00");

  it("moves the picked time into the calendar's zone (same instant)", () => {
    const result = newEventStart(
      at,
      "Europe/Madrid",
      "America/New_York",
      false
    );
    expect(result.zone).toBe("America/New_York");
    expect(result.at.toString()).toBe("2026-10-26T05:00:00");
  });

  it("keeps the user's zone otherwise", () => {
    for (const zone of [null, undefined, "Europe/Madrid", "Not/AZone"]) {
      expect(newEventStart(at, "Europe/Madrid", zone, false)).toStrictEqual({
        at,
        zone: "Europe/Madrid",
      });
    }
    expect(
      newEventStart(at, "Europe/Madrid", "Asia/Tokyo", true)
    ).toStrictEqual({
      at,
      zone: "Europe/Madrid",
    });
  });
});

describe(newCalendarSettings, () => {
  it("gives a new calendar the zone unless another was chosen", () => {
    expect(newCalendarSettings(undefined, "Europe/Madrid")).toStrictEqual({
      timeZone: "Europe/Madrid",
    });
    expect(
      newCalendarSettings({ timeZone: "Asia/Tokyo" }, "Europe/Madrid")
    ).toStrictEqual({ timeZone: "Asia/Tokyo" });
  });

  it("sends no zone if the user chose none", () => {
    expect(
      newCalendarSettings(
        { defaultAlertsWithTime: {}, timeZone: null },
        "Europe/Madrid"
      )
    ).toStrictEqual({ defaultAlertsWithTime: {} });
  });
});
