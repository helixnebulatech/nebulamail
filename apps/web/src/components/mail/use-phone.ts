// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { useSyncExternalStore } from "react";

/** `bp.mobile` (breakpoints.stylex.ts), for what CSS can't decide. Keep in sync. */
const PHONE = "(max-width: 640px)";

const subscribe = (onChange: () => void): (() => void) => {
  const query = globalThis.matchMedia(PHONE);
  query.addEventListener("change", onChange);
  return () => {
    query.removeEventListener("change", onChange);
  };
};

const phoneNow = (): boolean => globalThis.matchMedia(PHONE).matches;

/** Phone layout (one column). The server renders the desktop one. */
export const usePhone = (): boolean =>
  useSyncExternalStore(subscribe, phoneNow, () => false);
