// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the app makes of the BFF's preload answer (mail/preload-status.ts),
// including a full quota (accepted risk H15).

import { afterEach, describe, expect, it, vi } from "vitest";

import { postPreload } from "./preload-status";

const answer = (body: unknown, status = 200) => {
  vi.stubGlobal(
    "fetch",
    vi.fn<typeof fetch>().mockResolvedValue(Response.json(body, { status }))
  );
};

describe(postPreload, () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads a full quota and its date", async () => {
    answer({ available: true, full: { until: 1000 }, optOut: false });
    await expect(postPreload({})).resolves.toStrictEqual({
      full: { until: 1000 },
      kind: "ready",
      optOut: false,
    });
  });

  it("reads 'not full' from a BFF without the field", async () => {
    answer({ available: true, optOut: true });
    await expect(postPreload({})).resolves.toStrictEqual({
      full: null,
      kind: "ready",
      optOut: true,
    });
  });

  it("keeps the other states", async () => {
    answer({ available: false });
    await expect(postPreload({})).resolves.toStrictEqual({
      kind: "unavailable",
    });
    answer({ error: "too-many-addresses" }, 422);
    await expect(postPreload({})).resolves.toStrictEqual({ kind: "too-many" });
  });

  it("rejects a malformed date", async () => {
    answer({ available: true, full: { until: "soon" }, optOut: false });
    await expect(postPreload({})).rejects.toThrow(/until/u);
  });
});
