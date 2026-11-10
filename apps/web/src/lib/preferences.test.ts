// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import {
  DEFAULT_PREFERENCES,
  parsePreferences,
  serializePreferences,
} from "./preferences";

describe(parsePreferences, () => {
  it.each([
    [undefined, DEFAULT_PREFERENCES],
    ["", DEFAULT_PREFERENCES],
    [
      "dark.comfortable",
      {
        calendarTasks: "shown",
        density: "comfortable",
        pane: "side",
        sidebar: "open",
        theme: "dark",
      },
    ],
    [
      "light",
      {
        calendarTasks: "shown",
        density: "compact",
        pane: "side",
        sidebar: "open",
        theme: "light",
      },
    ],
    ["evil.<script>", DEFAULT_PREFERENCES],
    [
      "dark.x.y",
      {
        calendarTasks: "shown",
        density: "compact",
        pane: "side",
        sidebar: "open",
        theme: "dark",
      },
    ],
    [
      "dark.compact.side.collapsed",
      {
        calendarTasks: "shown",
        density: "compact",
        pane: "side",
        sidebar: "collapsed",
        theme: "dark",
      },
    ],
    [
      "light.compact.full.bogus",
      {
        calendarTasks: "shown",
        density: "compact",
        pane: "full",
        sidebar: "open",
        theme: "light",
      },
    ],
    [
      "dark.compact.side.collapsed.hidden",
      {
        calendarTasks: "hidden",
        density: "compact",
        pane: "side",
        sidebar: "collapsed",
        theme: "dark",
      },
    ],
    [
      "dark.compact.side.open.bogus",
      {
        calendarTasks: "shown",
        density: "compact",
        pane: "side",
        sidebar: "open",
        theme: "dark",
      },
    ],
    [
      "light.compact.full",
      {
        calendarTasks: "shown",
        density: "compact",
        pane: "full",
        sidebar: "open",
        theme: "light",
      },
    ],
  ])("%j", (value, expected) => {
    expect(parsePreferences(value)).toStrictEqual(expected);
  });

  it("round-trips", () => {
    const preferences = {
      calendarTasks: "hidden",
      density: "comfortable",
      pane: "full",
      sidebar: "collapsed",
      theme: "light",
    } as const;

    expect(parsePreferences(serializePreferences(preferences))).toStrictEqual(
      preferences
    );
  });
});
