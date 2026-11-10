// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { TaskSnapshot } from "./adapter";
import { PUSH_COALESCE_MS, pushRefresher, taskKeys } from "./hooks";

const KEY = taskKeys.snapshot("UTC", "2026-11-08");

const snapshot = (state: string): TaskSnapshot => ({
  lists: [],
  saved: [],
  states: new Map([["a", state]]),
  stored: new Map(),
  tasks: [],
  truncated: false,
});

let client = new QueryClient();
let stop = (): void => undefined;

/** A shown snapshot (an observer keeps it active) answering `next`. */
const show = (next: () => Promise<TaskSnapshot>) => {
  const observer = new QueryObserver(client, {
    queryFn: next,
    queryKey: KEY,
    staleTime: Infinity,
  });
  stop = observer.subscribe(() => {});
};

describe(pushRefresher, () => {
  beforeEach(() => {
    vi.useFakeTimers();
    client = new QueryClient();
  });

  afterEach(() => {
    stop();
    client.clear();
    vi.useRealTimers();
  });

  it("reads nothing for a push the snapshot already holds", async () => {
    const load = vi.fn<() => Promise<TaskSnapshot>>(
      async () => await Promise.resolve(snapshot("s1"))
    );
    show(load);
    await vi.waitFor(() => {
      expect(client.getQueryData(KEY)).toBeDefined();
    });
    const refresher = pushRefresher(client);
    refresher.onChange({ calendars: false, events: { a: "s1" } });
    await vi.advanceTimersByTimeAsync(PUSH_COALESCE_MS * 2);
    expect(load).toHaveBeenCalledOnce();
  });

  it("gathers pushes into one read of a newer state", async () => {
    const load = vi.fn<() => Promise<TaskSnapshot>>(
      async () => await Promise.resolve(snapshot("s1"))
    );
    show(load);
    await vi.waitFor(() => {
      expect(client.getQueryData(KEY)).toBeDefined();
    });
    const refresher = pushRefresher(client);
    refresher.onChange({ calendars: false, events: { a: "s2" } });
    refresher.onChange({ calendars: false, events: { a: "s3" } });
    await vi.advanceTimersByTimeAsync(PUSH_COALESCE_MS - 1);
    refresher.onChange({ calendars: false, events: { a: "s4" } });
    await vi.advanceTimersByTimeAsync(PUSH_COALESCE_MS * 2);
    expect(load).toHaveBeenCalledTimes(2);
  });

  it("waits for a load in progress instead of cancelling it", async () => {
    const pending = Promise.withResolvers<TaskSnapshot>();
    const load = vi.fn<() => Promise<TaskSnapshot>>(
      async () => await pending.promise
    );
    show(load);
    const refresher = pushRefresher(client);
    refresher.onChange({ calendars: false, events: { a: "s2" } });
    await vi.advanceTimersByTimeAsync(PUSH_COALESCE_MS * 3);
    expect(load).toHaveBeenCalledOnce();
    // The load read the pushed state: nothing more to read.
    pending.resolve(snapshot("s2"));
    await vi.advanceTimersByTimeAsync(PUSH_COALESCE_MS * 2);
    expect(load).toHaveBeenCalledOnce();
    expect(client.getQueryData<TaskSnapshot>(KEY)?.states.get("a")).toBe("s2");
  });

  it("reads again when a list changed, and the calendars too", async () => {
    const load = vi.fn<() => Promise<TaskSnapshot>>(
      async () => await Promise.resolve(snapshot("s1"))
    );
    show(load);
    await vi.waitFor(() => {
      expect(client.getQueryData(KEY)).toBeDefined();
    });
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const refresher = pushRefresher(client);
    refresher.onChange({ calendars: true, events: {} });
    await vi.advanceTimersByTimeAsync(PUSH_COALESCE_MS * 2);
    expect(load).toHaveBeenCalledTimes(2);
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["calendar", "calendars"],
    });
  });

  it("stops when cancelled", async () => {
    const load = vi.fn<() => Promise<TaskSnapshot>>(
      async () => await Promise.resolve(snapshot("s1"))
    );
    show(load);
    await vi.waitFor(() => {
      expect(client.getQueryData(KEY)).toBeDefined();
    });
    const refresher = pushRefresher(client);
    refresher.onChange({ calendars: true, events: { a: "s9" } });
    refresher.cancel();
    await vi.advanceTimersByTimeAsync(PUSH_COALESCE_MS * 2);
    expect(load).toHaveBeenCalledOnce();
  });
});
