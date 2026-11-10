// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A remembered on/off UI choice on this device (e.g. the collapsed
// sidebar). Only for non-sensitive UI state (nothing
// sensitive in localStorage). The server renders the default.

import { useSyncExternalStore } from "react";

const EVENT = "wm-local-flag";

/** Sets a flag outside React; mounted `useLocalFlag`s re-render. */
export const setLocalFlag = (key: string, value: boolean): void => {
  try {
    globalThis.localStorage.setItem(key, value ? "1" : "0");
  } catch {
    // Not remembered; the change still applies until reload.
  }
  globalThis.dispatchEvent(new Event(EVENT));
};

/** Reads a flag outside React (`fallback` if unset or unreadable). */
export const readLocalFlag = (key: string, fallback: boolean): boolean => {
  try {
    const value = globalThis.localStorage.getItem(key);
    return value === null ? fallback : value === "1";
  } catch {
    // Storage disabled (privacy mode): keep the default.
    return fallback;
  }
};

const subscribe = (notify: () => void): (() => void) => {
  globalThis.addEventListener(EVENT, notify);
  globalThis.addEventListener("storage", notify);
  return () => {
    globalThis.removeEventListener(EVENT, notify);
    globalThis.removeEventListener("storage", notify);
  };
};

export const useLocalFlag = (
  key: string,
  fallback: boolean
): readonly [boolean, (value: boolean) => void] => {
  const value = useSyncExternalStore(
    subscribe,
    () => readLocalFlag(key, fallback),
    () => fallback
  );
  const set = (next: boolean): void => {
    setLocalFlag(key, next);
  };
  return [value, set] as const;
};

/** Sets a remembered choice (one of a few words) outside React. */
export const setLocalChoice = (key: string, value: string): void => {
  try {
    globalThis.localStorage.setItem(key, value);
  } catch {
    // Not remembered; the change still applies until reload.
  }
  globalThis.dispatchEvent(new Event(EVENT));
};

/** Reads a remembered choice; anything not in `options` is `fallback`. */
export const readLocalChoice = <T extends string>(
  key: string,
  options: readonly T[],
  fallback: T
): T => {
  try {
    const value = globalThis.localStorage.getItem(key);
    return options.find((option) => option === value) ?? fallback;
  } catch {
    // Storage disabled (privacy mode): keep the default.
    return fallback;
  }
};

/** A remembered choice among `options`, like useLocalFlag. */
export const useLocalChoice = <T extends string>(
  key: string,
  options: readonly T[],
  fallback: T
): readonly [T, (value: T) => void] => {
  const value = useSyncExternalStore(
    subscribe,
    () => readLocalChoice(key, options, fallback),
    () => fallback
  );
  const set = (next: T): void => {
    setLocalChoice(key, next);
  };
  return [value, set] as const;
};
