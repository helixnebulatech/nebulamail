// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which email ids to fetch for threads, within the server's limits
// (`maxObjectsInGet`) and ours (huge mailing-list threads are cut to their
// newest emails).

/** Splits `items` into pieces of at most `size` (at least 1). */
export const chunk = <T>(items: readonly T[], size: number): T[][] => {
  const step = Math.max(1, Math.floor(size));
  const pieces: T[][] = [];
  for (let start = 0; start < items.length; start += step) {
    pieces.push(items.slice(start, start + step));
  }
  return pieces;
};

/**
 * The newest `max` ids of a thread (ids come oldest first, RFC 8621 §3),
 * always including `keep` (the email that was opened), in thread order.
 */
export const newestIds = <Id>(
  ids: readonly Id[],
  max: number,
  keep?: Id
): Id[] => {
  if (ids.length <= max) {
    return [...ids];
  }
  const newest = ids.slice(ids.length - Math.max(0, max));
  if (keep === undefined || newest.includes(keep) || !ids.includes(keep)) {
    return newest;
  }
  return [keep, ...newest.slice(1)];
};

/**
 * The thread emails a list page still needs (not already fetched as rows),
 * at most `budget` in total: every thread gets an equal share of its newest
 * emails, so one huge thread can't crowd out the others.
 */
export const memberIdsToFetch = <Id>(
  threads: readonly (readonly Id[])[],
  known: ReadonlyMap<Id, unknown>,
  budget: number
): Id[] => {
  const multi = threads.filter((ids) => ids.length > 1);
  if (multi.length === 0) {
    return [];
  }
  const share = Math.max(1, Math.floor(budget / multi.length));
  const wanted = new Set<Id>();
  for (const ids of multi) {
    for (const id of newestIds(ids, share)) {
      if (!known.has(id)) {
        wanted.add(id);
      }
    }
  }
  return [...wanted];
};
