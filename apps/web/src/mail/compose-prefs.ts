// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Writing preferences (Settings › General › Writing): the
// default font and size of new messages and whether they start as HTML or
// plain text. Kept on the device and synced like the other choices
// (§6.1b); nothing sensitive.

import {
  COMPOSE_FONT_IDS,
  COMPOSE_FONT_SIZES,
  DEFAULT_COMPOSE_FONT,
  DEFAULT_COMPOSE_FONT_SIZE,
} from "@nebulamail/mail-render";
import type {
  ComposeBodyStyle,
  ComposeFontId,
  ComposeFontSize,
} from "@nebulamail/mail-render";

import { readLocalChoice, useLocalChoice } from "../lib/use-local-flag";
import { recordChange } from "../prefs-sync/sync";

export const COMPOSE_FORMATS = ["html", "text"] as const;
export type ComposeFormat = (typeof COMPOSE_FORMATS)[number];
export const DEFAULT_COMPOSE_FORMAT: ComposeFormat = "html";

/** Where the device keeps each choice (localStorage; synced). */
export const COMPOSE_FONT_KEY = "wm-compose-font";
export const COMPOSE_FONT_SIZE_KEY = "wm-compose-font-size";
export const COMPOSE_FORMAT_KEY = "wm-compose-format";

export interface ComposePrefs {
  readonly format: ComposeFormat;
  readonly style: ComposeBodyStyle;
}

/** The writing preferences now (outside React, when a window opens). */
export const readComposePrefs = (): ComposePrefs => ({
  format: readLocalChoice(
    COMPOSE_FORMAT_KEY,
    COMPOSE_FORMATS,
    DEFAULT_COMPOSE_FORMAT
  ),
  style: {
    font: readLocalChoice(
      COMPOSE_FONT_KEY,
      COMPOSE_FONT_IDS,
      DEFAULT_COMPOSE_FONT
    ),
    size: readLocalChoice(
      COMPOSE_FONT_SIZE_KEY,
      COMPOSE_FONT_SIZES,
      DEFAULT_COMPOSE_FONT_SIZE
    ),
  },
});

/** Each preference and its setter (saved here and synced). */
export const useComposePrefs = () => {
  const [font, setFont] = useLocalChoice(
    COMPOSE_FONT_KEY,
    COMPOSE_FONT_IDS,
    DEFAULT_COMPOSE_FONT
  );
  const [size, setSize] = useLocalChoice(
    COMPOSE_FONT_SIZE_KEY,
    COMPOSE_FONT_SIZES,
    DEFAULT_COMPOSE_FONT_SIZE
  );
  const [format, setFormat] = useLocalChoice(
    COMPOSE_FORMAT_KEY,
    COMPOSE_FORMATS,
    DEFAULT_COMPOSE_FORMAT
  );
  return {
    font,
    format,
    setFont: (next: ComposeFontId): void => {
      setFont(next);
      recordChange("composeFont", next);
    },
    setFormat: (next: ComposeFormat): void => {
      setFormat(next);
      recordChange("composeFormat", next);
    },
    setSize: (next: ComposeFontSize): void => {
      setSize(next);
      recordChange("composeFontSize", next);
    },
    size,
  };
};
