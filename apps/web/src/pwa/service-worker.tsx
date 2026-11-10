// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Registers /sw.js (src/pwa/sw.ts) in production builds only: the dev
// server has no worker, so hot reload is never served from a cache.
// When a new build's worker takes over an open tab, a toast offers to
// reload.

import { useToast } from "@nebulamail/ui";
import { useEffect, useEffectEvent } from "react";

import { m } from "../paraglide/messages.js";

/** Registers the worker; returns the cleanup of the listeners. */
const start = (
  workers: ServiceWorkerContainer,
  onUpdate: () => void
): (() => void) => {
  // The first install also takes control of the page: that is no update.
  const hadWorker = workers.controller !== null;
  const onControllerChange = (): void => {
    if (hadWorker) {
      onUpdate();
    }
  };
  workers.addEventListener("controllerchange", onControllerChange);
  const base = import.meta.env.BASE_URL;
  const registration = workers
    .register(`${base}sw.js`, { scope: base, updateViaCache: "none" })
    .catch(() => null);
  // A mail tab stays open for days: look for a new build when it comes
  // back to the front (the browser itself checks on page loads).
  const checkForUpdate = async (): Promise<void> => {
    const current = await registration;
    await current?.update().catch(() => null);
  };
  const onVisible = (): void => {
    if (document.visibilityState === "visible") {
      void checkForUpdate();
    }
  };
  document.addEventListener("visibilitychange", onVisible);
  return () => {
    workers.removeEventListener("controllerchange", onControllerChange);
    document.removeEventListener("visibilitychange", onVisible);
  };
};

export const ServiceWorker = () => {
  const toast = useToast();
  const announceUpdate = useEffectEvent(() => {
    toast.show({
      action: {
        label: m.app_update_reload(),
        onClick: () => {
          globalThis.location.reload();
        },
      },
      countdown: false,
      timeout: 0,
      title: m.app_update_ready(),
    });
  });

  useEffect(() => {
    const supported = import.meta.env.PROD && "serviceWorker" in navigator;
    const stop = supported
      ? start(navigator.serviceWorker, () => {
          announceUpdate();
        })
      : null;
    return () => {
      stop?.();
    };
  }, []);

  return null;
};
