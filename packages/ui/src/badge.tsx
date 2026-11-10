// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { Mark } from "./brand";
import type { Icon } from "./button";
import { colors, font, radius, tint } from "./tokens.stylex";

const styles = stylex.create({
  base: {
    alignItems: "center",
    borderRadius: radius.pill,
    columnGap: "4px",
    display: "inline-flex",
    flexShrink: 0,
    fontFamily: font.sans,
    fontSize: font.sizeXs,
    fontWeight: 650,
    height: "20px",
    lineHeight: 1,
    paddingInline: "7px",
    whiteSpace: "nowrap",
  },
  icon: { flexShrink: 0 },
});

const tones = stylex.create({
  brand: { backgroundColor: colors.accSoft, color: colors.accText },
  danger: { backgroundColor: colors.dangerSoft, color: colors.danger },
  info: { backgroundColor: colors.infoSoft, color: colors.info },
  neutral: { backgroundColor: colors.hover, color: colors.fg2 },
  ok: { backgroundColor: colors.okSoft, color: colors.ok },
  star: { backgroundColor: colors.starSoft, color: colors.starText },
  warn: { backgroundColor: colors.warnSoft, color: colors.warn },
});

export type BadgeTone = keyof typeof tones;

export interface BadgeProps {
  readonly children: ReactNode;
  /** brand = «Oficial», ok = «Verificado», star = «VIP». */
  readonly tone?: BadgeTone;
  readonly icon?: Icon;
  /** Leads with the mono Nebula mark (official notices). */
  readonly mark?: boolean;
}

/** Small status pill. */
export const Badge = ({
  children,
  tone = "neutral",
  icon: IconGlyph,
  mark = false,
}: BadgeProps) => (
  <span {...stylex.props(styles.base, tones[tone])}>
    {mark ? <Mark core="none" label={null} size={11} tone="mono" /> : null}
    {IconGlyph === undefined ? null : (
      <IconGlyph aria-hidden size={12} {...stylex.props(styles.icon)} />
    )}
    {children}
  </span>
);

const tagStyles = stylex.create({
  base: {
    alignItems: "center",
    borderRadius: radius.pill,
    display: "inline-flex",
    flexShrink: 0,
    fontFamily: font.sans,
    fontSize: "11.5px",
    fontWeight: font.weightSemibold,
    height: "20px",
    lineHeight: 1,
    paddingInline: "8px",
    whiteSpace: "nowrap",
  },
  // The hue comes from the label; lightness from the theme (tint tokens).
  hue: (hue: number) => ({
    backgroundColor: `oklch(${tint.labelBg} ${hue})`,
    color: `oklch(${tint.labelFg} ${hue})`,
  }),
});

export interface TagProps {
  readonly children: ReactNode;
  /** Label colour as an HSL hue (0–360). */
  readonly hue: number;
}

/** User label chip (Casa, Trabajo…): soft fill derived from a hue. */
export const Tag = ({ children, hue }: TagProps) => (
  <span {...stylex.props(tagStyles.base, tagStyles.hue(hue))}>{children}</span>
);
