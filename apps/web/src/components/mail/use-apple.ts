// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useSyncExternalStore } from "react";

import { isApplePlatform, shortcutKeys } from "../../lib/shortcuts";

const subscribe = (): (() => void) => () => {
  // The platform never changes.
};

/**
 * ⌘ or Ctrl? The server can't know, so it renders "Ctrl" and the browser
 * switches after hydration (no mismatch).
 */
export const useApple = (): boolean =>
  useSyncExternalStore(subscribe, isApplePlatform, () => false);

/** Keys for `<Shortcut keys>` / `shortcut` props on this platform. */
export const useKeys = (): ((spec: string) => string[]) => {
  const apple = useApple();
  return (spec) => shortcutKeys(spec, apple);
};
