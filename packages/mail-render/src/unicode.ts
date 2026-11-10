// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Display-text hygiene for sender names, subjects and attachment filenames
//"factura\u202Efdp.exe" renders
// as "facturaexe.pdf". DOM-free.

/**
 * Bidirectional formatting characters: LRE, RLE, PDF, LRO, RLO (U+202A–E),
 * LRI, RLI, FSI, PDI (U+2066–9), LRM, RLM (U+200E/F) and ALM (U+061C).
 */
export const BIDI_CONTROLS = /[\u061C\u200E\u200F\u202A-\u202E\u2066-\u2069]/gu;

/**
 * Invisible characters with no legitimate use in a name or filename:
 * zero-width space, word joiner, invisible operators (U+2061–4), BOM /
 * ZWNBSP, Mongolian vowel separator, soft hyphen, Hangul fillers and
 * interlinear annotation marks. ZWJ/ZWNJ (U+200C/D) are kept: emoji
 * sequences and Persian/Indic scripts need them.
 */
export const INVISIBLE_CHARS =
  /[\u00AD\u115F\u1160\u180E\u200B\u2060-\u2064\u3164\uFEFF\uFFA0\uFFF9-\uFFFB]/gu;

// oxlint-disable no-control-regex -- matching control characters is the point here
const OTHER_CONTROLS =
  /[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F-\u009F]/gu;
// oxlint-enable no-control-regex
const LINE_BREAKS = /[\t\n\r\u2028\u2029]+/gu;

export const hasBidiControls = (text: string): boolean => {
  BIDI_CONTROLS.lastIndex = 0;
  return BIDI_CONTROLS.test(text);
};

export const stripBidiControls = (text: string): string =>
  text.replace(BIDI_CONTROLS, "");

export const stripInvisible = (text: string): string =>
  text.replaceAll(INVISIBLE_CHARS, "");

/**
 * Text safe to show on one line: no bidi controls, no invisible or control
 * characters, line breaks collapsed to spaces, NFC-normalized. Show it
 * inside a `<bdi>` (or `dir="auto"` element) so that right-to-left names
 * cannot reorder the surrounding UI.
 */
export const cleanDisplayText = (text: string): string =>
  stripInvisible(stripBidiControls(text))
    .replaceAll(OTHER_CONTROLS, "")
    .replaceAll(LINE_BREAKS, " ")
    .normalize("NFC")
    .trim();

export interface DisplayTextResult {
  readonly text: string;
  /** The original contained bidi controls or invisible characters. */
  readonly altered: boolean;
  readonly hadBidiControls: boolean;
}

export const analyzeDisplayText = (text: string): DisplayTextResult => {
  const cleaned = cleanDisplayText(text);
  return {
    altered: cleaned !== text.trim().normalize("NFC"),
    hadBidiControls: hasBidiControls(text),
    text: cleaned,
  };
};
