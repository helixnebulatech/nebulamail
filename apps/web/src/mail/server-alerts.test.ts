// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";

import { alertsToShow, closeServerAlert } from "./server-alerts";

const alert = (id: string) => ({
  id,
  receivedAt: "2026-11-06T10:00:00Z",
  text: `Alert ${id}`,
});

describe(alertsToShow, () => {
  beforeEach(() => {
    globalThis.localStorage.clear();
  });

  it("shows each alert once, and never again once closed", () => {
    const all = [alert("a"), alert("b")];
    expect(alertsToShow(all, new Set()).map((item) => item.id)).toStrictEqual([
      "a",
      "b",
    ]);
    expect(alertsToShow(all, new Set(["a"]))).toStrictEqual([alert("b")]);
    closeServerAlert("b");
    closeServerAlert("b");
    expect(alertsToShow(all, new Set())).toStrictEqual([alert("a")]);
  });

  it("survives damaged storage", () => {
    globalThis.localStorage.setItem("wm-closed-server-alerts", "{nope");
    expect(alertsToShow([alert("a")], new Set())).toHaveLength(1);
    globalThis.localStorage.setItem("wm-closed-server-alerts", '[1, "a"]');
    expect(alertsToShow([alert("a")], new Set())).toHaveLength(0);
  });
});
