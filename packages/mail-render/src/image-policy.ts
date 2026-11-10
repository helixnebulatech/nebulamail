// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What happens to one image reference. DOM-free
// and shared by the browser pipeline and the BFF extractor, so both decide
// the same way about trackers and blocked URLs.

import type { PrivacySettings } from "./settings";
import { isTrackerUrl } from "./trackers/match";
import type { TrackerMatch, TrackerMatchOptions } from "./trackers/match";
import { spyPixelReason } from "./trackers/spy-pixel";
import type { SpyPixelInput } from "./trackers/spy-pixel";
import { classifyImageUrl } from "./urls";
import type { BlockedImageReason } from "./urls";

export type ImageDecision =
  | { readonly kind: "inline"; readonly url: string }
  | { readonly kind: "cid"; readonly cid: string }
  | { readonly kind: "tracker"; readonly service: string; readonly url: string }
  | { readonly kind: "spy-pixel"; readonly url: string }
  | { readonly kind: "remote"; readonly url: string }
  | { readonly kind: "blocked"; readonly reason: BlockedImageReason };

export type TrackerSettings = Pick<
  PrivacySettings,
  | "removeTrackers"
  | "mailTrackerBlocker"
  | "spyPixelRules"
  | "blockedTrackerHosts"
  | "allowedTrackerHosts"
>;

const trackerOptions = (settings: TrackerSettings): TrackerMatchOptions => ({
  allowedHosts: settings.allowedTrackerHosts ?? [],
  blockedHosts: settings.blockedTrackerHosts ?? [],
  mailTrackerBlocker: settings.mailTrackerBlocker,
});

const decide = (
  raw: string,
  settings: TrackerSettings,
  dimensions: SpyPixelInput | undefined,
  trackerOf: (url: string) => TrackerMatch | null
): ImageDecision => {
  const classified = classifyImageUrl(raw);
  if (classified.kind !== "remote") {
    return classified;
  }
  const { url } = classified;
  if (settings.removeTrackers) {
    const tracker = trackerOf(url);
    if (tracker !== null) {
      return { kind: "tracker", service: tracker.service, url };
    }
    if (
      settings.spyPixelRules &&
      dimensions !== undefined &&
      spyPixelReason({ ...dimensions, url }) !== null
    ) {
      return { kind: "spy-pixel", url };
    }
  }
  return { kind: "remote", url };
};

/**
 * @param dimensions element size information when the reference belongs to
 *   an element (`<img>`, inline `style`, `background`); absent for `<style>`
 *   rules, where only the tracker list applies.
 */
export const decideImage = (
  raw: string,
  settings: TrackerSettings,
  dimensions?: SpyPixelInput
): ImageDecision =>
  decide(raw, settings, dimensions, (url) =>
    isTrackerUrl(url, trackerOptions(settings))
  );

/**
 * Image references decided per email, the same in the browser and on the
 * server. Each tracker check walks every list rule (about 0.3 ms), and a
 * 2 MB `<style>` can hold 100 000 `url()` that `maxElements` doesn't
 * limit. Past this, references are blocked (`too-many`): no ticket, no load.
 */
export const MAX_IMAGE_REFS = 5000;

export type ImageDecider = (
  raw: string,
  dimensions?: SpyPixelInput
) => ImageDecision;

/**
 * `decideImage` for one email: at most `MAX_IMAGE_REFS` references, and
 * the tracker-list check done once per normalized URL.
 */
export const createImageDecider = (
  settings: TrackerSettings,
  maxRefs = MAX_IMAGE_REFS
): ImageDecider => {
  let refs = 0;
  const options = trackerOptions(settings);
  const trackers = new Map<string, TrackerMatch | null>();
  const trackerOf = (url: string): TrackerMatch | null => {
    let tracker = trackers.get(url);
    if (tracker === undefined) {
      tracker = isTrackerUrl(url, options);
      trackers.set(url, tracker);
    }
    return tracker;
  };
  return (raw, dimensions) => {
    refs += 1;
    return refs > maxRefs
      ? { kind: "blocked", reason: "too-many" }
      : decide(raw, settings, dimensions, trackerOf);
  };
};
