// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { foldLongRuns } from "./thread-fold";

interface Item {
  readonly id: string;
}

const thread: readonly Item[] = ["a", "b", "c", "d", "e", "f"].map((id) => ({
  id,
}));

const ids = (entries: ReturnType<typeof foldLongRuns<Item>>): string[] =>
  entries.map((entry) =>
    entry.kind === "more" ? `+${entry.count}` : entry.email.id
  );

const allBut =
  (...open: string[]) =>
  (item: Item): boolean =>
    !open.includes(item.id);

describe("folding long runs of folded messages", () => {
  it("folds the middle of four or more folded emails", () => {
    expect(ids(foldLongRuns(thread, allBut("f"), false))).toStrictEqual([
      "a",
      "+3",
      "e",
      "f",
    ]);
  });

  it("leaves runs of three alone", () => {
    expect(
      ids(foldLongRuns(thread.slice(2), allBut("f"), false))
    ).toStrictEqual(["c", "d", "e", "f"]);
  });

  it("shows everything once unfolded", () => {
    expect(ids(foldLongRuns(thread, allBut("f"), true))).toStrictEqual([
      "a",
      "b",
      "c",
      "d",
      "e",
      "f",
    ]);
  });

  it("an open email splits the runs", () => {
    expect(ids(foldLongRuns(thread, allBut("c", "f"), false))).toStrictEqual([
      "a",
      "b",
      "c",
      "d",
      "e",
      "f",
    ]);
  });
});
