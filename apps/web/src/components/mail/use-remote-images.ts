// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Remote images for one open email: where "Show
// images" loads from, and loading them at once or after a click.

import type { EmailId } from "@nebulamail/jmap-client";
import { DEFAULT_PRIVACY, validateProxyOrigin } from "@nebulamail/mail-render";
import { useToast } from "@nebulamail/ui";
import { useState } from "react";

import { useMailAccount } from "../../mail/account-scope";
import type { EmailDetail } from "../../mail/api";
import type { RemoteImages } from "../../mail/images";
import { loadRemoteImages } from "../../mail/images";
import { PRELOAD_PROXY_ORIGIN, preloadedUrls } from "../../mail/preload-token";
import type { RemoteImageMode } from "../../mail/remote-images";
import { useRemoteImageMode } from "../../mail/remote-images";
import { useFreeImages } from "../../mail/use-free-images";
import type { EmailImages } from "../../mail/use-image-policy";
import { useEmailImages } from "../../mail/use-image-policy";
import { m } from "../../paraglide/messages.js";
import type { ImagesAccess } from "./privacy-bar";

/** Where "Show images" loads from, or nowhere (no button). */
type ImagesFrom = "proxy" | "sender" | "nowhere";

/**
 * Static build: where "Show images" loads from. With "proxy", only
 * emails the proxy preloaded (they carry its token); the rest stay blocked.
 */
const staticImagesFrom = (
  mode: RemoteImageMode,
  token: string | null
): ImagesFrom => {
  if (mode === "direct") {
    return "sender";
  }
  return mode === "proxy" && token !== null ? "proxy" : "nowhere";
};

/** Build with our server: the route of the settings (image-policy.ts). */
const serverImagesFrom = (images: EmailImages): ImagesFrom => {
  const route = images.settings.policy?.route ?? null;
  if (images.decision === "never" || route === null) {
    return "nowhere";
  }
  return route === "direct" ? "sender" : "proxy";
};

/**
 * Remote images for one open email: through the proxy
 * or straight from the sender, as the settings say; at once ("always", a
 * trusted sender) or after "Show images". In the static build, its own
 * choice (mail/remote-images.ts).
 */
export const useRemoteImages = (email: EmailDetail) => {
  const emailId = email.id;
  const toast = useToast();
  const account = useMailAccount();
  const [staticMode] = useRemoteImageMode();
  const policy = useEmailImages(email);
  const staticBuild = import.meta.env.NEBULA_STATIC;
  const from: ImagesFrom = staticBuild
    ? staticImagesFrom(staticMode, email.preloadToken)
    : serverImagesFrom(policy);
  const auto = !staticBuild && policy.decision === "auto";
  const [loaded, setLoaded] = useState<RemoteImages | null>(null);
  const [loading, setLoading] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  /** The email whose images were loaded at once (only tried once). */
  const [autoTried, setAutoTried] = useState<EmailId | null>(null);
  // Turned to "never" (here or on another device) while open: blocked again.
  const remote = from === "nowhere" ? null : loaded;

  // Batch-mode images are blob: URLs: free them when the email closes.
  useFreeImages(loaded);

  /** `urls`: the email's remote images (from the render). */
  const show = async (urls: readonly string[]): Promise<void> => {
    // Never without a way the settings allow: no ticket, no request. Once
    // per email (a click while they load at once does nothing more).
    if (from === "nowhere" || loading || loaded !== null) {
      return;
    }
    if (from === "sender") {
      setLoaded({ kind: "direct" });
      return;
    }
    if (staticBuild && email.preloadToken !== null) {
      setLoaded({
        kind: "proxy",
        origin: PRELOAD_PROXY_ORIGIN,
        urls: await preloadedUrls(
          PRELOAD_PROXY_ORIGIN,
          email.preloadToken,
          urls
        ),
      });
      return;
    }
    setLoading(true);
    try {
      const images = await loadRemoteImages(emailId, account);
      if (images.kind === "unavailable") {
        setUnavailable(true);
      } else {
        if (images.kind === "proxy") {
          // Throws for an origin mail-render won't put in the iframe CSP;
          // checked here so the render can't fail.
          validateProxyOrigin(images.origin);
        }
        setLoaded(images);
      }
    } catch {
      toast.show({
        priority: "high",
        title: m.toast_images_failed(),
        tone: "danger",
      });
    }
    setLoading(false);
  };

  /** "Always" or a trusted sender: load once, without a click. */
  const showAtOnce = (urls: readonly string[]): void => {
    if (!auto || autoTried === emailId || urls.length === 0) {
      return;
    }
    setAutoTried(emailId);
    void show(urls);
  };

  /** "Always show from this sender" (trusted-senders), then load. */
  const trustSender = (urls: readonly string[]): void => {
    const [sender] = email.from;
    if (sender !== undefined) {
      policy.settings.setTrusted(sender.email, true);
    }
    // Trusted now, so "at once" would load them a second time.
    setAutoTried(emailId);
    void show(urls);
  };

  let access: ImagesAccess;
  if (from === "nowhere") {
    access = staticBuild ? { kind: "blocked" } : { kind: "never" };
  } else if (unavailable) {
    access = { kind: "unavailable" };
  } else {
    access = {
      auto,
      from,
      kind: "offered",
      loading,
      offerTrust:
        !staticBuild &&
        policy.settings.policy?.load === "trusted-senders" &&
        !policy.senderTrusted &&
        email.from.length === 1 &&
        email.senderVerdict !== "fail",
      senderTrusted: policy.senderTrusted,
    };
  }

  return {
    access,
    /** For mail-render: "never" restores nothing whatever it is given. */
    settings: staticBuild
      ? DEFAULT_PRIVACY
      : {
          ...DEFAULT_PRIVACY,
          remoteImages: policy.settings.policy?.load ?? "never",
        },
    remote,
    show,
    showAtOnce,
    trustSender,
  };
};
