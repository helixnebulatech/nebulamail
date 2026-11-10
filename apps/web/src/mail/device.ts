// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the mail UI keeps on this device, and forgetting it on logout
// (a shared computer must not keep the user's data):
// recent recipients (localStorage) and draft copies (IndexedDB).

import { forgetWebPush } from "../lib/web-push";
import { forgetSyncedPreferences } from "../prefs-sync/sync";
import { forgetBackups } from "./draft-backup";
import { forgetPushStore } from "./push-store";
import { forgetRecent } from "./suggestions";

export const forgetDeviceData = async (): Promise<void> => {
  forgetRecent();
  forgetSyncedPreferences();
  await forgetBackups();
  // Reminders and new mail must not keep reaching this browser after
  // signing out; the worker forgets the mail state it kept.
  await forgetWebPush().catch(() => null);
  await forgetPushStore().catch(() => null);
};
