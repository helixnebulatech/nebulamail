// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Spy-pixel rule: our rule combined with FairEmail's.
// A remote image is a spy pixel when, after resolving its size:
//   - a side is ≤ 2 px, or
//   - its area is ≤ 25 px² (a 0 side takes the other side's value), or
//   - max-width or max-height is ≤ 1 px.
// Known spacers are exempt. DOM-free: works on attribute and style strings.

export interface SpyPixelInput {
  /** `width` attribute (string as written, or a number). */
  readonly width?: string | number | null;
  /** `height` attribute. */
  readonly height?: string | number | null;
  /** Inline `style` attribute. */
  readonly style?: string | null;
  /** Image URL, used for the spacer exceptions. */
  readonly url?: string | null;
}

export type SpyPixelReason = "small-side" | "small-area" | "max-size";

const MAX_SIDE = 2;
const MAX_AREA = 25;
const MAX_CLAMP = 1;

/** Upstream MailTrackerBlocker spacer exceptions (cid:, spacer, transparent.gif…). */
export const SPACER_PATTERN =
  /cid:|spacer|attachments\.office\.net\/owa\/|fedex_collective_logo_|apple_logo_web|sidebar-gradient|transparent\.gif/iu;

const HTML_DIMENSION = /^[\t\n\f\r ]*(?<value>\d+(?:\.\d+)?)(?<rest>.*)$/su;

/**
 * HTML "rules for parsing dimension values": leading digits, anything after
 * is ignored except `%`, which makes it relative (unknown here).
 */
export const parseHtmlDimension = (
  value: string | number | null | undefined
): number | null => {
  if (typeof value === "number") {
    return Number.isFinite(value) && value >= 0 ? value : null;
  }
  if (value === null || value === undefined) {
    return null;
  }
  const match = HTML_DIMENSION.exec(value);
  if (!match) {
    return null;
  }
  const rest = (match.groups?.rest ?? "").trimStart();
  if (rest.startsWith("%")) {
    return null;
  }
  return Number(match.groups?.value);
};

// Unambiguous (each part matches disjoint characters), so linear. CSS does
// not allow a space between the number and its unit (`10 px` is invalid
// and the browser ignores it), so there is none here either.
const CSS_LENGTH =
  /^(?<value>-?(?:\d+(?:\.\d+)?|\.\d+))(?<unit>[a-z]*)\s*(?:!\s*important)?$/iu;
/** No real CSS length is this long; longer values are not a length. */
const MAX_CSS_LENGTH = 64;

/** CSS length in px. `0` with any unit is 0; other units are unknown. */
export const parseCssPx = (value: string): number | null => {
  const trimmed = value.trim();
  if (trimmed.length > MAX_CSS_LENGTH) {
    return null;
  }
  const match = CSS_LENGTH.exec(trimmed);
  if (!match) {
    return null;
  }
  const n = Number(match.groups?.value);
  const unit = (match.groups?.unit ?? "").toLowerCase();
  if (n === 0) {
    return 0;
  }
  if (unit === "px" || unit === "") {
    return n < 0 ? null : n;
  }
  return null;
};

/** Last declaration wins, like the cascade inside one style attribute. */
export const parseInlineStyle = (style: string): Map<string, string> => {
  const declarations = new Map<string, string>();
  for (const part of style.split(";")) {
    const colon = part.indexOf(":");
    if (colon > 0) {
      const name = part.slice(0, colon).trim().toLowerCase();
      const value = part.slice(colon + 1).trim();
      if (name !== "" && value !== "") {
        declarations.set(name, value);
      }
    }
  }
  return declarations;
};

const styleLength = (
  declarations: Map<string, string>,
  name: string
): number | null => {
  const value = declarations.get(name);
  return value === undefined ? null : parseCssPx(value);
};

const resolveSide = (
  styleValue: number | null,
  attrValue: number | null
): number | null =>
  // A CSS width/height overrides the presentational attribute.
  styleValue ?? attrValue;

export const spyPixelReason = (input: SpyPixelInput): SpyPixelReason | null => {
  if (typeof input.url === "string" && SPACER_PATTERN.test(input.url)) {
    return null;
  }
  const declarations = parseInlineStyle(input.style ?? "");
  const maxWidth = styleLength(declarations, "max-width");
  const maxHeight = styleLength(declarations, "max-height");
  if (
    (maxWidth !== null && maxWidth <= MAX_CLAMP) ||
    (maxHeight !== null && maxHeight <= MAX_CLAMP)
  ) {
    return "max-size";
  }
  const width = resolveSide(
    styleLength(declarations, "width"),
    parseHtmlDimension(input.width)
  );
  const height = resolveSide(
    styleLength(declarations, "height"),
    parseHtmlDimension(input.height)
  );
  if (
    (width !== null && width <= MAX_SIDE) ||
    (height !== null && height <= MAX_SIDE)
  ) {
    return "small-side";
  }
  if (width !== null && height !== null) {
    const w = width === 0 ? height : width;
    const h = height === 0 ? width : height;
    if (w * h <= MAX_AREA) {
      return "small-area";
    }
  }
  return null;
};

export const isSpyPixel = (input: SpyPixelInput): boolean =>
  spyPixelReason(input) !== null;
