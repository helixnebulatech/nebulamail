// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { releasingWhenDone, StreamLimits } from "./streams";

describe(StreamLimits, () => {
  it("caps each session and the whole process", () => {
    const limits = new StreamLimits(2, 3);
    const a1 = limits.acquire("a");
    const a2 = limits.acquire("a");
    expect(a1).toBeDefined();
    expect(a2).toBeDefined();
    // The session is full; another one still gets a slot.
    expect(limits.acquire("a")).toBeUndefined();
    const b1 = limits.acquire("b");
    expect(b1).toBeDefined();
    // The process is full.
    expect(limits.acquire("c")).toBeUndefined();
  });

  it("frees a slot once, however often it is given back", () => {
    const limits = new StreamLimits(2, 3);
    const a1 = limits.acquire("a");
    limits.acquire("a");
    limits.acquire("b");
    a1?.();
    a1?.();
    expect(limits.openFor("a")).toBe(1);
    expect(limits.acquire("c")).toBeDefined();
    expect(limits.acquire("d")).toBeUndefined();
  });
});

describe(releasingWhenDone, () => {
  it("gives the slot back when the body ends", async () => {
    let released = 0;
    const response = releasingWhenDone(new Response("hello"), () => {
      released += 1;
    });
    expect(released).toBe(0);
    await expect(response.text()).resolves.toBe("hello");
    expect(released).toBe(1);
  });

  it("gives it back when the browser goes away, and cancels the source", async () => {
    let released = 0;
    let cancelled = false;
    const source = new ReadableStream<Uint8Array>({
      cancel: () => {
        cancelled = true;
      },
      pull: () => {
        // Never ends (a push stream).
      },
    });
    const response = releasingWhenDone(new Response(source), () => {
      released += 1;
    });
    await response.body?.cancel();
    expect(released).toBe(1);
    expect(cancelled).toBeTruthy();
  });

  it("gives it back when the source fails", async () => {
    let released = 0;
    const source = new ReadableStream<Uint8Array>({
      pull: (controller) => {
        controller.error(new Error("upstream broke"));
      },
    });
    const response = releasingWhenDone(new Response(source), () => {
      released += 1;
    });
    await expect(response.text()).rejects.toThrow("upstream broke");
    expect(released).toBe(1);
  });

  it("gives it back at once without a body, keeping status and headers", () => {
    let released = 0;
    const response = releasingWhenDone(
      new Response(null, { headers: { "retry-after": "5" }, status: 503 }),
      () => {
        released += 1;
      }
    );
    expect(released).toBe(1);
    expect(response.status).toBe(503);
    expect(response.headers.get("retry-after")).toBe("5");
  });
});
