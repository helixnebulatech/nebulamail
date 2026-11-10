// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Which account an event is in, for links and calls. The user's own
// account is written as null (URLs stay as they were); calendars shared by
// others keep their owner's account id.

import type { AccountId, CalendarEventId } from "@nebulamail/jmap-client";

import type { CalendarSummary } from "./model";

export interface EventRef {
  readonly id: CalendarEventId;
  readonly baseId: CalendarEventId;
  /** null: the user's own account. */
  readonly accountId: AccountId | null;
}

/** The user's own calendars account, from the calendar list. */
export const ownAccountOf = (
  calendars: readonly CalendarSummary[]
): AccountId | null =>
  calendars.find((calendar) => calendar.owner === null)?.accountId ?? null;

/** null for the user's own account, else the account id. */
export const accountRef = (
  accountId: AccountId,
  calendars: readonly CalendarSummary[]
): AccountId | null => {
  const own = ownAccountOf(calendars);
  return own === null || own === accountId ? null : accountId;
};

export const refOf = (
  event: {
    readonly id: CalendarEventId;
    readonly baseId: CalendarEventId;
    readonly accountId: AccountId;
  },
  calendars: readonly CalendarSummary[]
): EventRef => ({
  accountId: accountRef(event.accountId, calendars),
  baseId: event.baseId,
  id: event.id,
});
