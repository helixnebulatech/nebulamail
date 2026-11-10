// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";
import type { StyleXStyles } from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { colors, radius, shadow } from "./tokens.stylex";

/** apps/web's bp.mobile: one panel at a time, the whole screen wide. */
const PHONE = "@media (max-width: 640px)";

const styles = stylex.create({
  card: {
    backgroundColor: colors.bg,
    borderColor: colors.line,
    borderRadius: radius.xl,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh1,
    boxSizing: "border-box",
  },
  padLg: { padding: "22px" },
  padMd: { padding: "16px" },
  padNone: { padding: 0 },
  panel: {
    backgroundColor: colors.bg,
    borderColor: colors.line,
    // Phones (apps/web bp.mobile): the panel fills the screen edge to
    // edge, so a card's frame (corners, side borders, shadow) would only
    // show as stray lines along the screen's edges.
    borderRadius: { default: radius.xl, [PHONE]: 0 },
    borderStyle: "solid",
    borderWidth: { default: "1px", [PHONE]: 0 },
    boxShadow: { default: shadow.sh2, [PHONE]: "none" },
    boxSizing: "border-box",
    display: "flex",
    flexDirection: "column",
    minHeight: 0,
    overflow: "hidden",
    position: "relative",
  },
  raised: { backgroundColor: colors.raised },
});

export interface PanelProps {
  readonly children: ReactNode;
  /** Names the panel as a landmark region (e.g. "Lista de mensajes"). */
  readonly label?: string;
  /** Layout escape hatch (grid area, width, flex). */
  readonly xstyle?: StyleXStyles;
}

/** The rounded floating panel (list, reader): radius 16, elevation 2. */
export const Panel = ({ children, label, xstyle }: PanelProps) => (
  <section
    {...(label === undefined ? {} : { "aria-label": label })}
    {...stylex.props(styles.panel, xstyle)}
  >
    {children}
  </section>
);

export interface CardProps {
  readonly children: ReactNode;
  readonly padding?: "none" | "md" | "lg";
  readonly xstyle?: StyleXStyles;
}

const PADDING = {
  lg: styles.padLg,
  md: styles.padMd,
  none: styles.padNone,
} as const;

/** Quiet card: radius 16, hairline, elevation 1. */
export const Card = ({ children, padding = "lg", xstyle }: CardProps) => (
  <div {...stylex.props(styles.card, PADDING[padding], xstyle)}>{children}</div>
);

export interface BoxProps {
  readonly children?: ReactNode;
  readonly xstyle?: StyleXStyles;
}

/** Plain div for layout with StyleX (when a wrapper is needed). */
export const Box = ({ children, xstyle }: BoxProps) => (
  <div {...stylex.props(xstyle)}>{children}</div>
);
