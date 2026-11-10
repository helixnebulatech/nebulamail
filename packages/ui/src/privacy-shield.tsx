// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";
import { useState } from "react";

import { ShieldGlyph } from "./brand";
import { colors, motion } from "./tokens.stylex";

const glow = stylex.keyframes({
  "0%": { opacity: 0, transform: "scale(0.6)" },
  "30%": { opacity: 1, transform: "scale(1.3)" },
  "100%": { opacity: 0, transform: "scale(1.6)" },
});

const ring = stylex.keyframes({
  "0%": { opacity: 0.9, transform: "scale(0.8)" },
  "100%": { opacity: 0, transform: "scale(2.2)" },
});

const pop = stylex.keyframes({
  "0%": { transform: "scale(1)" },
  "40%": { transform: "scale(1.18)" },
  "100%": { transform: "scale(1)" },
});

const REDUCED = "@media (prefers-reduced-motion: reduce)";

const styles = stylex.create({
  glyph: { display: "grid", position: "relative", zIndex: 1 },
  halo: {
    animationDuration: motion.pulse,
    animationName: { default: glow, [REDUCED]: "none" },
    animationTimingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
    backgroundColor: `color-mix(in srgb, ${colors.acc} 22%, transparent)`,
    borderRadius: "50%",
    inset: "-30%",
    opacity: 0,
    pointerEvents: "none",
    position: "absolute",
  },
  pop: {
    animationDuration: "500ms",
    animationName: { default: pop, [REDUCED]: "none" },
    animationTimingFunction: motion.easingSpring,
  },
  ring: {
    animationDuration: motion.pulse,
    animationName: { default: ring, [REDUCED]: "none" },
    animationTimingFunction: "cubic-bezier(0.2, 0.8, 0.2, 1)",
    borderColor: colors.n1,
    borderRadius: "50%",
    borderStyle: "solid",
    borderWidth: "2px",
    inset: "-10%",
    opacity: 0,
    pointerEvents: "none",
    position: "absolute",
  },
  root: {
    alignItems: "center",
    display: "inline-grid",
    flexShrink: 0,
    justifyContent: "center",
    position: "relative",
  },
});

export interface PrivacyShieldProps {
  readonly size?: number;
  /**
   * Change this value (e.g. the blocked-tracker count or the message id) to
   * play the glow once. Never loops; nothing plays on first render.
   */
  readonly pulse?: number | string;
  /** Accessible name; decorative when omitted. */
  readonly label?: string;
  /** In the surrounding text colour instead of the brand's (at rest). */
  readonly quiet?: boolean;
}

/** The shield with the spark: protection is the brand. One-time glow pulse. */
export const PrivacyShield = ({
  size = 20,
  pulse,
  label,
  quiet = false,
}: PrivacyShieldProps) => {
  // Remember the last value; when it changes, start a new one-shot pulse
  // (state adjusted during render, React's pattern for prop changes).
  const [lastPulse, setLastPulse] = useState(pulse);
  const [playing, setPlaying] = useState<string | null>(null);
  if (pulse !== lastPulse) {
    setLastPulse(pulse);
    setPlaying(String(pulse));
  }
  const animate = playing !== null;
  const key = playing ?? "static";
  return (
    <span
      {...(label === undefined ? {} : { "aria-label": label, role: "img" })}
      {...stylex.props(styles.root)}
    >
      {animate ? (
        <>
          <span key={`halo-${key}`} {...stylex.props(styles.halo)} />
          <span key={`ring-${key}`} {...stylex.props(styles.ring)} />
        </>
      ) : null}
      <span
        key={`glyph-${animate ? key : "static"}`}
        {...stylex.props(styles.glyph, animate ? styles.pop : null)}
      >
        <ShieldGlyph quiet={quiet} size={size} />
      </span>
    </span>
  );
};
