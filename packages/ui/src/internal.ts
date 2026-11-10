// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Package-internal helpers. Not exported from index.ts.

import * as stylex from "@stylexjs/stylex";
import type {
  CompiledStyles,
  InlineStyles,
  StyleXArray,
} from "@stylexjs/stylex";
import { createContext, use } from "react";

import { colors } from "./tokens.stylex";

type StyleArg = StyleXArray<
  | (null | undefined | CompiledStyles)
  | boolean
  | Readonly<[CompiledStyles, InlineStyles]>
>;

/**
 * StyleX class names for Base UI's `className` (which also accepts a function
 * of the component state): `className={(s) => cls(a, s.open && b)}`.
 * Only for static styles; dynamic ones need `style` too.
 */
export const cls = (...args: readonly StyleArg[]): string =>
  stylex.props(...args).className ?? "";

/**
 * Where popups portal to. Inside a ThemeScope they portal into the scope so
 * they inherit its theme variables (null until its host element mounts);
 * outside any scope (undefined) Base UI uses document.body.
 */
export const PortalContainerContext = createContext<
  HTMLElement | null | undefined
>(undefined);

export const usePortalContainer = (): HTMLElement | null | undefined =>
  use(PortalContainerContext);

/** Shared style fragments. */
export const shared = stylex.create({
  focusRing: {
    outlineColor: { ":focus-visible": colors.focus, default: "transparent" },
    outlineOffset: "2px",
    outlineStyle: { ":focus-visible": "solid", default: "none" },
    outlineWidth: "2px",
  },
  /**
   * The ring inside the box, for items stacked tight in a list or sidebar
   * (an outside ring touches the neighbours and the sidebar's edge).
   */
  focusRingInset: { outlineOffset: "-2px" },
  resetButton: {
    appearance: "none",
    backgroundColor: "transparent",
    borderStyle: "none",
    borderWidth: 0,
    color: "inherit",
    fontFamily: "inherit",
    fontSize: "inherit",
    lineHeight: "inherit",
    margin: 0,
    padding: 0,
    textAlign: "start",
  },
  visuallyHidden: {
    borderWidth: 0,
    clipPath: "inset(50%)",
    height: "1px",
    margin: "-1px",
    overflow: "hidden",
    padding: 0,
    position: "absolute",
    whiteSpace: "nowrap",
    width: "1px",
  },
});
