// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One-click unsubscribe in the static build: off by default,
// each user turns it on (on the device, synced). When on, the
// emails that qualify (mail/unsubscribe.ts: https, List-Unsubscribe-Post
// and an aligned DKIM pass written by our own server) open the zip's
// unsubscribe.html, which asks once more and sends the POST from the
// browser (static/unsubscribe-page.ts). The build with our server always
// has one-click, sent by the BFF.

import { useLocalFlag } from "../lib/use-local-flag";
import { recordChange } from "../prefs-sync/sync";
import { isPublicHttpsUrl } from "./public-url";

export const ONE_CLICK_KEY = "wm-one-click-unsubscribe";

/** The zip's helper page (hashed name); null in the BFF build (none). */
export const UNSUBSCRIBE_PAGE: string | null =
  import.meta.env.NEBULA_UNSUBSCRIBE_PAGE === null ||
  import.meta.env.NEBULA_UNSUBSCRIBE_PAGE === ""
    ? null
    : `${import.meta.env.BASE_URL}${import.meta.env.NEBULA_UNSUBSCRIBE_PAGE}`;

export const useOneClickUnsubscribe = (): readonly [
  boolean,
  (enabled: boolean) => void,
] => {
  const [enabled, setEnabled] = useLocalFlag(ONE_CLICK_KEY, false);
  return [
    enabled,
    (next) => {
      setEnabled(next);
      recordChange("oneClickUnsubscribe", next);
    },
  ] as const;
};

/** The helper page for a one-click URL, or null if it can't go there. */
export const unsubscribePageFor = (oneClick: string): string | null =>
  UNSUBSCRIBE_PAGE !== null && isPublicHttpsUrl(oneClick)
    ? `${UNSUBSCRIBE_PAGE}#${encodeURIComponent(oneClick)}`
    : null;
