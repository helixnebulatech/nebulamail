// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Live updates of the calendar (JMAP push over SSE, like mail/push.ts):
// a change only says which types changed (RFC 8620 §7.1 StateChange), so
// the views fetch again: only the events, unless the calendars changed.

import { getClient } from "../mail/client";

/** What a push said changed: the calendars, only events, or unknown. */
export type CalendarChangeKind = "calendars" | "events" | "reconnected";

/** Calls `onChange` for each calendar change (and after a reconnect). */
export const watchCalendars = async (
  onChange: (change: CalendarChangeKind) => void,
  signal: AbortSignal
): Promise<void> => {
  const client = await getClient();
  const changes = client.push({
    onConnect: () => {
      onChange("reconnected");
    },
    signal,
    types: ["Calendar", "CalendarEvent"],
  });
  for await (const change of changes) {
    const types = new Set(
      Object.values(change.changed).flatMap((perAccount) =>
        Object.keys(perAccount ?? {})
      )
    );
    if (types.has("Calendar")) {
      onChange("calendars");
    } else if (types.has("CalendarEvent")) {
      onChange("events");
    }
  }
};
