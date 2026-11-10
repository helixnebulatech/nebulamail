// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// React hooks over the calendar domain functions (TanStack Query). Only in
// the browser: the access token lives in memory, never during SSR.

import type { CalendarDate } from "@internationalized/date";
import type {
  AccountId,
  BlobId,
  CalendarEventId,
} from "@nebulamail/jmap-client";
import {
  queryOptions,
  skipToken,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";

import {
  calendarContents,
  createCalendar,
  deleteCalendar,
  getAvailability,
  getEvent,
  listCalendars,
  listOccurrences,
  listOwnAddresses,
  me,
  updateCalendar,
} from "./api";
import { isSafeColor } from "./colors";
import type { SettingsPatch } from "./defaults";
import { loadInvitation } from "./invitations";
import type { InvitationSender } from "./itip";
import type { CalendarSummary, Occurrence } from "./model";
import { watchCalendars } from "./push";
import { browserTimeZone } from "./time";

export const calendarKeys = {
  all: ["calendar"] as const,
  availability: (emails: readonly string[], from: number, to: number) =>
    [...calendarKeys.all, "availability", emails.join(","), from, to] as const,
  calendars: () => [...calendarKeys.all, "calendars"] as const,
  contents: (accountId: string, calendarId: string) =>
    [...calendarKeys.all, "contents", accountId, calendarId] as const,
  event: (id: CalendarEventId) => [...calendarKeys.events(), id] as const,
  events: () => [...calendarKeys.all, "event"] as const,
  invitation: (blobId: BlobId) =>
    [...calendarKeys.all, "invitation", blobId] as const,
  me: () => [...calendarKeys.all, "me"] as const,
  occurrences: () => [...calendarKeys.all, "occurrences"] as const,
  own: () => [...calendarKeys.all, "own"] as const,
  range: (start: string, end: string, zone: string) =>
    [...calendarKeys.occurrences(), start, end, zone] as const,
};

/** The user's zone. One value per tab; the browser decides. */
export const useTimeZone = (): string =>
  typeof Intl === "undefined" ? "UTC" : browserTimeZone();

/** Also the /calendar route's loader (lib/route-prefetch.ts). */
export const calendarsQuery = () =>
  queryOptions({ queryFn: listCalendars, queryKey: calendarKeys.calendars() });

export const ownAddressesQuery = () =>
  queryOptions({
    queryFn: listOwnAddresses,
    queryKey: calendarKeys.own(),
    staleTime: Infinity,
  });

export const meQuery = () =>
  queryOptions({
    queryFn: me,
    queryKey: calendarKeys.me(),
    staleTime: Infinity,
  });

/** `enabled: false`: only what is cached, no request (a mail may not need it). */
export const useCalendars = (enabled = true) =>
  useQuery({ ...calendarsQuery(), enabled });

export const useOwnCalendarAddresses = (enabled = true) =>
  useQuery({ ...ownAddressesQuery(), enabled });

export const useMe = () => useQuery(meQuery());

/** Occurrences of [start, end) once calendars and addresses are known. */
export const useOccurrences = (start: CalendarDate, end: CalendarDate) => {
  const zone = useTimeZone();
  const calendars = useCalendars();
  const own = useOwnCalendarAddresses();
  const ready = calendars.data !== undefined && own.data !== undefined;
  return useQuery({
    placeholderData: (previous: Occurrence[] | undefined) => previous,
    queryFn: ready
      ? async () =>
          await listOccurrences({
            calendars: calendars.data,
            end,
            own: own.data,
            start,
            zone,
          })
      : skipToken,
    queryKey: [
      ...calendarKeys.range(start.toString(), end.toString(), zone),
      // Colours and the user's addresses are part of the mapped result.
      // So are the accounts queried (calendars shared or unshared).
      (calendars.data ?? [])
        .map((calendar) => `${calendar.key}=${calendar.color}`)
        .join(","),
    ],
  });
};

export const useEventDetail = (
  target: {
    readonly id: CalendarEventId;
    readonly baseId: CalendarEventId;
    /** null: the user's own account. */
    readonly accountId: AccountId | null;
  } | null
) => {
  const zone = useTimeZone();
  const calendars = useCalendars();
  const own = useOwnCalendarAddresses();
  const ready =
    target !== null && calendars.data !== undefined && own.data !== undefined;
  return useQuery({
    queryFn: ready
      ? async () =>
          await getEvent(
            target.id,
            target.baseId,
            zone,
            own.data,
            calendars.data,
            target.accountId
          )
      : skipToken,
    queryKey: [
      ...calendarKeys.events(),
      target?.accountId ?? "",
      target?.id ?? "",
      zone,
    ],
  });
};

export const useInvitation = (
  blobId: BlobId | null,
  sender: InvitationSender,
  enabled: boolean
) => {
  const zone = useTimeZone();
  // Only for a mail that shows an invitation: not on every mail opened.
  const wanted = enabled && blobId !== null;
  const calendars = useCalendars(wanted);
  const own = useOwnCalendarAddresses(wanted);
  const ready =
    wanted && calendars.data !== undefined && own.data !== undefined;
  return useQuery({
    queryFn: ready
      ? async () =>
          await loadInvitation(blobId, sender, {
            calendars: calendars.data,
            own: own.data,
            zone,
          })
      : skipToken,
    queryKey: [
      ...calendarKeys.all,
      "invitation",
      blobId ?? "",
      sender.email ?? "",
      sender.verdict ?? "",
      zone,
    ],
    retry: false,
  });
};

export const useAvailability = (
  emails: readonly string[],
  from: number,
  to: number
) =>
  useQuery({
    queryFn:
      emails.length === 0
        ? skipToken
        : async () => await getAvailability(emails, from, to),
    queryKey: calendarKeys.availability(emails, from, to),
    staleTime: 60_000,
  });

/**
 * Kinds of `calendarKeys.all` (the second part of the key) no change of
 * calendars or events touches: whether the server has calendars and the
 * user's own addresses and principal (ParticipantIdentity, Principal).
 */
const STABLE: ReadonlySet<string> = new Set(["available", "me", "own"]);
/** Kinds that only a change of the calendars themselves touches. */
const CALENDAR_ONLY: ReadonlySet<string> = new Set([
  ...STABLE,
  "calendars",
  "new-shares",
]);

const refreshExcept = async (
  client: QueryClient,
  skip: ReadonlySet<string>
): Promise<void> => {
  await client.invalidateQueries({
    predicate: ({ queryKey: [, kind] }) => !skip.has(String(kind)),
    queryKey: calendarKeys.all,
  });
};

/** After a change of calendars: every list, the open event and invitations. */
export const refreshCalendar = async (client: QueryClient): Promise<void> => {
  await refreshExcept(client, STABLE);
};

/**
 * After a change of events only: occurrences, the open event, invitations,
 * availability… but not the calendar list (nor what never changes).
 */
export const refreshEvents = async (client: QueryClient): Promise<void> => {
  await refreshExcept(client, CALENDAR_ONLY);
};

export interface CalendarChange {
  readonly calendar: CalendarSummary;
  readonly visible?: boolean;
  readonly color?: string;
  readonly name?: string;
  /** Time zone and default alerts (`settingsPatch`). */
  readonly settings?: SettingsPatch;
}

/**
 * Show or hide a calendar, change its colour or name, at once (rolled back
 * if the server says no).
 */
export const useUpdateCalendar = () => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async ({ calendar, ...change }: CalendarChange) => {
      await updateCalendar(calendar, change);
    },
    onError: (
      _error,
      _variables,
      saved: { readonly list?: CalendarSummary[] } | undefined
    ) => {
      client.setQueryData(calendarKeys.calendars(), saved?.list);
    },
    onMutate: ({ calendar, color, name, visible }) => {
      const saved = client.getQueryData<CalendarSummary[]>(
        calendarKeys.calendars()
      );
      client.setQueryData<CalendarSummary[]>(
        calendarKeys.calendars(),
        saved?.map((item) =>
          item.key === calendar.key
            ? {
                ...item,
                ...(visible === undefined ? {} : { visible }),
                ...(color !== undefined && isSafeColor(color) ? { color } : {}),
                ...(name === undefined || name.trim() === ""
                  ? {}
                  : { name: name.trim() }),
              }
            : item
        )
      );
      return saved === undefined ? {} : { list: saved };
    },
    onSettled: async () => {
      // Colours are part of every mapped occurrence.
      await refreshCalendar(client);
    },
  });
};

/** Show or hide a calendar (the sidebar's tick). */
export const useToggleCalendar = () => {
  const update = useUpdateCalendar();
  return {
    ...update,
    mutate: ({
      calendar,
      visible,
    }: {
      readonly calendar: CalendarSummary;
      readonly visible: boolean;
    }) => {
      update.mutate({ calendar, visible });
    },
  };
};

/** New calendar in the user's own account. */
export const useCreateCalendar = () => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async (input: {
      readonly name: string;
      readonly color: string;
      readonly settings?: SettingsPatch;
      readonly shareWith?: NonNullable<
        Parameters<typeof createCalendar>[0]["shareWith"]
      >;
    }) => await createCalendar(input),
    onSettled: async () => {
      await refreshCalendar(client);
    },
  });
};

/**
 * What else a calendar holds (tasks, other apps' objects), for the delete
 * dialog. Always fresh: asked when the dialog opens.
 */
export const useCalendarContents = (calendar: CalendarSummary | null) =>
  useQuery({
    enabled: calendar !== null,
    gcTime: 0,
    queryFn: async () => {
      if (calendar === null) {
        throw new Error("no calendar");
      }
      return await calendarContents(calendar);
    },
    queryKey: calendarKeys.contents(
      calendar?.accountId ?? "",
      calendar?.id ?? ""
    ),
    staleTime: 0,
  });

/** Deletes a calendar with its events (the UI asks first). */
export const useDeleteCalendar = () => {
  const client = useQueryClient();
  return useMutation({
    mutationFn: async (calendar: CalendarSummary) => {
      await deleteCalendar(calendar);
    },
    onSettled: async () => {
      await refreshCalendar(client);
    },
  });
};

/** Live updates of calendars and events (JMAP push) while mounted. */
export const useCalendarPush = (): void => {
  const queryClient = useQueryClient();
  useEffect(() => {
    const controller = new AbortController();
    // Push is a nicety: without it the views refresh when they are opened.
    watchCalendars((change) => {
      if (change === "reconnected") {
        // Anything may have changed meanwhile.
        void queryClient.invalidateQueries({ queryKey: calendarKeys.all });
      } else if (change === "calendars") {
        void refreshCalendar(queryClient);
      } else {
        void refreshEvents(queryClient);
      }
    }, controller.signal).catch(() => null);
    return () => {
      controller.abort();
    };
  }, [queryClient]);
};
