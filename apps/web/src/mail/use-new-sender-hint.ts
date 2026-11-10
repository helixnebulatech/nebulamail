// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The «Primera vez que te escribe» setting (sender-new.ts): on by default,
// each user can turn it off (on the device, synced). Off,
// the reader shows no hint and makes no first-contact search at all.

import { useLocalFlag } from "../lib/use-local-flag";
import { recordChange } from "../prefs-sync/sync";

export const NEW_SENDER_HINT_KEY = "wm-new-sender-hint";

export const useNewSenderHint = (): readonly [
  boolean,
  (enabled: boolean) => void,
] => {
  const [enabled, setEnabled] = useLocalFlag(NEW_SENDER_HINT_KEY, true);
  return [
    enabled,
    (next) => {
      setEnabled(next);
      recordChange("newSenderHint", next);
    },
  ] as const;
};
