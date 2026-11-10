// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { describe, expect, it } from "vitest";

import { progressStep } from "./use-uploads";

describe(progressStep, () => {
  it("draws whole percents only, so most progress events change nothing", () => {
    expect(progressStep(0)).toBe(0);
    expect(progressStep(0.004)).toBe(0);
    expect(progressStep(0.01)).toBe(1);
    expect(progressStep(0.425)).toBe(42);
    expect(progressStep(1)).toBe(100);
  });

  it("keeps the unknown state and clamps odd values", () => {
    expect(progressStep(null)).toBeNull();
    expect(progressStep(-0.5)).toBe(0);
    expect(progressStep(1.5)).toBe(100);
  });
});
