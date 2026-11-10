// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { AccountId, CalendarId } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import type { CalendarSummary } from "../calendar/model";
import {
  addableCalendars,
  bridgeHint,
  defaultListKey,
  taskListsOf,
  withSavedList,
} from "./lists";

const calendar = (
  id: string,
  extra: Partial<CalendarSummary> = {}
): CalendarSummary => ({
  accountId: AccountId.parse("c"),
  color: "#2353C9",
  id: CalendarId.parse(id),
  isDefault: false,
  key: `c/${id}`,
  mayDelete: true,
  mayWrite: true,
  name: id,
  owner: null,
  sortOrder: 0,
  visible: true,
  ...extra,
});

const calendars = [
  calendar("default", { isDefault: true }),
  calendar("groceries"),
  calendar("empty-list"),
  calendar("shared", {
    accountId: AccountId.parse("b"),
    key: "b/shared",
    mayWrite: false,
    owner: "Bob",
  }),
];

describe(taskListsOf, () => {
  it("lists calendars with tasks and the ones saved as lists", () => {
    const lists = taskListsOf({
      calendars,
      open: new Map([["c/groceries", 3]]),
      saved: ["c/empty-list"],
      withTasks: new Set(["c/groceries", "b/shared"]),
    });
    expect(lists.map((list) => [list.key, list.open])).toStrictEqual([
      ["c/groceries", 3],
      ["c/empty-list", 0],
      ["b/shared", 0],
    ]);
  });

  it("leaves out the ones the user removed, with tasks or not", () => {
    const lists = taskListsOf({
      calendars,
      hint: () => true,
      open: new Map(),
      removed: ["c/groceries", "c/empty-list"],
      saved: ["c/empty-list"],
      withTasks: new Set(["c/groceries"]),
    });
    expect(lists.map((list) => list.key)).toStrictEqual([
      "c/default",
      "b/shared",
    ]);
  });

  it("follows what the server says about a calendar", () => {
    const hints = new Map([
      ["c/default", true],
      ["c/groceries", false],
    ]);
    const lists = taskListsOf({
      calendars,
      hint: (item) => hints.get(item.key) ?? null,
      open: new Map(),
      saved: ["c/groceries"],
      withTasks: new Set(["c/groceries"]),
    });
    expect(lists.map((list) => list.key)).toStrictEqual(["c/default"]);
  });
});

describe(defaultListKey, () => {
  const lists = taskListsOf({
    calendars,
    open: new Map(),
    saved: [],
    withTasks: new Set(["c/groceries", "c/empty-list", "b/shared"]),
  });

  it("prefers the first saved list the user can write to", () => {
    expect(defaultListKey(lists, ["b/shared", "c/empty-list"])).toBe(
      "c/empty-list"
    );
    expect(defaultListKey(lists, [])).toBe("c/groceries");
    expect(defaultListKey([], [])).toBeNull();
  });
});

describe(withSavedList, () => {
  it("adds first, without repeats, and removes", () => {
    expect(withSavedList(["a", "b"], "b", true)).toStrictEqual(["b", "a"]);
    expect(withSavedList(["a", "b"], "a", false)).toStrictEqual(["b"]);
  });
});

describe(bridgeHint, () => {
  it("reads the bridge's components of each calendar", () => {
    expect(bridgeHint(calendar("t", { components: ["VTODO"] }))).toBeTruthy();
    expect(
      bridgeHint(calendar("both", { components: ["VEVENT", "VTODO"] }))
    ).toBeTruthy();
    expect(bridgeHint(calendar("e", { components: ["VEVENT"] }))).toBeFalsy();
    expect(bridgeHint(calendar("none", { components: null }))).toBeNull();
    expect(bridgeHint(calendar("unsaid"))).toBeNull();
  });

  it("lists task calendars without tasks and leaves event-only ones out", () => {
    const lists = taskListsOf({
      calendars: [
        calendar("tasks", { components: ["VTODO"] }),
        calendar("events", { components: ["VEVENT"] }),
      ],
      hint: bridgeHint,
      open: new Map(),
      saved: ["c/events"],
      withTasks: new Set(["c/events"]),
    });
    expect(lists.map((list) => list.id)).toStrictEqual(["tasks"]);
  });
});

describe(addableCalendars, () => {
  it("offers the calendars that aren't lists yet, own first", () => {
    const keys = addableCalendars(
      [calendars[3], ...calendars.slice(0, 3)].flatMap((item) =>
        item === undefined ? [] : [item]
      ),
      [{ key: "c/groceries" }]
    ).map((item) => item.key);
    expect(keys).toStrictEqual(["c/default", "c/empty-list", "b/shared"]);
  });

  it("leaves out the calendars the server says can't hold tasks", () => {
    const keys = addableCalendars(
      [
        calendar("events", { components: ["VEVENT"] }),
        calendar("both", { components: ["VEVENT", "VTODO"] }),
        calendar("stalwart"),
      ],
      []
    ).map((item) => item.key);
    expect(keys).toStrictEqual(["c/both", "c/stalwart"]);
  });
});
