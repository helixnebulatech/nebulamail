// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Shared, DOM-free tracker module: used by the browser, the
// BFF and the image proxy. Import it as `@nebulamail/mail-render/trackers` on
// servers so no DOM code is pulled in.

export { MTB_PATTERN_COUNT, MTB_RULES, MTB_SOURCE } from "./data.generated";
export {
  CUSTOM_TRACKER_SERVICE,
  LINEAR_EQUIVALENTS,
  MAX_TRACKER_INPUT,
  hostMatches,
  isTrackerUrl,
  type TrackerMatch,
  type TrackerMatchOptions,
} from "./match";
export {
  SPACER_PATTERN,
  isSpyPixel,
  parseCssPx,
  parseHtmlDimension,
  parseInlineStyle,
  spyPixelReason,
  type SpyPixelInput,
  type SpyPixelReason,
} from "./spy-pixel";
