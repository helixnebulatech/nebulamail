// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The "New mail notifications" setting: off by default, per
// device. Turning it on asks the browser for permission; it only counts as
// on when the browser allows it. When the server can push to browsers
// (VAPID) and the app has its service worker, it also subscribes this
// device, so new mail is announced with every tab closed (mail-push.ts).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";

import { useLocalFlag } from "../lib/use-local-flag";
import { webPushAvailable } from "../lib/web-push";
import { turnOffMailPush, turnOnMailPush } from "./mail-push";
import { NOTIFY_KEY, notificationsSupported } from "./notify";

export type NotifyPermission = NotificationPermission | "unsupported";

const noSubscribe = (): (() => void) => () => {
  // Permission changes have no portable event; the flag change re-renders.
};

const readPermission = (): NotifyPermission =>
  notificationsSupported() ? Notification.permission : "unsupported";

export interface NotificationSetting {
  readonly enabled: boolean;
  readonly permission: NotifyPermission;
  /** New mail also reaches this device with every tab closed. */
  readonly closedTabs: boolean | undefined;
  /** Resolves to whether the setting is on afterwards. */
  readonly setEnabled: (enabled: boolean) => Promise<boolean>;
}

/** Same query as the reminders' (calendar/use-reminders.ts). */
const PUSH_KEY = ["calendar", "web-push-available"] as const;

export const useNewMailNotifications = (): NotificationSetting => {
  const client = useQueryClient();
  const [chosen, setChosen] = useLocalFlag(NOTIFY_KEY, false);
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
        await turnOffMailPush().catch(() => null);
        return false;
      }
      if (!notificationsSupported()) {
        return false;
      }
      const result = await Notification.requestPermission();
      const granted = result === "granted";
      setChosen(granted);
      if (granted) {
        await turnOnMailPush().catch(() => false);
      }
      await client.invalidateQueries({ queryKey: PUSH_KEY });
      return granted;
    },
  };
};
