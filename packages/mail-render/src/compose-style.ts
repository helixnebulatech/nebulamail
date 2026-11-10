// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The fonts, sizes and directions the compose editor offers,
// in one place for the editor (@nebulamail/ui), the sanitizer (compose.ts) and
// the app. DOM-free, so the editor can import it without the sanitizer
// (`@nebulamail/mail-render/compose-style`).
//
// Only fonts every system has, each with its generic family as fallback:
// mail can't carry web fonts (many clients block them, and our reader
// blocks remote ones for privacy). Values are written in one
// canonical form: the sanitizer rewrites whatever matches to it and drops
// the rest, so nothing else (`url()`, `var()`, `@font-face`…) gets through.

/** A font of the editor's menu. */
export interface ComposeFont {
  readonly id: ComposeFontId;
  /** Shown in the menu (a font name, not translated). */
  readonly label: string;
  /** The CSS `font-family` written into the email. */
  readonly stack: string;
}

export const COMPOSE_FONT_IDS = [
  "sans",
  "serif",
  "georgia",
  "arial",
  "helvetica",
  "monospace",
  "tahoma",
  "verdana",
  "times",
  "trebuchet",
] as const;
export type ComposeFontId = (typeof COMPOSE_FONT_IDS)[number];

export const COMPOSE_FONTS: readonly ComposeFont[] = [
  { id: "sans", label: "Sans Serif", stack: "sans-serif" },
  { id: "serif", label: "Serif", stack: "serif" },
  { id: "georgia", label: "Georgia", stack: "Georgia, serif" },
  { id: "arial", label: "Arial", stack: "Arial, sans-serif" },
  { id: "helvetica", label: "Helvetica", stack: "Helvetica, sans-serif" },
  { id: "monospace", label: "Monospace", stack: "monospace" },
  { id: "tahoma", label: "Tahoma", stack: "Tahoma, sans-serif" },
  { id: "verdana", label: "Verdana", stack: "Verdana, sans-serif" },
  { id: "times", label: "Times New Roman", stack: "Times New Roman, serif" },
  {
    id: "trebuchet",
    label: "Trebuchet MS",
    stack: "Trebuchet MS, sans-serif",
  },
];

/** Font sizes of the editor's menu, in px. */
export const COMPOSE_FONT_SIZES = [
  "10px",
  "12px",
  "14px",
  "16px",
  "18px",
  "20px",
  "24px",
  "32px",
] as const;
export type ComposeFontSize = (typeof COMPOSE_FONT_SIZES)[number];

/** Paragraph directions the editor sets (`dir`). */
export const TEXT_DIRECTIONS = ["ltr", "rtl"] as const;
export type TextDirection = (typeof TEXT_DIRECTIONS)[number];

/** What a new message starts with (Proton's defaults). */
export const DEFAULT_COMPOSE_FONT: ComposeFontId = "arial";
export const DEFAULT_COMPOSE_FONT_SIZE: ComposeFontSize = "14px";

export const fontById = (id: ComposeFontId): ComposeFont =>
  COMPOSE_FONTS.find((font) => font.id === id) ?? {
    id: "sans",
    label: "Sans Serif",
    stack: "sans-serif",
  };

export const isComposeFontId = (value: unknown): value is ComposeFontId =>
  COMPOSE_FONT_IDS.some((id) => id === value);

export const isComposeFontSize = (value: unknown): value is ComposeFontSize =>
  COMPOSE_FONT_SIZES.some((size) => size === value);

export const isTextDirection = (value: unknown): value is TextDirection =>
  value === "ltr" || value === "rtl";

/** Longest `font-family` value looked at (a real one is far shorter). */
const MAX_FAMILY_LENGTH = 200;

/** Family names: letters, digits, spaces, hyphens and quotes only. */
const FAMILY_LIST = /^[\w\s'",-]+$/u;

/** A family name without quotes, lower case, single spaces. */
const familyKey = (name: string): string =>
  name.replaceAll(/["']/gu, "").replaceAll(/\s+/gu, " ").trim().toLowerCase();

/** The first family of each font, as `familyKey` writes it. */
const FIRST_FAMILY: ReadonlyMap<string, ComposeFontId> = new Map(
  COMPOSE_FONTS.map(
    (font) => [familyKey(font.stack.split(",")[0] ?? ""), font.id] as const
  )
);

/**
 * The font a CSS `font-family` value asks for, or null if none of ours.
 * The first family decides ("arial, helvetica, sans-serif" is Arial), so
 * what Gmail or Outlook write keeps its font.
 */
export const composeFontOf = (value: string): ComposeFontId | null => {
  if (value.length > MAX_FAMILY_LENGTH || !FAMILY_LIST.test(value)) {
    return null;
  }
  const [first] = value.split(",");
  return FIRST_FAMILY.get(familyKey(first ?? "")) ?? null;
};

/** A `font-family` value as the sanitizer keeps it (canonical), or null. */
export const safeFontFamily = (value: string): string | null => {
  const id = composeFontOf(value);
  return id === null ? null : fontById(id).stack;
};

/** A `font-size` value as kept: one of ours exactly (any case), or null. */
export const safeFontSize = (value: string): ComposeFontSize | null => {
  const size = value.trim().toLowerCase();
  return isComposeFontSize(size) ? size : null;
};

/** The style a message starts with: its default font and size. */
export interface ComposeBodyStyle {
  readonly font: ComposeFontId;
  readonly size: ComposeFontSize;
}

/**
 * The HTML that goes into the email: the editor's, inside a `<div>` with
 * the default font and size, so the recipient sees what the writer saw.
 * The editor drops the wrapper when a draft is opened again (its schema
 * has no such block), so it never piles up.
 */
export const withBodyStyle = (html: string, style: ComposeBodyStyle): string =>
  `<div style="font-family: ${fontById(style.font).stack}; font-size: ${style.size}">${html}</div>`;
