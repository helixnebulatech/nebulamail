// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The settings queries the mail UI also uses (push invalidation, the
// vacation banner), apart from queries.ts so the inbox doesn't load the
// Sieve parser and generator: the vacation response is read through a
// dynamic import. Same query keys, so the banner and the settings page
// share the cache.

import type { EmailId, SieveScriptId } from "@nebulamail/jmap-client";
import {
  CAPABILITIES,
  hasCapability,
  sieveCapability,
} from "@nebulamail/jmap-client";
import { queryOptions, useQuery } from "@tanstack/react-query";

import { getClient } from "../mail/client";

export const settingsKeys = {
  all: ["settings"] as const,
  email: (id: EmailId) => [...settingsKeys.all, "email", id] as const,
  imapNames: () => [...settingsKeys.all, "imap-names"] as const,
  scripts: () => [...settingsKeys.all, "scripts"] as const,
  scriptText: (id: SieveScriptId, blobId: string) =>
    [...settingsKeys.all, "script-text", id, blobId] as const,
  support: () => [...settingsKeys.all, "support"] as const,
  vacation: () => [...settingsKeys.all, "vacation"] as const,
};

export interface Support {
  readonly sieve: boolean;
  readonly vacation: boolean;
  /** Sieve extensions the server has (e.g. "regex", "body"). */
  readonly extensions: readonly string[];
}

const getSupport = async (): Promise<Support> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const { session } = client;
  return {
    extensions: sieveCapability(session, accountId)?.sieveExtensions ?? [],
    sieve: hasCapability(session, CAPABILITIES.sieve, accountId),
    vacation: hasCapability(session, CAPABILITIES.vacationResponse, accountId),
  };
};

export const supportQuery = () =>
  queryOptions({
    queryFn: getSupport,
    queryKey: settingsKeys.support(),
    staleTime: Infinity,
  });

export const vacationQuery = () =>
  queryOptions({
    queryFn: async () => {
      const { getVacation } = await import("./vacation/api");
      return await getVacation();
    },
    queryKey: settingsKeys.vacation(),
    // The banner in the mail UI asks too; a few minutes is fresh enough.
    staleTime: 5 * 60_000,
  });

export const useSupport = () => useQuery(supportQuery());

// Only ask when the server has it: a server without Sieve or vacation
// responses (the IMAP bridge) rejects the whole request.
export const useVacation = () => {
  const support = useSupport();
  return useQuery({
    ...vacationQuery(),
    enabled: support.data?.vacation === true,
  });
};
