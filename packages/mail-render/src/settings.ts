// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Privacy options are configurable; the security parts
// (sanitizer, sandbox, CSP, form removal, link scheme checks, relative URL
// blocking) are not, so they have no settings here.

export type RemoteImages = "never" | "ask" | "trusted-senders" | "always";
export type LinkConfirmation = "always" | "suspicious" | "never";
export type DefaultView = "html" | "text";

export interface PrivacySettings {
  readonly remoteImages: RemoteImages;
  readonly loadThroughProxy: boolean;
  readonly removeTrackers: boolean;
  readonly mailTrackerBlocker: boolean;
  readonly spyPixelRules: boolean;
  readonly cleanLinks: boolean;
  readonly linkConfirmation: LinkConfirmation;
  readonly privacyReport: boolean;
  readonly defaultView: DefaultView;
  /** Admin/user list of hosts treated as trackers (a host matches itself and its subdomains). */
  readonly blockedTrackerHosts?: readonly string[];
  /** Hosts never reported by the MailTrackerBlocker list (spy-pixel rules still apply). */
  readonly allowedTrackerHosts?: readonly string[];
}

export const DEFAULT_PRIVACY: PrivacySettings = {
  cleanLinks: true,
  defaultView: "html",
  linkConfirmation: "always",
  loadThroughProxy: true,
  mailTrackerBlocker: true,
  privacyReport: true,
  remoteImages: "ask",
  removeTrackers: true,
  spyPixelRules: true,
};

export interface RemoteImageContext {
  /** The user pressed "Show images" for this email. */
  readonly userApproved?: boolean;
  /** The sender is in the user's trusted senders list. */
  readonly senderTrusted?: boolean;
}

/**
 * Whether remote images may be restored (always through the image source
 * the caller provides). "ask" and "trusted-senders" behave the same here:
 * the difference (whether to offer the "Show images" button) is UI.
 */
export const shouldLoadRemoteImages = (
  settings: Pick<PrivacySettings, "remoteImages">,
  context: RemoteImageContext
): boolean => {
  switch (settings.remoteImages) {
    case "never": {
      return false;
    }
    case "always": {
      return true;
    }
    case "ask":
    case "trusted-senders": {
      return context.userApproved === true || context.senderTrusted === true;
    }
    default: {
      return false;
    }
  }
};
