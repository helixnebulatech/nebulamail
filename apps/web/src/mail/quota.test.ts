// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { pickQuota, quotaLevel } from "./quota";

const MB = 1024 * 1024;

const octets = (used: number, hardLimit: number) => ({
  hardLimit,
  resourceType: "octets",
  scope: "account",
  softLimit: null,
  types: ["Email", "ContactCard"],
  used,
  warnLimit: null,
});

describe(quotaLevel, () => {
  it("warns at 80 % and is full at 95 % without server limits", () => {
    expect(quotaLevel(octets(79 * MB, 100 * MB))).toBe("ok");
    expect(quotaLevel(octets(80 * MB, 100 * MB))).toBe("warn");
    expect(quotaLevel(octets(95 * MB, 100 * MB))).toBe("full");
  });

  it("uses the server's warn and soft limits when given", () => {
    const quota = { ...octets(50, 100), softLimit: 60, warnLimit: 40 };
    expect(quotaLevel(quota)).toBe("warn");
    expect(quotaLevel({ ...quota, used: 60 })).toBe("full");
    expect(quotaLevel({ ...quota, used: 39 })).toBe("ok");
  });
});

describe(pickQuota, () => {
  it("shows the storage quota closest to full", () => {
    expect(
      pickQuota([
        octets(5 * MB, 100 * MB),
        { ...octets(10, 100), resourceType: "count" },
        { ...octets(900 * MB, 1000 * MB), scope: "domain" },
      ])
    ).toStrictEqual({ level: "warn", limit: 1000 * MB, used: 900 * MB });
  });

  // RFC 9425 §4.1: `types` says what a quota counts.
  it("ignores storage quotas that don't count emails", () => {
    expect(
      pickQuota([
        octets(5 * MB, 100 * MB),
        { ...octets(99 * MB, 100 * MB), types: ["CalendarEvent"] },
        { ...octets(99 * MB, 100 * MB), types: [] },
      ])
    ).toStrictEqual({ level: "ok", limit: 100 * MB, used: 5 * MB });
  });

  it("is null without a storage limit", () => {
    expect(pickQuota([])).toBeNull();
    expect(pickQuota([{ ...octets(1, 0) }])).toBeNull();
    expect(pickQuota([{ ...octets(1, 10), resourceType: "count" }])).toBeNull();
  });
});
