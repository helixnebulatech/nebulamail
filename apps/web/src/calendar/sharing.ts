// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Sharing a calendar (or a task list, which is a calendar.8)
// with `Calendar.shareWith` (JMAP Calendars, by principal id) and the
// people of `Principal/query`, like the address books (contacts/
// sharing.ts). Calendars shared with us are other accounts of the session
// (api.ts lists them under "Shared by…").
//
// Stalwart (checked live, calendar/sharing.live.test.ts):
// `shareWith/<id>` in an update; the rights come back exactly as sent,
// all eight of them; an unknown principal is `invalidProperties`. Someone
// who may only see free/busy doesn't get the account at all (their
// `Calendar/get` there is `forbidden`). The bridge maps
// any write right to DAV `write`, which also lets them delete the
// calendar, so its rights come back with `mayDelete: true`; CalendarServer
// servers (Baïkal) only know read and read-write.

import type {
  AccountId,
  CalendarId,
  CalendarRights,
  JmapClient,
} from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  PrincipalId,
  isJmapError,
} from "@nebulamail/jmap-client";

import { getClient } from "../mail/client";
import type { Person, Share } from "../mail/sharing";
import { EventSaveError } from "./api";

/** What the share dialog offers, least to most. */
export const CALENDAR_SHARE_LEVELS = [
  "freeBusy",
  "read",
  "write",
  "manage",
] as const;
export type CalendarShareLevel = (typeof CALENDAR_SHARE_LEVELS)[number];

const RIGHT_NAMES = [
  "mayReadFreeBusy",
  "mayReadItems",
  "mayRSVP",
  "mayShare",
  "mayUpdatePrivate",
  "mayWriteAll",
  "mayWriteOwn",
] as const satisfies readonly (keyof CalendarRights)[];

/**
 * - freeBusy: only when they are busy (for scheduling), not the events;
 * - read: the events;
 * - write: also add, change and delete events;
 * - manage: also share it with others.
 * Deleting the calendar itself (`mayDelete`) is never asked for.
 */
export const calendarRightsFor = (
  level: CalendarShareLevel
): Required<CalendarRights> => {
  const read = level !== "freeBusy";
  const write = level === "write" || level === "manage";
  return {
    mayDelete: false,
    mayReadFreeBusy: true,
    mayReadItems: read,
    mayRSVP: write,
    mayShare: level === "manage",
    mayUpdatePrivate: write,
    mayWriteAll: write,
    mayWriteOwn: write,
  };
};

/**
 * The level these rights match, or "custom" (set by another client:
 * shown as is, never rewritten). `mayDelete` comes along with writing
 * through the bridge, so it only makes a reading level custom.
 */
export const calendarLevelOf = (
  rights: CalendarRights
): CalendarShareLevel | "custom" =>
  CALENDAR_SHARE_LEVELS.find((level) => {
    const wanted = calendarRightsFor(level);
    return (
      RIGHT_NAMES.every(
        (name) => (rights[name] ?? false) === (wanted[name] ?? false)
      ) &&
      (rights.mayDelete !== true || wanted.mayWriteAll === true)
    );
  }) ?? "custom";

/** The `Calendar/set` update for one person; `null` stops sharing. */
export const calendarSharePatch = (
  principalId: PrincipalId,
  level: CalendarShareLevel | null
): Record<`shareWith/${string}`, CalendarRights | null> => ({
  [`shareWith/${principalId}`]:
    level === null ? null : calendarRightsFor(level),
});

/** The calendar being shared: its id and the account it lives in. */
export interface SharedCalendarRef {
  readonly id: CalendarId;
  readonly accountId: AccountId;
}

/**
 * Who a calendar is shared with, and how (`getCalendarShares` with a
 * given connection: live tests).
 */
export const calendarSharesOf = async (
  client: JmapClient,
  calendar: SharedCalendarRef,
  people: readonly Person[]
): Promise<Share<CalendarShareLevel>[]> => {
  const [result] = await client.request((b) => [
    b.call("Calendar/get", {
      accountId: calendar.accountId,
      ids: [calendar.id],
      properties: ["shareWith"],
    }),
  ]);
  const shareWith =
    result.list.find((item) => item.id === calendar.id)?.shareWith ?? {};
  return Object.entries(shareWith).flatMap(([id, rights]) => {
    if (rights === undefined) {
      return [];
    }
    const person = people.find((candidate) => candidate.id === id) ?? {
      email: null,
      id: PrincipalId.parse(id),
      name: id,
      type: "individual",
    };
    return [{ level: calendarLevelOf(rights), person }];
  });
};

export const getCalendarShares = async (
  calendar: SharedCalendarRef,
  people: readonly Person[]
): Promise<Share<CalendarShareLevel>[]> =>
  await calendarSharesOf(await getClient(), calendar, people);

/**
 * Shares a calendar with someone (or changes how); `null` stops
 * (`setCalendarShare` with a given connection: live tests).
 */
export const shareCalendarWith = async (
  client: JmapClient,
  calendar: SharedCalendarRef,
  principalId: PrincipalId,
  level: CalendarShareLevel | null
): Promise<void> => {
  const [result] = await client.request((b) => {
    b.using(CAPABILITIES.principals);
    return [
      b.call("Calendar/set", {
        accountId: calendar.accountId,
        update: { [calendar.id]: calendarSharePatch(principalId, level) },
      }),
    ];
  });
  const error = result.notUpdated[calendar.id];
  if (error !== undefined) {
    throw new EventSaveError(error.type, error.description ?? null);
  }
};

export const setCalendarShare = async (
  calendar: SharedCalendarRef,
  principalId: PrincipalId,
  level: CalendarShareLevel | null
): Promise<void> => {
  await shareCalendarWith(await getClient(), calendar, principalId, level);
};

/**
 * Why sharing failed, for the message: the server refused ("forbidden"),
 * didn't take that person ("invalidProperties") or something else.
 */
export const shareFailure = (
  error: unknown
): "forbidden" | "invalidProperties" | "other" => {
  let type: string | null = null;
  if (error instanceof EventSaveError) {
    ({ type } = error);
  } else if (isJmapError(error) && error.detail.kind === "method") {
    ({ type } = error.detail);
  }
  return type === "forbidden" || type === "invalidProperties" ? type : "other";
};
