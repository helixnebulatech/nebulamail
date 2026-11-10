// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Vacation response (RFC 8621 §8) domain functions. In Stalwart it is a
// Sieve script; when our filters are the active script they include it,
// so turning it on never switches the filters off (scripts.ts).

import { getClient } from "../../mail/client";
import {
  recordChange,
  syncPreferences,
  syncedOnce,
  syncedValue,
} from "../../prefs-sync/sync";
import { OUR_SCRIPT_NAME, generateScript } from "../filters/generate";
import type { ScriptsState } from "../scripts";
import {
  activateScript,
  filterOptions,
  loadScripts,
  saveScript,
  vacationIncluded,
} from "../scripts";

export interface VacationSettings {
  /** `YYYY-MM-DD` in the user's time zone, or "" for no limit. */
  readonly fromDate: string;
  readonly toDate: string;
  readonly subject: string;
  readonly textBody: string;
  /** Already sanitized (sanitizeComposeHtml) by the editor. */
  readonly htmlBody: string;
}

export interface Vacation {
  readonly enabled: boolean;
  readonly settings: VacationSettings;
  /**
   * A script that turning the vacation response on would switch off: the
   * server runs one at a time and we can only include it in our rules.
   */
  readonly pausesScript: string | null;
  /**
   * The script turning it on paused, which turning it off switches back
   * on (remembered in the synced preferences, so any device does it).
   */
  readonly resumesScript: string | null;
  /** Raw server dates, for the banner. */
  readonly from: Date | null;
  readonly to: Date | null;
}

/** Stalwart refuses longer values (checked live). */
export const VACATION_LIMITS = {
  body: 2047,
  subject: 511,
} as const;

const pad = (value: number): string => String(value).padStart(2, "0");

/** A server UTCDate → the local calendar day (`YYYY-MM-DD`). */
export const toDay = (iso: string | null): string => {
  if (iso === null) {
    return "";
  }
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  return `${String(date.getFullYear())}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
};

/**
 * A local day → UTCDate: the start of the day for "from", the end of it
 * for "to" (both inclusive, as the user reads them).
 */
export const fromDay = (day: string, edge: "start" | "end"): string | null => {
  const match = /^(?<year>\d{4})-(?<month>\d{2})-(?<date>\d{2})$/u.exec(day);
  if (match === null) {
    return null;
  }
  const year = Number(match.groups?.year);
  const month = Number(match.groups?.month);
  const date = Number(match.groups?.date);
  const local =
    edge === "start"
      ? new Date(year, month - 1, date, 0, 0, 0)
      : new Date(year, month - 1, date, 23, 59, 59);
  // UTCDate: no milliseconds (RFC 8620 §1.4).
  return `${local.toISOString().slice(0, 19)}Z`;
};

const byteLength = (text: string): number =>
  new TextEncoder().encode(text).byteLength;

export type VacationProblem = "subjectTooLong" | "bodyTooLong" | "datesOrder";

export const vacationProblems = (
  settings: VacationSettings
): VacationProblem[] => {
  const problems: VacationProblem[] = [];
  if (byteLength(settings.subject) > VACATION_LIMITS.subject) {
    problems.push("subjectTooLong");
  }
  if (
    byteLength(settings.textBody) > VACATION_LIMITS.body ||
    byteLength(settings.htmlBody) > VACATION_LIMITS.body
  ) {
    problems.push("bodyTooLong");
  }
  if (
    settings.fromDate !== "" &&
    settings.toDate !== "" &&
    settings.toDate < settings.fromDate
  ) {
    problems.push("datesOrder");
  }
  return problems;
};

const toDate = (iso: string | null | undefined): Date | null =>
  iso === null || iso === undefined ? null : new Date(iso);

const nullable = (text: string): string | null =>
  text.trim() === "" ? null : text;

/** The script that turning the vacation response on would switch off. */
const pausedBy = (state: ScriptsState): string | null => {
  if (state.active === null || vacationIncluded(state)) {
    return null;
  }
  // Our own rules, still readable: we include the vacation response.
  if (state.active.id === state.ours?.id && state.oursFilters !== null) {
    return null;
  }
  return state.active.name;
};

/** The remembered paused script, if it still exists and nothing replaced it. */
const pausedScript = (
  state: ScriptsState
): { readonly id: string; readonly name: string } | null => {
  const paused = syncedValue("vacationPausedScript") ?? null;
  if (paused === null || state.active !== null) {
    return null;
  }
  const script = state.scripts.find((item) => item.id === paused.id);
  return script === undefined ? null : { id: script.id, name: script.name };
};

export const getVacation = async (): Promise<Vacation> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  const [result, state] = await Promise.all([
    client.request((b) =>
      b.call("VacationResponse/get", {
        accountId,
        properties: [
          "isEnabled",
          "fromDate",
          "toDate",
          "subject",
          "textBody",
          "htmlBody",
        ],
      })
    ),
    loadScripts(),
    // The paused script may have been noted on another device.
    syncedOnce(),
  ]);
  const [vacation] = result.list;
  const enabled = vacation?.isEnabled === true || vacationIncluded(state);
  return {
    enabled,
    from: toDate(vacation?.fromDate),
    pausesScript: pausedBy(state),
    resumesScript: enabled ? (pausedScript(state)?.name ?? null) : null,
    settings: {
      fromDate: toDay(vacation?.fromDate ?? null),
      htmlBody: vacation?.htmlBody ?? "",
      subject: vacation?.subject ?? "",
      textBody: vacation?.textBody ?? "",
      toDate: toDay(vacation?.toDate ?? null),
    },
    to: toDate(vacation?.toDate),
  };
};

/**
 * Saves the message and turns the vacation response on or off. Returns
 * the name of the script that was switched back on, if any.
 */
export const saveVacation = async (
  settings: VacationSettings,
  enabled: boolean
): Promise<string | null> => {
  const client = await getClient();
  const accountId = client.primaryAccountId();
  // The paused script may have been noted on another device.
  await syncPreferences();
  const state = await loadScripts();
  const pausing = enabled ? pausedBy(state) : null;
  // Our rules are active and readable: keep them active, include it.
  const viaFilters =
    state.active !== null &&
    state.active.id === state.ours?.id &&
    state.oursFilters !== null;
  const [set] = await client.request((b) => [
    b.call("VacationResponse/set", {
      accountId,
      update: {
        singleton: {
          fromDate: fromDay(settings.fromDate, "start"),
          htmlBody: nullable(settings.htmlBody),
          isEnabled: viaFilters ? false : enabled,
          subject: nullable(settings.subject),
          textBody: nullable(settings.textBody),
          toDate: fromDay(settings.toDate, "end"),
        },
      },
    }),
  ]);
  const error = set.notUpdated.singleton;
  if (error !== undefined) {
    throw new Error(error.description ?? error.type);
  }
  // Turning it on switched another app's script off: note it, so turning
  // it off can switch it back on.
  if (pausing !== null && state.active !== null) {
    recordChange("vacationPausedScript", {
      id: state.active.id,
      name: state.active.name,
    });
  }
  if (!enabled && !viaFilters) {
    const paused = pausedScript(state);
    if (paused !== null) {
      const script = state.scripts.find((item) => item.id === paused.id);
      if (script !== undefined) {
        await activateScript(script.id);
      }
      recordChange("vacationPausedScript", null);
      return paused.name;
    }
  }
  if (viaFilters && state.ours !== null && state.oursFilters !== null) {
    await saveScript({
      activate: true,
      content: generateScript(
        { ...state.oursFilters, vacation: enabled },
        await filterOptions()
      ),
      id: state.ours.id,
      name: OUR_SCRIPT_NAME,
    });
  }
  return null;
};
