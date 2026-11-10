// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Per-recipient quota and opt-out for `preload`.
//
// Stalwart's MTA Hook sees envelope addresses, not JMAP accounts, so both
// are per recipient address (its keyed hash, `recipients.ts`):
//
// - `usage:<rcpt>`: `{ v, e: { <urlHash>: [bytes, expiresAt] } }`, the
//   preloaded images charged to that recipient. A recipient whose live
//   entries reach `maxBytes` or `maxImages` is "full": its next emails are
//   not preloaded and behave like `cache` (fetched on first open). The
//   check is made before downloading, so one email can go over by at most
//   its own budget (10 MB).
// - `refs:<urlHash>`: `{ v, r: { <rcpt>: expiresAt }, pinned?, until? }`,
//   who uses a preloaded entry. Entries are shared (one copy per URL), so
//   an opt-out only deletes the ones nobody else uses: a local user must
//   not be able to evict the copies other users rely on. More than
//   `MAX_REFS` users: `pinned` until `until`, never deleted by an opt-out
//   (it simply expires). A missing `refs:` never means "nobody uses it":
//   the image is kept and expires.
// - Order: `reserve` adds the recipients to `refs:` BEFORE the image is
//   looked up or downloaded, so a concurrent opt-out of another recipient
//   always sees them and never deletes an image they are about to use.
//   `charge` then writes `usage:` for the images really stored. A reserve
//   whose download failed only keeps an image a little longer.
// - `sweep` drops expired entries of `usage:` and `refs:` (and records left
//   empty); `prefs:` is never swept.
// - `prefs:<rcpt>`: `{ v, optOut, at }`. Applied only if newer (`at`) than
//   the stored one, so a replayed old request cannot undo a newer choice.
//
// Read-modify-write is serialized per key inside the process. Several
// instances (Vercel) can race: the quota is then approximate, like the
// rate limits. Reading failures fail closed: an unreadable preference or
// usage record means "do not preload this recipient".

import { z } from "zod";

import type { ImageStorage } from "../config";
import { storageKey } from "../storage-key";
import type { PreloadStateStore } from "./state";

export const DEFAULT_MAX_BYTES_PER_RECIPIENT = 256 * 1024 * 1024;
export const DEFAULT_MAX_IMAGES_PER_RECIPIENT = 10_000;
/** Recipients tracked per shared entry; more pins it. */
export const MAX_REFS = 20_000;
/** Not a JMAP id (`*`), so never an account's key. Same as hook.ts. */
const PRELOAD_ACCOUNT = "*preload";

const Hex = z.string().regex(/^[0-9a-f]{64}$/u);
const Time = z.number().int().nonnegative();

const Usage = z.object({
  e: z.record(Hex, z.tuple([z.number().int().nonnegative(), Time])),
  v: z.literal(1),
});
const Refs = z.object({
  pinned: z.boolean().optional(),
  r: z.record(Hex, Time),
  /** Pinned records only: when the pin expires (missing in old records). */
  until: Time.optional(),
  v: z.literal(1),
});
const Prefs = z.object({ at: Time, optOut: z.boolean(), v: z.literal(1) });

type UsageRecord = z.infer<typeof Usage>;
type RefsRecord = z.infer<typeof Refs>;

export interface UsageOptions {
  readonly store: PreloadStateStore;
  readonly images: ImageStorage;
  readonly ttlMs: number;
  readonly maxBytes?: number;
  readonly maxImages?: number;
  readonly now?: () => number;
  readonly onError?: () => void;
}

export interface Eligibility {
  /** Recipients that may be preloaded now. */
  readonly allowed: readonly string[];
  readonly optedOut: number;
  readonly full: number;
}

export interface ChargedImage {
  readonly urlHash: string;
  readonly bytes: number;
}

/** A full quota: preloading starts again at `until` (null: unknown). */
export interface FullState {
  readonly until: number | null;
}

export interface OptOutState {
  /** True only if every recipient is opted out. */
  readonly optOut: boolean;
  /** The recipients that are opted out now (their images get purged). */
  readonly optedOut: readonly string[];
}

/** "Off" only when every address is off (and there is at least one). */
const allOptedOut = (states: readonly boolean[]): boolean =>
  states.length > 0 && states.every(Boolean);

export interface UsageIndex {
  readonly check: (recipients: readonly string[]) => Promise<Eligibility>;
  /**
   * Add the recipients to `refs:` of these URL hashes, before the images
   * are looked up or downloaded. Returns the hashes reserved (a failed
   * write leaves that image out: it is not preloaded).
   */
  readonly reserve: (
    recipients: readonly string[],
    urlHashes: readonly string[]
  ) => Promise<ReadonlySet<string>>;
  /** Charge the stored images (already reserved) to the recipients' quota. */
  readonly charge: (
    recipients: readonly string[],
    images: readonly ChargedImage[]
  ) => Promise<void>;
  /**
   * True only if all of them opted out: the user sees "off" only when no
   * address of theirs is preloaded any more. A read error counts as opted
   * out for that address (fails closed).
   */
  readonly optedOut: (recipients: readonly string[]) => Promise<boolean>;
  /**
   * Store the choice (ignored if an equal or newer one is stored). Returns
   * the effective state (`optOut`: true only if all of them are now opted
   * out, as `optedOut`) and which ones are opted out (to purge).
   */
  readonly setOptOut: (
    recipients: readonly string[],
    optOut: boolean,
    at: number
  ) => Promise<OptOutState>;
  /**
   * Null if no address that still preloads is full. Otherwise when the
   * last of the full ones has room again (null: unknown, e.g. a read
   * error: fails closed like `check`). Shown to the user (Settings and
   * the privacy bar), so they know their mail is in `cache` for now.
   */
  readonly full: (recipients: readonly string[]) => Promise<FullState | null>;
  /** Drop a recipient's usage and every preloaded image only it used. */
  readonly purge: (recipient: string) => Promise<number>;
  /**
   * Drop the expired entries of every `usage:` and `refs:` record, and the
   * records left empty. Returns how many records were deleted.
   */
  readonly sweep: () => Promise<number>;
}

const parse = <T>(schema: z.ZodType<T>, text: string | null): T | null => {
  if (text === null) {
    return null;
  }
  try {
    const parsed = schema.safeParse(JSON.parse(text));
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
};

export const createUsageIndex = (options: UsageOptions): UsageIndex => {
  const { store, images, ttlMs } = options;
  const maxBytes = options.maxBytes ?? DEFAULT_MAX_BYTES_PER_RECIPIENT;
  const maxImages = options.maxImages ?? DEFAULT_MAX_IMAGES_PER_RECIPIENT;
  for (const value of [maxBytes, maxImages, ttlMs]) {
    if (!Number.isSafeInteger(value) || value <= 0) {
      throw new TypeError("image proxy: preload limits must be positive");
    }
  }
  const now = options.now ?? Date.now;
  const onError = options.onError ?? Boolean;
  const locks = new Map<string, Promise<unknown>>();

  /** Run `fn` after every earlier job on `key` in this process. */
  const withLock = async <T>(key: string, fn: () => Promise<T>): Promise<T> => {
    const previous = locks.get(key) ?? Promise.resolve();
    const job = previous.then(fn, fn);
    // The chain must go on after a failure; the caller sees the error.
    const tail = job.then(Boolean, Boolean);
    locks.set(key, tail);
    try {
      return await job;
    } finally {
      if (locks.get(key) === tail) {
        locks.delete(key);
      }
    }
  };

  const live = (record: UsageRecord | null): UsageRecord["e"] => {
    const t = now();
    return Object.fromEntries(
      Object.entries(record?.e ?? {}).filter(([, [, until]]) => until > t)
    );
  };

  const prefsOf = async (recipient: string): Promise<boolean> => {
    const text = await store.get(`prefs:${recipient}`);
    if (text === null) {
      return false;
    }
    // Unreadable: treated as opted out (fail closed).
    return parse(Prefs, text)?.optOut ?? true;
  };

  /**
   * Null if the recipient has room. If full, when enough entries expire to
   * leave room again (`until`), or null for a corrupt record.
   */
  const fullState = async (recipient: string): Promise<FullState | null> => {
    const text = await store.get(`usage:${recipient}`);
    const record = parse(Usage, text);
    if (text !== null && record === null) {
      // A corrupt record would reset the quota: count it as full instead.
      return { until: null };
    }
    const entries = Object.values(live(record)).toSorted(
      ([, a], [, b]) => a - b
    );
    let bytes = entries.reduce((sum, [size]) => sum + size, 0);
    let count = entries.length;
    if (bytes < maxBytes && count < maxImages) {
      return null;
    }
    // Oldest first: the first expiry that leaves room.
    for (const [size, until] of entries) {
      bytes -= size;
      count -= 1;
      if (bytes < maxBytes && count < maxImages) {
        return { until };
      }
    }
    return { until: null };
  };

  const isFull = async (recipient: string): Promise<boolean> =>
    (await fullState(recipient)) !== null;

  const chargeOne = async (
    recipient: string,
    charged: readonly ChargedImage[]
  ): Promise<void> => {
    await withLock(`usage:${recipient}`, async () => {
      const text = await store.get(`usage:${recipient}`);
      const entries = live(parse(Usage, text));
      const until = now() + ttlMs;
      for (const image of charged) {
        entries[image.urlHash] = [image.bytes, until];
      }
      await store.put(
        `usage:${recipient}`,
        JSON.stringify({ e: entries, v: 1 } satisfies UsageRecord)
      );
    });
  };

  const addRefs = async (
    urlHash: string,
    recipients: readonly string[]
  ): Promise<void> => {
    await withLock(`refs:${urlHash}`, async () => {
      const record = parse(Refs, await store.get(`refs:${urlHash}`));
      const t = now();
      const until = t + ttlMs;
      if (record?.pinned === true) {
        // Still shared by many: the pin lives as long as the newest use.
        await store.put(
          `refs:${urlHash}`,
          JSON.stringify({ pinned: true, r: {}, until, v: 1 })
        );
        return;
      }
      const refs: RefsRecord["r"] = Object.fromEntries(
        Object.entries(record?.r ?? {}).filter(([, at]) => at > t)
      );
      for (const recipient of recipients) {
        refs[recipient] = until;
      }
      const next: RefsRecord =
        Object.keys(refs).length > MAX_REFS
          ? { pinned: true, r: {}, until, v: 1 }
          : { r: refs, v: 1 };
      await store.put(`refs:${urlHash}`, JSON.stringify(next));
    });
  };

  /** Remove `recipient` from an entry; delete the image if nobody is left. */
  const release = async (
    urlHash: string,
    recipient: string
  ): Promise<boolean> =>
    await withLock(`refs:${urlHash}`, async () => {
      const text = await store.get(`refs:${urlHash}`);
      const record = parse(Refs, text);
      if (record === null || record.pinned === true) {
        // Missing (a reserve that failed, or swept), shared by many, or
        // unreadable: keep it, it expires anyway. Never "nobody uses it".
        return false;
      }
      const t = now();
      const refs = Object.fromEntries(
        Object.entries(record.r).filter(
          ([other, at]) => other !== recipient && at > t
        )
      );
      if (Object.keys(refs).length > 0) {
        await store.put(`refs:${urlHash}`, JSON.stringify({ r: refs, v: 1 }));
        return false;
      }
      await images.delete?.(await storageKey(PRELOAD_ACCOUNT, urlHash));
      await store.delete(`refs:${urlHash}`);
      return true;
    });

  const purge = async (recipient: string): Promise<number> =>
    await withLock(`usage:${recipient}`, async () => {
      const record = parse(Usage, await store.get(`usage:${recipient}`));
      const entries = record?.e ?? {};
      const pending: UsageRecord["e"] = {};
      let deleted = 0;
      for (const [urlHash, entry] of Object.entries(entries)) {
        try {
          // One at a time: an opt-out should not flood the storage.
          // oxlint-disable-next-line no-await-in-loop
          if (await release(urlHash, recipient)) {
            deleted += 1;
          }
        } catch {
          // Kept in `usage:`: the next opt-out finishes the job.
          pending[urlHash] = entry;
          onError();
        }
      }
      await (Object.keys(pending).length === 0
        ? store.delete(`usage:${recipient}`)
        : store.put(
            `usage:${recipient}`,
            JSON.stringify({ e: pending, v: 1 } satisfies UsageRecord)
          ));
      return deleted;
    });

  /** True if the record was deleted. Unreadable records are left as is. */
  const sweepUsage = async (key: string): Promise<boolean> =>
    await withLock(key, async () => {
      const record = parse(Usage, await store.get(key));
      if (record === null) {
        // Missing, or unreadable (counts as full: fail closed).
        return false;
      }
      const entries = live(record);
      const left = Object.keys(entries).length;
      if (left === 0) {
        await store.delete(key);
        return true;
      }
      if (left !== Object.keys(record.e).length) {
        await store.put(
          key,
          JSON.stringify({ e: entries, v: 1 } satisfies UsageRecord)
        );
      }
      return false;
    });

  const sweepRefs = async (key: string): Promise<boolean> =>
    await withLock(key, async () => {
      const record = parse(Refs, await store.get(key));
      if (record === null) {
        return false;
      }
      const t = now();
      if (record.pinned === true) {
        if (record.until === undefined) {
          // Pinned before `until` existed: from now on it expires.
          await store.put(
            key,
            JSON.stringify({ pinned: true, r: {}, until: t + ttlMs, v: 1 })
          );
          return false;
        }
        if (record.until > t) {
          return false;
        }
        await store.delete(key);
        return true;
      }
      const refs = Object.fromEntries(
        Object.entries(record.r).filter(([, at]) => at > t)
      );
      const left = Object.keys(refs).length;
      if (left === 0) {
        await store.delete(key);
        return true;
      }
      if (left !== Object.keys(record.r).length) {
        await store.put(key, JSON.stringify({ r: refs, v: 1 }));
      }
      return false;
    });

  const sweep = async (): Promise<number> => {
    let deleted = 0;
    for (const [kind, one] of [
      ["usage", sweepUsage],
      ["refs", sweepRefs],
    ] as const) {
      // One record at a time: a sweep should not flood the storage.
      // oxlint-disable-next-line no-await-in-loop
      for await (const key of store.list(kind)) {
        try {
          // oxlint-disable-next-line no-await-in-loop
          if (await one(key)) {
            deleted += 1;
          }
        } catch {
          onError();
        }
      }
    }
    return deleted;
  };

  return {
    charge: async (recipients, charged) => {
      if (recipients.length === 0 || charged.length === 0) {
        return;
      }
      const jobs = recipients.map(async (recipient) => {
        await chargeOne(recipient, charged);
      });
      for (const result of await Promise.allSettled(jobs)) {
        if (result.status === "rejected") {
          onError();
        }
      }
    },

    check: async (recipients) => {
      const allowed: string[] = [];
      let optedOut = 0;
      let full = 0;
      await Promise.all(
        recipients.map(async (recipient) => {
          try {
            if (await prefsOf(recipient)) {
              optedOut += 1;
            } else if (await isFull(recipient)) {
              full += 1;
            } else {
              allowed.push(recipient);
            }
          } catch {
            // Fail closed: this recipient is not preloaded.
            onError();
            full += 1;
          }
        })
      );
      return { allowed, full, optedOut };
    },

    full: async (recipients) => {
      const states = await Promise.all(
        recipients.map(async (recipient): Promise<FullState | null> => {
          try {
            return (await prefsOf(recipient))
              ? null
              : await fullState(recipient);
          } catch {
            onError();
            return { until: null };
          }
        })
      );
      const full = states.filter((state) => state !== null);
      if (full.length === 0) {
        return null;
      }
      const untils = full.map((state) => state.until);
      return untils.includes(null)
        ? { until: null }
        : { until: Math.max(...untils.filter((until) => until !== null)) };
    },

    optedOut: async (recipients) => {
      const states = await Promise.all(
        recipients.map(async (recipient) => {
          try {
            return await prefsOf(recipient);
          } catch {
            onError();
            return true;
          }
        })
      );
      return allOptedOut(states);
    },

    purge,

    reserve: async (recipients, urlHashes) => {
      const reserved = new Set<string>();
      if (recipients.length === 0) {
        return reserved;
      }
      await Promise.all(
        [...new Set(urlHashes)].map(async (urlHash) => {
          try {
            await addRefs(urlHash, recipients);
            reserved.add(urlHash);
          } catch {
            onError();
          }
        })
      );
      return reserved;
    },

    setOptOut: async (recipients, optOut, at) => {
      const states = await Promise.all(
        recipients.map(
          async (recipient) =>
            await withLock(`prefs:${recipient}`, async () => {
              const text = await store.get(`prefs:${recipient}`);
              const current = parse(Prefs, text);
              if (current !== null && current.at >= at) {
                return current.optOut;
              }
              await store.put(
                `prefs:${recipient}`,
                JSON.stringify({ at, optOut, v: 1 })
              );
              return optOut;
            })
        )
      );
      return {
        optOut: allOptedOut(states),
        optedOut: recipients.filter((_, index) => states[index] === true),
      };
    },

    sweep,
  };
};
