// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Remote images in the static build, each user's choice,
// on the device and synced:
//   - "blocked" (A, the default): never loaded;
//   - "direct" (B): blocked, with a button that loads them straight from
//     the sender after a warning (the sender sees the IP and when the
//     email is opened). Trackers are still removed first.
//   - "proxy" (D, only if the zip was built with an image proxy): blocked,
//     with a button that shows the copies the proxy downloaded when the
//     email arrived (mail/preload-token.ts). Emails it didn't preload stay
//     blocked.
// The build with our server always uses its image proxy instead.

import { useLocalChoice } from "../lib/use-local-flag";
import { recordChange } from "../prefs-sync/sync";
import { PRELOAD_PROXY_ORIGIN } from "./preload-token";

export const REMOTE_IMAGE_MODES = ["blocked", "direct", "proxy"] as const;
export type RemoteImageMode = (typeof REMOTE_IMAGE_MODES)[number];

/**
 * The page an email is shown in when its images come straight from the
 * sender (lib/frame-page.ts): a file with a hashed name in the static zip
 * (static/plugin.ts), a route with our server (routes/email-frame[.]html.ts).
 * Its own CSP allows `https:` images, which the app's CSP (inherited by
 * `srcdoc` iframes) doesn't. Null if a build had no such page: no "direct"
 * mode then.
 */
export const DIRECT_FRAME_PAGE: string | null =
  import.meta.env.NEBULA_FRAME_PAGE === null ||
  import.meta.env.NEBULA_FRAME_PAGE === ""
    ? null
    : `${import.meta.env.BASE_URL}${import.meta.env.NEBULA_FRAME_PAGE}`;

/**
 * The modes this build offers ("proxy" needs an image proxy, "direct" the
 * frame page).
 */
export const AVAILABLE_REMOTE_IMAGE_MODES: readonly RemoteImageMode[] = (
  PRELOAD_PROXY_ORIGIN === ""
    ? (["blocked", "direct"] as const)
    : (["blocked", "proxy", "direct"] as const)
).filter((mode) => mode !== "direct" || DIRECT_FRAME_PAGE !== null);
export const DEFAULT_REMOTE_IMAGES: RemoteImageMode = "blocked";
export const REMOTE_IMAGES_KEY = "wm-remote-images";

export const useRemoteImageMode = (): readonly [
  RemoteImageMode,
  (mode: RemoteImageMode) => void,
] => {
  const [stored, setMode] = useLocalChoice(
    REMOTE_IMAGES_KEY,
    REMOTE_IMAGE_MODES,
    DEFAULT_REMOTE_IMAGES
  );
  // "proxy" synced from a build with a proxy, here without one: blocked.
  const mode = AVAILABLE_REMOTE_IMAGE_MODES.includes(stored)
    ? stored
    : DEFAULT_REMOTE_IMAGES;
  return [
    mode,
    (next) => {
      setMode(next);
      recordChange("remoteImages", next);
    },
  ] as const;
};
