// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "Now", refreshed every minute, so relative times ("9:41", "Yesterday")
// stay right without reading the clock during render.

import { useSyncExternalStore } from "react";

const MINUTE = 60_000;
let now = Date.now();
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | undefined;

const subscribe = (notify: () => void): (() => void) => {
  listeners.add(notify);
  if (timer === undefined) {
    now = Date.now();
    timer = setInterval(() => {
      now = Date.now();
      for (const listener of listeners) {
        listener();
      }
    }, MINUTE);
  }
  return () => {
    listeners.delete(notify);
    if (listeners.size === 0) {
      clearInterval(timer);
      timer = undefined;
    }
  };
};

/** Milliseconds since the epoch, updated each minute (0 on the server). */
export const useNow = (): number =>
  useSyncExternalStore(
    subscribe,
    () => now,
    () => 0
  );
