// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";

import { Mark } from "./brand";

// The loader: the helixnebula swirl turning, like the nebula. Static with
// prefers-reduced-motion.

const REDUCED = "@media (prefers-reduced-motion: reduce)";

const turn = stylex.keyframes({
  from: { transform: "rotate(0deg)" },
  to: { transform: "rotate(360deg)" },
});

const styles = stylex.create({
  root: {
    animationDuration: "1.6s",
    animationIterationCount: "infinite",
    animationName: { default: turn, [REDUCED]: "none" },
    animationTimingFunction: "linear",
    display: "inline-flex",
    flexShrink: 0,
  },
});

const SIZES = { md: 28, sm: 14 } as const;

export interface SpinnerProps {
  readonly size?: "sm" | "md";
  /** "brand" = galaxy colours; "current" = text colour (inside buttons). */
  readonly tone?: "brand" | "current";
  /** Accessible name (e.g. "Cargando"). Without it the spinner is decorative. */
  readonly label?: string;
}

/** Loader: the turning swirl. */
export const Spinner = ({
  size = "md",
  tone = "brand",
  label,
}: SpinnerProps) => (
  <span
    {...(label === undefined
      ? { "aria-hidden": true }
      : { "aria-label": label, role: "status" })}
    {...stylex.props(styles.root)}
  >
    <Mark
      core="none"
      label={null}
      size={SIZES[size]}
      tone={tone === "brand" ? "brand" : "mono"}
    />
  </span>
);
