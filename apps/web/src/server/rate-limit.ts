// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

/** Sliding-window limit per key, in memory (one process). */
export interface RateLimiter {
  /** True if allowed (and counted). */
  readonly take: (key: string, now: number) => boolean;
}

export const createRateLimiter = (
  max: number,
  windowMs: number
): RateLimiter => {
  const hits = new Map<string, number[]>();
  return {
    take: (key, now) => {
      const recent = (hits.get(key) ?? []).filter((at) => now - at < windowMs);
      if (recent.length >= max) {
        hits.set(key, recent);
        return false;
      }
      recent.push(now);
      hits.set(key, recent);
      // Keep the map small: forget keys with no recent hits.
      if (hits.size > 10_000) {
        for (const [other, times] of hits) {
          if (times.every((at) => now - at >= windowMs)) {
            hits.delete(other);
          }
        }
      }
      return true;
    },
  };
};
