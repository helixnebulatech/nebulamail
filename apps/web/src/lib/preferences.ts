// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Display preferences the server needs to render the first page without a
// flash: theme, density, where emails open, whether the
// sidebar is collapsed and whether the calendar shows tasks, in one small
// cookie. Nothing
// sensitive; readable by JS (not HttpOnly) because the app changes it.
// Everything else lives on the device (IndexedDB) or, later, synced.

import { createIsomorphicFn } from "@tanstack/react-start";
import { getRequest } from "@tanstack/react-start/server";
import { z } from "zod";

export const PREFERENCES_COOKIE = "wm-prefs";

const PreferencesSchema = z.object({
  /** Tasks in the calendar, on by default (calendar-tasks.tsx). */
  calendarTasks: z.enum(["shown", "hidden"]).catch("shown"),
  density: z.enum(["compact", "comfortable"]).catch("compact"),
  /**
   * Where an email opens: "side" next to the list (the reading pane), or
   * "full" in place of the list, like Proton (the list alone otherwise).
   */
  pane: z.enum(["side", "full"]).catch("side"),
  /** The sidebar, collapsed to its icons or not (shell.tsx). */
  sidebar: z.enum(["open", "collapsed"]).catch("open"),
  theme: z.enum(["light", "dark", "system"]).catch("system"),
});

export type Preferences = z.infer<typeof PreferencesSchema>;
export type ThemeMode = Preferences["theme"];
export type ReadingPane = Preferences["pane"];
/** What the settings change; the layout has its own setter below. */
export type DisplayPreferences = Pick<
  Preferences,
  "density" | "pane" | "theme"
>;

/** Layout state the screens change themselves (`saveLayout`). */
export type LayoutPreferences = Pick<Preferences, "calendarTasks" | "sidebar">;

export const DEFAULT_PREFERENCES: Preferences = {
  calendarTasks: "shown",
  density: "compact",
  pane: "side",
  sidebar: "open",
  theme: "system",
};

/**
 * `theme.density.pane.sidebar.calendarTasks`, e.g.
 * `dark.compact.full.collapsed.hidden`: no JSON, nothing to escape. Older
 * shorter cookies still parse (pane: "side", sidebar: "open",
 * calendarTasks: "shown").
 */
export const serializePreferences = (preferences: Preferences): string =>
  [
    preferences.theme,
    preferences.density,
    preferences.pane,
    preferences.sidebar,
    preferences.calendarTasks,
  ].join(".");

export const parsePreferences = (value: string | undefined): Preferences => {
  const [theme, density, pane, sidebar, calendarTasks] = (value ?? "").split(
    "."
  );
  return PreferencesSchema.parse({
    calendarTasks,
    density,
    pane,
    sidebar,
    theme,
  });
};

export const cookieValue = (
  header: string | null,
  name: string
): string | undefined =>
  header
    ?.split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${name}=`))
    ?.slice(name.length + 1);

/** Browser only: the preferences as saved now. */
export const readSavedPreferences = (): Preferences =>
  parsePreferences(cookieValue(document.cookie, PREFERENCES_COOKIE));

/**
 * The current preferences: from the request on the server, from
 * `document.cookie` in the browser (no server round trip per navigation).
 * Compiled away on each side, so no RPC: the static build
 * has no server, and its shell is prerendered with the defaults.
 */
export const readPreferences = createIsomorphicFn()
  .server(() =>
    parsePreferences(
      cookieValue(getRequest().headers.get("cookie"), PREFERENCES_COOKIE)
    )
  )
  .client(() => readSavedPreferences());

const writeCookie = (preferences: Preferences): void => {
  const secure = globalThis.location.protocol === "https:" ? "; Secure" : "";
  // oxlint-disable-next-line unicorn/no-document-cookie -- the Cookie Store API isn't in Firefox/Safari yet.
  document.cookie = `${PREFERENCES_COOKIE}=${serializePreferences(preferences)}; Path=/; Max-Age=31536000; SameSite=Lax${secure}`;
};

/**
 * Browser only: saves the display preferences for the next server render.
 * The layout keeps its saved state (the caller's copy may be older).
 */
export const savePreferences = (preferences: DisplayPreferences): void => {
  const saved = readSavedPreferences();
  writeCookie({
    calendarTasks: saved.calendarTasks,
    density: preferences.density,
    pane: preferences.pane,
    sidebar: saved.sidebar,
    theme: preferences.theme,
  });
};

/** Browser only: saves layout state, keeping the rest as saved. */
export const saveLayout = (layout: Partial<LayoutPreferences>): void => {
  writeCookie({ ...readSavedPreferences(), ...layout });
};
