// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Shows the mail server's alerts (mail/server-alerts.ts) as notices that
// stay until the user closes them, now and whenever the session changes.

import type { Session } from "@nebulamail/jmap-client";
import { primaryAccountId, serverAlerts } from "@nebulamail/jmap-client";
import { useToast } from "@nebulamail/ui";
import { useEffect, useEffectEvent } from "react";

import { getClient } from "../../mail/client";
import { alertsToShow, closeServerAlert } from "../../mail/server-alerts";
import { m } from "../../paraglide/messages.js";

export const useServerAlerts = (): void => {
  const toast = useToast();
  const show = useEffectEvent((session: Session, shown: Set<string>): void => {
    const accountId = primaryAccountId(session);
    if (accountId === undefined) {
      return;
    }
    for (const alert of alertsToShow(serverAlerts(session, accountId), shown)) {
      shown.add(alert.id);
      toast.show({
        closeLabel: m.close(),
        // Plain text: React escapes it.
        description: alert.text,
        onClose: () => {
          closeServerAlert(alert.id);
        },
        priority: "high",
        timeout: 0,
        title: m.server_alert_title(),
        tone: "info",
      });
    }
  });
  useEffect(() => {
    let stop: (() => void) | null = null;
    let cancelled = false;
    const shown = new Set<string>();
    const watch = async (): Promise<void> => {
      const client = await getClient();
      if (cancelled) {
        return;
      }
      show(client.session, shown);
      stop = client.onSessionChange((session) => {
        show(session, shown);
      });
    };
    // No client (signed out): nothing to show.
    watch().catch(() => null);
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);
};
