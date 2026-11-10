// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The IMAP server's `[ALERT]`s, which "MUST be presented to the user"
// (RFC 3501 §7.1, RFC 9051 §7.1). The bridge puts them in the session
//each one is shown once as a
// notice that stays until closed. The ids of the closed ones are kept on
// this device (opaque ids, nothing sensitive.1).

import type { ServerAlert } from "@nebulamail/jmap-client";

const KEY = "wm-closed-server-alerts";
/** Ids remembered; the bridge keeps at most 5 alerts, for a day. */
const MAX_REMEMBERED = 50;

const readClosed = (): string[] => {
  try {
    const parsed: unknown = JSON.parse(
      globalThis.localStorage.getItem(KEY) ?? "[]"
    );
    return Array.isArray(parsed)
      ? parsed.filter((id): id is string => typeof id === "string")
      : [];
  } catch {
    // Storage disabled or damaged: show them again.
    return [];
  }
};

/** Remembers that the user closed the alert `id`. */
export const closeServerAlert = (id: string): void => {
  const closed = [...readClosed().filter((item) => item !== id), id].slice(
    -MAX_REMEMBERED
  );
  try {
    globalThis.localStorage.setItem(KEY, JSON.stringify(closed));
  } catch {
    // Not remembered: it shows again after a reload.
  }
};

/** The alerts to show now: not closed, and not shown yet (`shown`). */
export const alertsToShow = (
  alerts: readonly ServerAlert[],
  shown: ReadonlySet<string>
): ServerAlert[] => {
  const closed = new Set(readClosed());
  return alerts.filter(
    (alert) => !closed.has(alert.id) && !shown.has(alert.id)
  );
};
