// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { JmapError } from "@nebulamail/jmap-client";
import { describe, expect, it } from "vitest";

import { listStatusOf } from "./list-status";
import type { ListStatusInput } from "./list-status";
import { searchQuery } from "./queries";

const methodError = (type: string): JmapError =>
  new JmapError({
    arguments: {},
    callId: "c0",
    kind: "method",
    method: "Email/query",
    type,
  });

const unsupported = methodError("unsupportedFilter");

const base: ListStatusInput = {
  error: null,
  mailboxesFailed: false,
  mailboxesLoaded: true,
  query: "hello",
  queryStatus: "error",
  view: { kind: "starred" },
};

describe(listStatusOf, () => {
  it("is «unsupported» for a search the server refused", () => {
    expect(listStatusOf({ ...base, error: unsupported })).toBe("unsupported");
  });

  it("stays an error for other errors and for folder lists", () => {
    expect([
      listStatusOf({ ...base, error: methodError("serverFail") }),
      listStatusOf({ ...base, error: new JmapError({ kind: "network" }) }),
      listStatusOf({ ...base, error: unsupported, query: "" }),
    ]).toStrictEqual(["error", "error", "error"]);
  });

  it("follows the query otherwise", () => {
    expect([
      listStatusOf({ ...base, queryStatus: "pending" }),
      listStatusOf({ ...base, queryStatus: "success" }),
      listStatusOf({ ...base, query: "", queryStatus: "success" }),
      listStatusOf({ ...base, query: "", queryStatus: "pending", view: null }),
      listStatusOf({
        ...base,
        mailboxesLoaded: false,
        query: "",
        queryStatus: "pending",
        view: null,
      }),
    ]).toStrictEqual(["pending", "success", "success", "error", "pending"]);
  });

  it("is an error when the folders failed and the folder list waits for them", () => {
    // 2026-11-08: «Loading messages…» forever after the first request failed.
    const failed = {
      ...base,
      mailboxesFailed: true,
      mailboxesLoaded: false,
      query: "",
      view: null,
    };
    expect([
      listStatusOf({ ...failed, queryStatus: "pending" }),
      // Folders from before (a later refetch failed): the list still works.
      listStatusOf({
        ...failed,
        queryStatus: "success",
        view: { kind: "starred" },
      }),
      // A search doesn't need the folders.
      listStatusOf({ ...failed, query: "hello", queryStatus: "pending" }),
    ]).toStrictEqual(["error", "success", "pending"]);
  });

  it("is «unsupported» when a later page is refused (rows already loaded)", () => {
    // react-query keeps the pages and turns the status to "error".
    expect(
      listStatusOf({ ...base, error: unsupported, queryStatus: "error" })
    ).toBe("unsupported");
  });
});

describe("search retries", () => {
  const { retry } = searchQuery("hello");

  it("doesn't retry a search the server refused", () => {
    expect(retry).toBeTypeOf("function");
    if (typeof retry !== "function") {
      return;
    }

    expect([
      retry(0, unsupported),
      retry(0, new JmapError({ kind: "network" })),
      retry(1, new JmapError({ kind: "network" })),
    ]).toStrictEqual([false, true, false]);
  });
});
