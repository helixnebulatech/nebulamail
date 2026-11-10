// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The app side of the synced preferences (sync.ts): what this device has
// (the display cookie, the language, the "Verified sender" flag), how to
// apply a change made on another device, and when to sync: on opening the
// app and on coming back to the tab (at most once a minute).

import { useRouter } from "@tanstack/react-router";
import { useEffect, useSyncExternalStore } from "react";

import {
  PREFERENCES_COOKIE,
  cookieValue,
  parsePreferences,
  savePreferences,
} from "../lib/preferences";
import {
  readLocalChoice,
  readLocalFlag,
  setLocalChoice,
  setLocalFlag,
} from "../lib/use-local-flag";
import {
  COMPOSE_FONT_KEY,
  COMPOSE_FONT_SIZE_KEY,
  COMPOSE_FORMAT_KEY,
  readComposePrefs,
} from "../mail/compose-prefs";
import { ONE_CLICK_KEY } from "../mail/one-click-unsubscribe";
import {
  DEFAULT_READ_RECEIPTS,
  READ_RECEIPTS_KEY,
  READ_RECEIPT_MODES,
} from "../mail/read-receipts";
import {
  DEFAULT_REMOTE_IMAGES,
  REMOTE_IMAGES_KEY,
  REMOTE_IMAGE_MODES,
} from "../mail/remote-images";
import { NEW_SENDER_HINT_KEY } from "../mail/use-new-sender-hint";
import { VERIFIED_BADGE_KEY } from "../mail/use-verified-badge";
import { getLocale, setLocale } from "../paraglide/runtime.js";
import type { SyncValues } from "./model";
import {
  setSyncDevice,
  subscribeSyncStatus,
  syncPreferences,
  syncStatus,
} from "./sync";
import type { SyncStatus } from "./sync";

const MIN_INTERVAL_MS = 60_000;

const readCookie = () =>
  parsePreferences(cookieValue(document.cookie, PREFERENCES_COOKIE));

const readDevice = (): SyncValues => {
  const display = readCookie();
  const language = getLocale();
  const compose = readComposePrefs();
  return {
    composeFont: compose.style.font,
    composeFontSize: compose.style.size,
    composeFormat: compose.format,
    density: display.density,
    language: language === "es" ? "es" : "en",
    newSenderHint: readLocalFlag(NEW_SENDER_HINT_KEY, true),
    oneClickUnsubscribe: readLocalFlag(ONE_CLICK_KEY, false),
    pane: display.pane,
    readReceipts: readLocalChoice(
      READ_RECEIPTS_KEY,
      READ_RECEIPT_MODES,
      DEFAULT_READ_RECEIPTS
    ),
    remoteImages: readLocalChoice(
      REMOTE_IMAGES_KEY,
      REMOTE_IMAGE_MODES,
      DEFAULT_REMOTE_IMAGES
    ),
    theme: display.theme,
    verifiedBadge: readLocalFlag(VERIFIED_BADGE_KEY, false),
  };
};

/** Options kept in localStorage: sync key and storage key. */
const LOCAL_FLAGS = [
  ["newSenderHint", NEW_SENDER_HINT_KEY],
  ["oneClickUnsubscribe", ONE_CLICK_KEY],
  ["verifiedBadge", VERIFIED_BADGE_KEY],
] as const;
const LOCAL_CHOICES = [
  ["composeFont", COMPOSE_FONT_KEY],
  ["composeFontSize", COMPOSE_FONT_SIZE_KEY],
  ["composeFormat", COMPOSE_FORMAT_KEY],
  ["readReceipts", READ_RECEIPTS_KEY],
  ["remoteImages", REMOTE_IMAGES_KEY],
] as const;

/** A change from another device to the options kept in localStorage. */
const applyLocal = (values: SyncValues, keys: readonly string[]): void => {
  for (const [key, storageKey] of LOCAL_FLAGS) {
    const value = values[key];
    if (keys.includes(key) && value !== undefined) {
      setLocalFlag(storageKey, value);
    }
  }
  for (const [key, storageKey] of LOCAL_CHOICES) {
    const value = values[key];
    if (keys.includes(key) && value !== undefined) {
      setLocalChoice(storageKey, value);
    }
  }
};

export const usePreferencesSync = (): void => {
  const router = useRouter();
  useEffect(() => {
    setSyncDevice({
      apply: (values, keys) => {
        const display = readCookie();
        const next = {
          density: keys.includes("density")
            ? (values.density ?? display.density)
            : display.density,
          pane: keys.includes("pane")
            ? (values.pane ?? display.pane)
            : display.pane,
          theme: keys.includes("theme")
            ? (values.theme ?? display.theme)
            : display.theme,
        };
        if (
          next.density !== display.density ||
          next.pane !== display.pane ||
          next.theme !== display.theme
        ) {
          savePreferences(next);
          void router.invalidate();
        }
        applyLocal(values, keys);
        // Last: changing the language reloads the page.
        if (
          keys.includes("language") &&
          values.language !== undefined &&
          values.language !== getLocale()
        ) {
          void setLocale(values.language);
        }
      },
      read: readDevice,
    });
    let last = 0;
    const run = (): void => {
      if (Date.now() - last < MIN_INTERVAL_MS) {
        return;
      }
      last = Date.now();
      void syncPreferences();
    };
    run();
    const onVisible = (): void => {
      if (document.visibilityState === "visible") {
        run();
      }
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      document.removeEventListener("visibilitychange", onVisible);
      setSyncDevice(null);
    };
  }, [router]);
};

const IDLE: SyncStatus = { state: "idle" };

/** Where the preferences are kept, for the settings page. */
export const useSyncStatus = (): SyncStatus =>
  useSyncExternalStore(subscribeSyncStatus, syncStatus, () => IDLE);
