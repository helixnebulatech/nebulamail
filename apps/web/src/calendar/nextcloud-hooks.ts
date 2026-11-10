// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// React hooks over Nextcloud's calendar extras (calendar/nextcloud.ts).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { calendarKeys } from "./hooks";
import type { CalendarSummary } from "./model";
import {
  NO_EXTRAS,
  calendarExtras,
  changeTrash,
  getTrash,
  setPublished,
  subscribe,
} from "./nextcloud";

const extrasKey = () => [...calendarKeys.all, "extras"] as const;
const trashKey = () => [...calendarKeys.all, "trash"] as const;

/** What the server offers (nothing until known). */
export const useCalendarExtras = () =>
  useQuery({
    queryFn: calendarExtras,
    queryKey: extrasKey(),
    staleTime: Infinity,
  }).data ?? NO_EXTRAS;

const useRefresh = () => {
  const client = useQueryClient();
  return async (): Promise<void> => {
    await client.invalidateQueries({ queryKey: calendarKeys.all });
  };
};

export const useSetPublished = () => {
  const refresh = useRefresh();
  return useMutation({
    mutationFn: async (input: {
      readonly calendar: Pick<CalendarSummary, "accountId" | "id">;
      readonly published: boolean;
    }) => {
      await setPublished(input.calendar, input.published);
    },
    onSettled: refresh,
  });
};

export const useSubscribe = () => {
  const refresh = useRefresh();
  return useMutation({
    mutationFn: subscribe,
    onSettled: refresh,
  });
};

/** The trash bin, while `open`. */
export const useTrash = (open: boolean) =>
  useQuery({ enabled: open, queryFn: getTrash, queryKey: trashKey() });

export const useChangeTrash = () => {
  const refresh = useRefresh();
  return useMutation({
    mutationFn: async (input: {
      readonly action: "restore" | "destroy";
      readonly ids: readonly string[];
    }) => {
      await changeTrash(input.action, input.ids);
    },
    onSettled: refresh,
  });
};
