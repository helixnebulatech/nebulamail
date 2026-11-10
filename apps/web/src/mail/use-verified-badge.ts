// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The "Verified sender" badge setting: the admin can
// force it on or off (VERIFIED_SENDER_BADGE); otherwise each user decides
// on this device, off by default. Warnings on a failed check are always
// shown; only the green "pass" depends on this.

import { useQuery } from "@tanstack/react-query";

import { getGrant } from "../lib/auth";
import { useLocalFlag } from "../lib/use-local-flag";
import { recordChange } from "../prefs-sync/sync";

export const VERIFIED_BADGE_KEY = "wm-verified-sender-badge";

export interface VerifiedBadgeSetting {
  /** Whether the green badge is shown. */
  readonly enabled: boolean;
  /** The admin fixed it; the user can't change it. */
  readonly forced: boolean;
  readonly setEnabled: (enabled: boolean) => void;
}

export const useVerifiedBadge = (): VerifiedBadgeSetting => {
  const [chosen, setChosen] = useLocalFlag(VERIFIED_BADGE_KEY, false);
  const { data: forced } = useQuery({
    queryFn: async () => {
      const grant = await getGrant();
      return grant.verifiedSenderBadge;
    },
    queryKey: ["config", "verified-sender-badge"],
    staleTime: Number.POSITIVE_INFINITY,
  });
  const locked = forced !== null && forced !== undefined;
  return {
    enabled: locked ? forced : chosen,
    forced: locked,
    setEnabled: (enabled) => {
      setChosen(enabled);
      recordChange("verifiedBadge", enabled);
    },
  };
};
