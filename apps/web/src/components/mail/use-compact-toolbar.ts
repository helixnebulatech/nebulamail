// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useSyncExternalStore } from "react";

/**
 * When the reader's toolbar hides its `wide` buttons (reader.tsx: phones and
 * touch tablets, as `bp.mobile` and `bp.narrowTouch`). Keep in sync.
 */
const COMPACT = "(max-width: 640px), (max-width: 1000px) and (pointer: coarse)";

const subscribe = (onChange: () => void): (() => void) => {
  const query = globalThis.matchMedia(COMPACT);
  query.addEventListener("change", onChange);
  return () => {
    query.removeEventListener("change", onChange);
  };
};

const compactNow = (): boolean => globalThis.matchMedia(COMPACT).matches;

/**
 * Whether the toolbar is compact, so the "More" menu carries what it hid
 * (and only that: the same action twice on screen is noise). The server
 * renders the wide toolbar.
 */
export const useCompactToolbar = (): boolean =>
  useSyncExternalStore(subscribe, compactNow, () => false);
