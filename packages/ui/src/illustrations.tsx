// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Empty-state illustrations: line art in the text colours with the
// helixnebula swirl and stars as the only colour.

import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import { Mark, sparkPath } from "./brand";
import { colors } from "./tokens.stylex";

const s = stylex.create({
  // Flat accent, no gradients.
  accentStroke: { stroke: colors.acc },
  dot: { fill: colors.fg3 },
  fill: {
    fill: colors.bg,
    stroke: colors.fg3,
    strokeLinejoin: "round",
    strokeWidth: 2,
  },
  line: {
    fill: "none",
    stroke: colors.fg3,
    strokeLinecap: "round",
    strokeLinejoin: "round",
    strokeWidth: 2,
  },
  line2: {
    fill: "none",
    stroke: colors.line2,
    strokeLinecap: "round",
    strokeWidth: 2,
  },
  root: { display: "block", flexShrink: 0, maxWidth: "100%" },
  star: { fill: colors.core },
  orbit: { fill: "none", strokeLinecap: "round", strokeWidth: 3 },
});

interface IllustrationProps {
  /** Accessible description; decorative when omitted (the title says it). */
  readonly label?: string;
  /** Smaller (140×96 instead of 220×150), for a pane's resting state. */
  readonly compact?: boolean;
}

const Frame = ({
  label,
  children,
  compact = false,
}: IllustrationProps & {
  readonly children: ReactNode;
}) => (
  <svg
    height={compact ? "96" : "150"}
    viewBox="0 0 220 150"
    width={compact ? "140" : "220"}
    {...(label === undefined
      ? { "aria-hidden": true, focusable: false }
      : { "aria-label": label, role: "img" })}
    {...stylex.props(s.root)}
  >
    {children}
  </svg>
);

/** A decorative star in the galaxy's cyan. */
const Star = ({
  x,
  y,
  r,
}: {
  readonly x: number;
  readonly y: number;
  readonly r: number;
}) => <path d={sparkPath(x, y, r)} {...stylex.props(s.star)} />;

/** Contacts: an address card with the nebula behind it. */
export const IllustrationContacts = ({
  compact = false,
  label,
}: IllustrationProps) => (
  <Frame compact={compact} {...(label === undefined ? {} : { label })}>
    <rect
      height="70"
      rx="12"
      width="112"
      x="68"
      y="44"
      {...stylex.props(s.fill)}
    />
    <rect
      height="70"
      rx="12"
      width="112"
      x="54"
      y="36"
      {...stylex.props(s.fill)}
    />
    <circle cx="84" cy="64" r="13" {...stylex.props(s.fill)} />
    <circle
      cx="84"
      cy="64"
      r="21"
      strokeDasharray="2 6"
      {...stylex.props(s.accentStroke, s.orbit)}
    />
    <path d="M108 58h40M108 70h28M70 92h76" {...stylex.props(s.line2)} />
    <Star r={5} x={170} y={24} />
    <Star r={3.5} x={40} y={30} />
    <Star r={3} x={194} y={96} />
  </Frame>
);

/**
 * Pick a message: an open envelope with its letter half out, the nebula
 * behind it (the reading pane's resting state).
 */
export const IllustrationOpenMail = ({
  compact = false,
  label,
}: IllustrationProps) => (
  <Frame compact={compact} {...(label === undefined ? {} : { label })}>
    {/* Back of the envelope with its open flap. */}
    <path d="M58 70l52-36 52 36v52H58z" {...stylex.props(s.fill)} />
    {/* The letter, half out. */}
    <rect
      height="66"
      rx="6"
      width="80"
      x="70"
      y="30"
      {...stylex.props(s.fill)}
    />
    <path d="M82 46h40M82 58h56M82 70h48" {...stylex.props(s.line2)} />
    <circle
      cx="134"
      cy="46"
      r="7"
      strokeDasharray="2 5"
      {...stylex.props(s.accentStroke, s.orbit)}
    />
    {/* Front pocket, over the letter. */}
    <path
      d="M58 74l52 30 52-30v40a8 8 0 0 1-8 8H66a8 8 0 0 1-8-8z"
      {...stylex.props(s.fill)}
    />
    <Star r={5} x={174} y={28} />
    <Star r={3.5} x={44} y={40} />
    <Star r={3} x={186} y={104} />
  </Frame>
);

/** Inbox zero: the nebula over an empty tray. */
export const IllustrationInboxZero = ({
  compact = false,
  label,
}: IllustrationProps) => (
  <Frame compact={compact} {...(label === undefined ? {} : { label })}>
    <Mark label={null} size={56} x={82} y={14} />
    <Star r={5} x={168} y={18} />
    <Star r={3.5} x={52} y={16} />
    <Star r={3} x={190} y={78} />
    <path d="M58 92h104l-10-18H68z" {...stylex.props(s.fill)} />
    <path
      d="M50 96h34l8 12h36l8-12h34v30a10 10 0 0 1-10 10H60a10 10 0 0 1-10-10z"
      {...stylex.props(s.fill)}
    />
    <path d="M86 120h48" {...stylex.props(s.line2)} />
  </Frame>
);

/** No results: a magnifier with an empty orbit. */
export const IllustrationNoResults = ({
  compact = false,
  label,
}: IllustrationProps) => (
  <Frame compact={compact} {...(label === undefined ? {} : { label })}>
    <circle cx="96" cy="66" r="38" {...stylex.props(s.fill)} />
    <path d="M124 94l26 26" strokeWidth="7" {...stylex.props(s.line)} />
    <circle
      cx="96"
      cy="66"
      r="20"
      strokeDasharray="2 7"
      {...stylex.props(s.accentStroke, s.orbit)}
    />
    <Star r={6} x={96} y={66} />
    <Star r={5} x={164} y={34} />
    <Star r={2.5} x={150} y={52} />
    <Star r={3.5} x={40} y={108} />
    <circle cx="176" cy="22" r="2" {...stylex.props(s.dot)} />
    <circle cx="186" cy="14" r="1.5" {...stylex.props(s.dot)} />
  </Frame>
);

/** Privacy: a shield with the nebula inside, keeping trackers out. */
export const IllustrationPrivacy = ({
  compact = false,
  label,
}: IllustrationProps) => (
  <Frame compact={compact} {...(label === undefined ? {} : { label })}>
    <ellipse
      cx="110"
      cy="78"
      rx="86"
      ry="26"
      strokeDasharray="3 6"
      transform="rotate(-12 110 78)"
      {...stylex.props(s.line2)}
    />
    <path
      d="M110 22c12 9 26 13 40 13v32c0 30-20 44-40 52-20-8-40-22-40-52V35c14 0 28-4 40-13z"
      {...stylex.props(s.fill)}
    />
    <Mark label={null} size={44} x={88} y={50} />
    {[
      [34, 70],
      [186, 52],
      [170, 112],
    ].map(([cx = 0, cy = 0]) => (
      <g key={`${cx}-${cy}`}>
        <circle
          cx={cx}
          cy={cy}
          r="7"
          strokeWidth="1.8"
          {...stylex.props(s.fill)}
        />
        <path
          d={`M${cx - 3} ${cy - 3}l6 6M${cx + 3} ${cy - 3}l-6 6`}
          strokeWidth="1.8"
          {...stylex.props(s.line)}
        />
      </g>
    ))}
    <Star r={4} x={58} y={32} />
    <Star r={3} x={176} y={22} />
  </Frame>
);
