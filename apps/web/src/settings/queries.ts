// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// TanStack Query options and hooks for the settings pages. Browser only,
// like the mail data (the token lives in memory).

import type { EmailId, SieveScriptId } from "@nebulamail/jmap-client";
import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import {
  emailFacts,
  imapNames,
  saveFilters,
  saveRawScript,
} from "./filters/api";
import type { FilterSet } from "./filters/model";
import type { ScriptInfo, ScriptsState } from "./scripts";
import {
  activateScript,
  deleteScript,
  loadScripts,
  readScriptText,
  renameScript,
  saveScript,
} from "./scripts";
import { settingsKeys, useSupport } from "./shared-queries";
import type { VacationSettings } from "./vacation/api";
import { saveVacation } from "./vacation/api";

export type { Support } from "./shared-queries";
export {
  settingsKeys,
  supportQuery,
  useSupport,
  useVacation,
  vacationQuery,
} from "./shared-queries";

export const scriptsQuery = () =>
  queryOptions({ queryFn: loadScripts, queryKey: settingsKeys.scripts() });

export const useImapNames = () =>
  useQuery({ queryFn: imapNames, queryKey: settingsKeys.imapNames() });
// Only ask when the server has it: a server without Sieve or vacation
// responses (the IMAP bridge) rejects the whole request.
export const useScripts = () => {
  const support = useSupport();
  return useQuery({ ...scriptsQuery(), enabled: support.data?.sieve === true });
};

export const useEmailFacts = (id: EmailId | undefined) =>
  useQuery({
    enabled: id !== undefined,
    queryFn: async () => (id === undefined ? null : await emailFacts(id)),
    queryKey: id === undefined ? settingsKeys.all : settingsKeys.email(id),
    staleTime: Infinity,
  });

/**
 * Saves the rules. The list shows the new rules at once (rule ids stay the
 * same, so focus survives a reorder); on failure it reloads from the server.
 */
export const useSaveFilters = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      filters,
      state,
    }: {
      readonly filters: FilterSet;
      readonly state: ScriptsState;
    }) => {
      await saveFilters(filters, state);
    },
    onError: async () => {
      await queryClient.invalidateQueries({ queryKey: settingsKeys.all });
    },
    onMutate: ({ filters, state }) => {
      queryClient.setQueryData<ScriptsState>(settingsKeys.scripts(), {
        ...state,
        // Our rules replace any raw script once saved.
        rawContent: null,
        oursFilters: {
          ...filters,
          vacation: state.vacationScriptActive || filters.vacation,
        },
      });
    },
    onSuccess: async (_data, { state }) => {
      // A new script (or one that wasn't active) changes ids and states.
      if (state.ours === null || state.active?.id !== state.ours.id) {
        await queryClient.invalidateQueries({ queryKey: settingsKeys.all });
      }
    },
  });
};

export const useSaveRawScript = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      activate,
      content,
      id,
    }: {
      readonly id: SieveScriptId;
      readonly content: string;
      readonly activate: boolean;
    }) => {
      await saveRawScript(id, content, activate);
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: settingsKeys.all });
    },
  });
};

export const useSaveVacation = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async ({
      enabled,
      settings,
    }: {
      readonly settings: VacationSettings;
      readonly enabled: boolean;
    }) => await saveVacation(settings, enabled),
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: settingsKeys.all });
    },
  });
};

/** The text of one script (its blob changes with every save). */
export const useScriptText = (script: ScriptInfo | null) =>
  useQuery({
    enabled: script !== null,
    // After a save the blob changes: keep showing the same script meanwhile
    // (not another one).
    placeholderData: (previous, previousQuery) =>
      previousQuery?.queryKey[2] === script?.id ? previous : undefined,
    queryFn: async () => (script === null ? "" : await readScriptText(script)),
    queryKey:
      script === null
        ? settingsKeys.all
        : settingsKeys.scriptText(script.id, script.blobId),
    staleTime: Infinity,
  });

/** Creating, renaming, deleting and (de)activating scripts. */
export type ScriptChange =
  | { readonly kind: "create"; readonly name: string; readonly content: string }
  | {
      readonly kind: "rename";
      readonly id: SieveScriptId;
      readonly name: string;
    }
  | { readonly kind: "delete"; readonly script: ScriptInfo }
  | { readonly kind: "activate"; readonly id: SieveScriptId | null };

export const useChangeScript = () => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (change: ScriptChange): Promise<void> => {
      switch (change.kind) {
        case "create": {
          await saveScript({
            activate: false,
            content: change.content,
            id: null,
            name: change.name,
          });
          return;
        }
        case "rename": {
          await renameScript(change.id, change.name);
          return;
        }
        case "delete": {
          await deleteScript(change.script);
          return;
        }
        case "activate": {
          await activateScript(change.id);
          return;
        }
        default: {
          change satisfies never;
        }
      }
    },
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: settingsKeys.all });
    },
  });
};
