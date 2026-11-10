// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The locale dates are written in, and the first day of the week. The app
// language is "es" or "en"; when the browser asks for a regional variant of
// it ("en-GB", "es-MX") that one is used for dates (week start, 24 h clock).
// The week start can be chosen; it is remembered on this device only.

import type { DayOfWeek } from "@internationalized/date";
import { useSyncExternalStore } from "react";

import { getLocale } from "../../paraglide/runtime.js";

export const WEEK_STARTS = ["auto", "mon", "sun", "sat"] as const;
export type WeekStartChoice = (typeof WEEK_STARTS)[number];

const KEY = "nebula.calendar.weekStart";
const EVENT = "wm-calendar-prefs";

const subscribe = (notify: () => void): (() => void) => {
  globalThis.addEventListener(EVENT, notify);
  globalThis.addEventListener("storage", notify);
  return () => {
    globalThis.removeEventListener(EVENT, notify);
    globalThis.removeEventListener("storage", notify);
  };
};

const readChoice = (): WeekStartChoice => {
  try {
    const value = globalThis.localStorage.getItem(KEY);
    return WEEK_STARTS.find((choice) => choice === value) ?? "auto";
  } catch {
    return "auto";
  }
};

const browserLocale = (language: string): string => {
  const wanted = globalThis.navigator.languages.find(
    (tag) => tag.toLowerCase().split("-")[0] === language
  );
  try {
    return wanted === undefined
      ? language
      : (Intl.getCanonicalLocales(wanted)[0] ?? language);
  } catch {
    return language;
  }
};

/** Locale for formatting dates (the app language, regional if possible). */
export const useDateLocale = (): string => {
  const language = getLocale();
  return useSyncExternalStore(
    subscribe,
    () => browserLocale(language),
    () => language
  );
};

export const useWeekStart = (): readonly [
  WeekStartChoice,
  DayOfWeek | undefined,
  (choice: WeekStartChoice) => void,
] => {
  const choice = useSyncExternalStore(
    subscribe,
    readChoice,
    (): WeekStartChoice => "auto"
  );
  const set = (next: WeekStartChoice): void => {
    try {
      globalThis.localStorage.setItem(KEY, next);
    } catch {
      // Not remembered; applies until reload.
    }
    globalThis.dispatchEvent(new Event(EVENT));
  };
  return [choice, choice === "auto" ? undefined : choice, set] as const;
};
