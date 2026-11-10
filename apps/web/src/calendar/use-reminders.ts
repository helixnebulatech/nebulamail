// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The "Event reminders" setting, like new-mail
// notifications: off by default, per device. Turning it on asks the
// browser for permission and, when the server can push to browsers (VAPID)
// and the app has its service worker, subscribes this device so reminders
// arrive with every tab closed. In an open tab reminders always show
// inside the app; the setting adds system notifications.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";

import { useLocalFlag } from "../lib/use-local-flag";
import { setPushTypes, webPushAvailable } from "../lib/web-push";
import { notificationsSupported } from "../mail/notify";
import type { NotifyPermission } from "../mail/use-notifications";
import { ALERTS_KEY } from "./alerts";

const PUSH_TYPE = "CalendarAlert";

const noSubscribe = (): (() => void) => () => {
  // Permission changes have no portable event; the flag change re-renders.
};

const readPermission = (): NotifyPermission =>
  notificationsSupported() ? Notification.permission : "unsupported";

/** On in this device's settings and allowed by the browser. */
export const remindersWanted = (): boolean => {
  if (!notificationsSupported() || Notification.permission !== "granted") {
    return false;
  }
  try {
    return globalThis.localStorage.getItem(ALERTS_KEY) === "1";
  } catch {
    return false;
  }
};

export interface ReminderSetting {
  readonly enabled: boolean;
  readonly permission: NotifyPermission;
  /** Reminders also reach this device with every tab closed. */
  readonly closedTabs: boolean | undefined;
  /** Resolves to whether the setting is on afterwards. */
  readonly setEnabled: (enabled: boolean) => Promise<boolean>;
}

const PUSH_KEY = ["calendar", "web-push-available"] as const;

export const useReminders = (): ReminderSetting => {
  const client = useQueryClient();
  const [chosen, setChosen] = useLocalFlag(ALERTS_KEY, false);
  const permission = useSyncExternalStore(
    noSubscribe,
    readPermission,
    () => "default" as const
  );
  const push = useQuery({
    queryFn: webPushAvailable,
    queryKey: PUSH_KEY,
    staleTime: Infinity,
  });
  const enabled = chosen && permission === "granted";
  return {
    closedTabs: push.data === undefined ? undefined : enabled && push.data,
    enabled,
    permission,
    setEnabled: async (next) => {
      if (!next) {
        setChosen(false);
        await setPushTypes({ remove: [PUSH_TYPE] }).catch(() => false);
        return false;
      }
      if (!notificationsSupported()) {
        return false;
      }
      const result = await Notification.requestPermission();
      const granted = result === "granted";
      setChosen(granted);
      // Turned on before the check finished: ask now, or closed tabs
      // would never get reminders on this device.
      if (
        granted &&
        (push.data ?? (await webPushAvailable().catch(() => false)))
      ) {
        await setPushTypes({ add: [PUSH_TYPE] }).catch(() => false);
      }
      await client.invalidateQueries({ queryKey: PUSH_KEY });
      return granted;
    },
  };
};
