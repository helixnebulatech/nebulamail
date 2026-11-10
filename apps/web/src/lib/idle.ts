// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Work that can wait (code for later): after the page has
// loaded and when the browser has nothing more urgent, so it doesn't
// compete with the first screen (its code, its data, its first paint).

/** At the latest this long after the page loaded, even if never idle. */
const IDLE_TIMEOUT_MS = 5000;
/** Without requestIdleCallback (Safari): a fixed pause instead. */
const FALLBACK_DELAY_MS = 1500;

/** Runs `task` once the page has loaded and the browser is idle. Returns a cancel. */
export const whenIdle = (task: () => void): (() => void) => {
  let cancelled = false;
  let cancelStep: (() => void) | undefined;
  const schedule = (): void => {
    if (cancelled) {
      return;
    }
    if (typeof globalThis.requestIdleCallback === "function") {
      const handle = globalThis.requestIdleCallback(task, {
        timeout: IDLE_TIMEOUT_MS,
      });
      cancelStep = () => {
        globalThis.cancelIdleCallback(handle);
      };
      return;
    }
    const handle = globalThis.setTimeout(task, FALLBACK_DELAY_MS);
    cancelStep = () => {
      globalThis.clearTimeout(handle);
    };
  };
  if (document.readyState === "complete") {
    schedule();
  } else {
    globalThis.addEventListener("load", schedule, { once: true });
    cancelStep = () => {
      globalThis.removeEventListener("load", schedule);
    };
  }
  return () => {
    cancelled = true;
    cancelStep?.();
  };
};
