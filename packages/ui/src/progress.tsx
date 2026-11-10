// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import { Progress } from "@base-ui/react/progress";
import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { cls } from "./internal";
import { colors, font, motion } from "./tokens.stylex";

const REDUCED = "@media (prefers-reduced-motion: reduce)";

// Indeterminate: a short streak sweeping across the track. With
// prefers-reduced-motion it stands still, filling the track at half
// strength, so the bar still looks busy and not empty.
const sweep = stylex.keyframes({
  from: { transform: "translateX(-100%)" },
  to: { transform: "translateX(250%)" },
});

const styles = stylex.create({
  indeterminate: {
    animationDuration: "1.4s",
    animationIterationCount: "infinite",
    animationName: { default: sweep, [REDUCED]: "none" },
    animationTimingFunction: "ease-in-out",
    opacity: { default: 1, [REDUCED]: 0.5 },
    transitionProperty: "none",
    width: { default: "40%", [REDUCED]: "100%" },
  },
  indicator: {
    backgroundColor: colors.acc,
    borderRadius: "9px",
    display: "block",
    height: "100%",
    transitionDuration: motion.slow,
    transitionProperty: "width",
    transitionTimingFunction: motion.easing,
  },
  label: { color: colors.fg3 },
  root: {
    color: colors.fg3,
    display: "flex",
    flexDirection: "column",
    fontFamily: font.sans,
    fontSize: font.sizeSm,
    rowGap: "6px",
  },
  row: { display: "flex", justifyContent: "space-between" },
  track: {
    backgroundColor: colors.line,
    borderRadius: "9px",
    height: "5px",
    overflow: "hidden",
  },
  value: { color: colors.fg3, fontVariantNumeric: "tabular-nums" },
});

export interface ProgressBarProps {
  /**
   * `null` = indeterminate: something is happening but how much is unknown
   * (e.g. an upload whose transport can't report progress). The bar sweeps
   * (still with prefers-reduced-motion) and screen readers hear `busyText`.
   */
  readonly value: number | null;
  /** Spoken value while indeterminate (e.g. "Subiendo…"). */
  readonly busyText?: string;
  readonly max?: number;
  /** Visible label (e.g. "1,2 GB de 15 GB"). */
  readonly label?: ReactNode;
  /** Content at the end of the label row (e.g. a "Gestionar" link). */
  readonly end?: ReactNode;
  /** Accessible name when there is no visible label. */
  readonly "aria-label"?: string;
}

/** Thin bar with the galaxy streak (storage, uploads). */
export const ProgressBar = ({
  value,
  max = 100,
  busyText,
  label,
  end,
  "aria-label": ariaLabel,
}: ProgressBarProps) => (
  <Progress.Root
    className={cls(styles.root)}
    max={max}
    value={value}
    {...(ariaLabel === undefined ? {} : { "aria-label": ariaLabel })}
    {...(value === null && busyText !== undefined
      ? { getAriaValueText: () => busyText }
      : {})}
  >
    {label === undefined && end === undefined ? null : (
      <div {...stylex.props(styles.row)}>
        <Progress.Label className={cls(styles.label)}>{label}</Progress.Label>
        {end}
      </div>
    )}
    <Progress.Track className={cls(styles.track)}>
      <Progress.Indicator
        className={cls(
          styles.indicator,
          value === null ? styles.indeterminate : null
        )}
      />
    </Progress.Track>
  </Progress.Root>
);

const ringStyles = stylex.create({
  fill: {
    fill: "none",
    strokeLinecap: "round",
    strokeWidth: 3.5,
    transitionDuration: motion.slow,
    transitionProperty: "stroke-dasharray",
  },
  root: { display: "block", flexShrink: 0, transform: "rotate(-90deg)" },
  fillColor: { stroke: colors.acc },
  track: { fill: "none", stroke: colors.line, strokeWidth: 3.5 },
  wrap: { display: "inline-flex", flexShrink: 0 },
});

const RING_RADIUS = 15;
const CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

export interface ProgressRingProps {
  readonly value: number;
  readonly max?: number;
  readonly size?: number;
  /** Accessible name, e.g. "Configuración: 2 de 4 pasos". */
  readonly label: string;
}

/** Progress ring in the accent colour (onboarding steps). */
export const ProgressRing = ({
  value,
  max = 100,
  size = 32,
  label,
}: ProgressRingProps) => {
  const ratio = max > 0 ? Math.min(Math.max(value / max, 0), 1) : 0;
  const filled = ratio * CIRCUMFERENCE;
  return (
    <Progress.Root
      aria-label={label}
      className={cls(ringStyles.wrap)}
      max={max}
      value={value}
    >
      <svg
        aria-hidden
        focusable={false}
        height={size}
        viewBox="0 0 36 36"
        width={size}
        {...stylex.props(ringStyles.root)}
      >
        <circle
          cx="18"
          cy="18"
          r={RING_RADIUS}
          {...stylex.props(ringStyles.track)}
        />
        {ratio > 0 ? (
          <circle
            cx="18"
            cy="18"
            r={RING_RADIUS}
            strokeDasharray={`${filled} ${CIRCUMFERENCE}`}
            {...stylex.props(ringStyles.fill, ringStyles.fillColor)}
          />
        ) : null}
      </svg>
    </Progress.Root>
  );
};
