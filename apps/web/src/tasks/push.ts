// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Live updates of Tasks (JMAP push over SSE, like calendar/push.ts): tasks
// live in calendars, so a Calendar or CalendarEvent change means "fetch
// again". A task ticked on a phone shows up without reloading.

import { getClient } from "../mail/client";

const TYPES = ["Calendar", "CalendarEvent"] as const;

export interface TasksPush {
  /**
   * A Calendar changed (a list was added, renamed or removed), or the push
   * reconnected and anything may have.
   */
  readonly calendars: boolean;
  /** The new `CalendarEvent` state per account (RFC 8620 §7.1). */
  readonly events: Readonly<Record<string, string>>;
}

/** Calls `onChange` for each calendar change (and after a reconnect). */
export const watchTasks = async (
  onChange: (change: TasksPush) => void,
  signal: AbortSignal
): Promise<void> => {
  const client = await getClient();
  const changes = client.push({
    onConnect: () => {
      onChange({ calendars: true, events: {} });
    },
    signal,
    types: TYPES,
  });
  for await (const change of changes) {
    const types = new Set(
      Object.values(change.changed).flatMap((perAccount) =>
        Object.keys(perAccount ?? {})
      )
    );
    if (TYPES.some((type) => types.has(type))) {
      const events: Record<string, string> = {};
      for (const [accountId, perAccount] of Object.entries(change.changed)) {
        const state = perAccount?.CalendarEvent;
        if (state !== undefined) {
          events[accountId] = state;
        }
      }
      onChange({ calendars: types.has("Calendar"), events });
    }
  }
};
