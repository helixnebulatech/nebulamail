// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { chunk, memberIdsToFetch, newestIds } from "./thread-ids";

describe(chunk, () => {
  it("splits into pieces of the server's maxObjectsInGet", () => {
    expect(chunk([1, 2, 3, 4, 5], 2)).toStrictEqual([[1, 2], [3, 4], [5]]);
    expect(chunk([], 500)).toStrictEqual([]);
  });

  it("never loops on a size below 1", () => {
    expect(chunk([1, 2], 0)).toStrictEqual([[1], [2]]);
  });
});

describe(newestIds, () => {
  it("keeps a short thread as it is", () => {
    expect(newestIds(["a", "b"], 5)).toStrictEqual(["a", "b"]);
  });

  it("keeps the newest of a long thread (ids come oldest first)", () => {
    expect(newestIds(["a", "b", "c", "d"], 2)).toStrictEqual(["c", "d"]);
  });

  it("always keeps the opened email, in thread order", () => {
    expect(newestIds(["a", "b", "c", "d"], 2, "a")).toStrictEqual(["a", "d"]);
    expect(newestIds(["a", "b", "c", "d"], 2, "d")).toStrictEqual(["c", "d"]);
    expect(newestIds(["a", "b", "c"], 2, "zz")).toStrictEqual(["b", "c"]);
  });
});

describe(memberIdsToFetch, () => {
  it("asks for nothing when every thread is a single email", () => {
    expect(memberIdsToFetch([["a"], ["b"]], new Map(), 100)).toStrictEqual([]);
  });

  it("skips emails already fetched as rows", () => {
    const known = new Map([["c", true]]);

    expect(
      memberIdsToFetch([["a", "b", "c"], ["d"]], known, 100)
    ).toStrictEqual(["a", "b"]);
  });

  it("shares the budget so one huge thread can't take it all", () => {
    const huge = Array.from({ length: 100 }, (_, index) => `h${index}`);
    const wanted = memberIdsToFetch([huge, ["x", "y"]], new Map(), 10);

    expect(wanted).toContain("x");
    expect(wanted).toContain("y");
    expect(wanted).toContain("h99");
    expect(wanted).not.toContain("h0");
    expect(wanted.length).toBeLessThanOrEqual(10);
  });
});
