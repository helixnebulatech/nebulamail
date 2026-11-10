// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Layout state the screens change themselves (the collapsed sidebar, tasks
// in the calendar), remembered in the preferences cookie (preferences.ts)
// so the server renders it as it was: no first paint that changes after
// hydration. The static build has no server: its
// prerendered shell has the defaults and the cookie applies right after
// hydration, as before.

import { useRouteContext } from "@tanstack/react-router";
import { useEffect, useSyncExternalStore } from "react";

import type { LayoutPreferences } from "./preferences";
import { readSavedPreferences, saveLayout } from "./preferences";

const EVENT = "wm-layout";

const subscribe = (notify: () => void): (() => void) => {
  globalThis.addEventListener(EVENT, notify);
  return () => {
    globalThis.removeEventListener(EVENT, notify);
  };
};

const save = (layout: Partial<LayoutPreferences>): void => {
  saveLayout(layout);
  globalThis.dispatchEvent(new Event(EVENT));
};

/**
 * Moves a choice an older version kept in localStorage (`legacyKey`, "1"
 * or "0", lib/use-local-flag.ts) to the cookie, once.
 */
const migrate = (legacyKey: string, apply: (on: boolean) => void): void => {
  try {
    const old = globalThis.localStorage.getItem(legacyKey);
    if (old === null) {
      return;
    }
    globalThis.localStorage.removeItem(legacyKey);
    apply(old === "1");
  } catch {
    // Storage disabled (privacy mode): nothing to move.
  }
};

/** One layout flag: the cookie's value, as the server rendered it first. */
const useLayoutFlag = <K extends keyof LayoutPreferences>(
  key: K,
  on: LayoutPreferences[K],
  off: LayoutPreferences[K],
  legacyKey: string
): readonly [boolean, (value: boolean) => void] => {
  // What the server rendered (from the request's cookie), so hydration
  // matches; the browser reads the cookie itself afterwards.
  const { preferences } = useRouteContext({ from: "__root__" });
  const value = useSyncExternalStore(
    subscribe,
    () => readSavedPreferences()[key] === on,
    () => preferences[key] === on
  );
  const set = (next: boolean): void => {
    save({ [key]: next ? on : off });
  };
  useEffect(() => {
    migrate(legacyKey, (old) => {
      save({ [key]: old ? on : off });
    });
  }, [key, legacyKey, off, on]);
  return [value, set] as const;
};

/** Whether the sidebar is collapsed to its icons (shell.tsx). */
export const useSidebarCollapsed = (
  legacyKey: string
): readonly [boolean, (collapsed: boolean) => void] =>
  useLayoutFlag("sidebar", "collapsed", "open", legacyKey);

/** Whether the calendar shows tasks (calendar-tasks.tsx; on by default). */
export const useCalendarTasksShown = (
  legacyKey: string
): readonly [boolean, (shown: boolean) => void] =>
  useLayoutFlag("calendarTasks", "shown", "hidden", legacyKey);
