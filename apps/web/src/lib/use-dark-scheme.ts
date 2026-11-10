// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useRouteContext } from "@tanstack/react-router";
import { useSyncExternalStore } from "react";

const DARK = "(prefers-color-scheme: dark)";

const subscribe = (onChange: () => void): (() => void) => {
  const query = globalThis.matchMedia(DARK);
  query.addEventListener("change", onChange);
  return () => {
    query.removeEventListener("change", onChange);
  };
};

const systemDark = (): boolean => globalThis.matchMedia(DARK).matches;

/**
 * Whether the app is showing its dark theme: the chosen one, or the
 * system's when it follows the system. For what CSS can't reach, like the
 * colours inside an email's frame. The server renders light.
 */
export const useDarkScheme = (): boolean => {
  const { preferences } = useRouteContext({ from: "__root__" });
  const system = useSyncExternalStore(subscribe, systemDark, () => false);
  return preferences.theme === "system" ? system : preferences.theme === "dark";
};
