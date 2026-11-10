// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Scheduled send for the UI (schedule.ts): whether the account can, how far
// ahead, and the messages waiting to go out.

import type { EmailId } from "@nebulamail/jmap-client";
import { useQuery } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";

import { useNow } from "../lib/use-now";
import { getClient } from "./client";
import { mailKeys } from "./queries";
import type { ScheduleLimit } from "./schedule";
import { limitFor, listScheduled, scheduledByEmail } from "./schedule";
import { canSendSignedOut } from "./send-signed-out";

export const scheduleKeys = {
  /** The pending submissions (the «Programados» view). */
  list: () => [...mailKeys.all, "scheduled"] as const,
  /** The account's limit; refreshed after the server says less (schedule.ts). */
  limit: () => [...mailKeys.all, "support", "schedule"] as const,
  /** Whether scheduled mail can go out after sign-out (the bridge). */
  signedOut: () => [...mailKeys.all, "support", "send-signed-out"] as const,
};

/**
 * Whether the server sends scheduled mail after sign-out
 * (send-signed-out.ts); false until known.
 */
export const useSendSignedOutSupport = (): boolean =>
  useQuery({
    queryFn: async () => canSendSignedOut(await getClient()),
    queryKey: scheduleKeys.signedOut(),
    staleTime: Number.POSITIVE_INFINITY,
  }).data ?? false;

const useLimitQuery = () =>
  useQuery({
    queryFn: async () => limitFor(await getClient()),
    queryKey: scheduleKeys.limit(),
    staleTime: Number.POSITIVE_INFINITY,
  });

/** How far ahead the account can schedule; null if it can't (or unknown yet). */
export const useScheduleLimit = (): ScheduleLimit | null =>
  useLimitQuery().data ?? null;

/** The limit, and whether it is still being found out. */
export const useScheduleSupport = (): {
  readonly limit: ScheduleLimit | null;
  readonly loading: boolean;
} => {
  const { data, isPending } = useLimitQuery();
  return { limit: data ?? null, loading: isPending };
};

/**
 * After cancelling or moving a scheduled message: the list, and the
 * folders it moves between (Sent, Drafts) with their counts.
 */
export const refreshAfterChange = async (
  queryClient: QueryClient
): Promise<void> => {
  await Promise.all(
    [
      scheduleKeys.list(),
      mailKeys.lists(),
      mailKeys.mailboxes(),
      mailKeys.emails(),
    ].map(async (queryKey) => {
      await queryClient.invalidateQueries({ queryKey });
    })
  );
};

export const useScheduled = (enabled: boolean) =>
  useQuery({
    enabled,
    queryFn: listScheduled,
    queryKey: scheduleKeys.list(),
    // A message leaves the list when its time comes.
    refetchInterval: 60_000,
  });

const NONE: ReadonlyMap<EmailId, string> = new Map();

/**
 * Sent and the reader: when each scheduled email goes out, from the
 * «Programados» query (one request for the whole list, refreshed every
 * minute while shown; none per row). Empty when the account can't
 * schedule or `enabled` is false.
 */
export const useScheduledSendAt = (
  enabled: boolean
): ReadonlyMap<EmailId, string> => {
  const limit = useScheduleLimit();
  const { data } = useScheduled(enabled && limit !== null);
  // The clock of the list's times: a message whose time came stops being
  // «Programado» within a minute, even before the list refreshes.
  const now = useNow();
  return data === undefined || !enabled || now === 0
    ? NONE
    : scheduledByEmail(data, new Date(now));
};
