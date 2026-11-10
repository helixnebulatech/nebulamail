// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Nebula Mail brand: the mark (the helixnebula swirl: the Helix Nebula seen
// as an eye, arms turning around a ring with the central star as its
// pupil), the app icon, the wordmark and the small glyphs (star, the
// nebula line of the active tab, privacy shield). Drawn after the original
// helixnebula logo.

import * as stylex from "@stylexjs/stylex";
import { useId } from "react";

import { colors, font } from "./tokens.stylex";

/** Four-point star path centred on (cx, cy) with radius r. */
export const sparkPath = (cx: number, cy: number, r: number): string => {
  const k = r * 0.22;
  return `M${cx} ${cy - r}C${cx + k} ${cy - k} ${cx + k} ${cy - k} ${cx + r} ${cy}C${cx + k} ${cy + k} ${cx + k} ${cy + k} ${cx} ${cy + r}C${cx - k} ${cy + k} ${cx - k} ${cy + k} ${cx - r} ${cy}C${cx - k} ${cy - k} ${cx - k} ${cy - k} ${cx} ${cy - r}Z`;
};

// The arms of the swirl in the 64-unit grid: 8 thin ones like the original
// logo for large sizes, 6 thicker ones that stay readable down to 16 px.
const ARMS_FULL = [
  "M32.0 16.5C33.0 16.1 35.6 14.6 37.7 14.3C39.8 13.9 42.3 13.9 44.7 14.4C47.1 14.9 49.7 15.8 52.1 17.3C54.4 18.7 57.5 22.2 58.6 23.2",
  "M43.0 21.0C43.8 21.5 46.4 22.6 47.9 23.9C49.4 25.1 50.9 26.8 52.0 28.8C53.1 30.7 54.0 33.0 54.4 35.4C54.8 37.8 54.4 41.9 54.4 43.2",
  "M47.5 32.0C47.9 33.0 49.4 35.6 49.7 37.7C50.1 39.8 50.1 42.3 49.6 44.7C49.1 47.1 48.2 49.7 46.7 52.1C45.3 54.4 41.8 57.5 40.8 58.6",
  "M43.0 43.0C42.5 43.8 41.4 46.4 40.1 47.9C38.9 49.4 37.2 50.9 35.2 52.0C33.3 53.1 31.0 54.0 28.6 54.4C26.2 54.8 22.1 54.4 20.8 54.4",
  "M32.0 47.5C31.0 47.9 28.4 49.4 26.3 49.7C24.2 50.1 21.7 50.1 19.3 49.6C16.9 49.1 14.3 48.2 11.9 46.7C9.6 45.3 6.5 41.8 5.4 40.8",
  "M21.0 43.0C20.2 42.5 17.6 41.4 16.1 40.1C14.6 38.9 13.1 37.2 12.0 35.2C10.9 33.3 10.0 31.0 9.6 28.6C9.2 26.2 9.6 22.1 9.6 20.8",
  "M16.5 32.0C16.1 31.0 14.6 28.4 14.3 26.3C13.9 24.2 13.9 21.7 14.4 19.3C14.9 16.9 15.8 14.3 17.3 11.9C18.7 9.6 22.2 6.5 23.2 5.4",
  "M21.0 21.0C21.5 20.2 22.6 17.6 23.9 16.1C25.1 14.6 26.8 13.1 28.8 12.0C30.7 10.9 33.0 10.0 35.4 9.6C37.8 9.2 41.9 9.6 43.2 9.6",
] as const;

const ARMS_COMPACT = [
  "M32.0 16.0C32.9 15.6 35.4 14.1 37.4 13.7C39.4 13.2 41.8 13.0 44.1 13.3C46.4 13.6 49.0 14.3 51.3 15.5C53.6 16.7 56.9 19.5 58.0 20.4",
  "M45.9 24.0C46.5 24.6 48.8 26.2 50.0 27.7C51.2 29.2 52.3 31.1 53.0 33.1C53.7 35.1 54.1 37.5 54.2 39.8C54.2 42.2 53.2 46.0 53.1 47.2",
  "M45.9 40.0C45.7 41.0 45.8 43.9 45.2 45.9C44.5 47.8 43.5 50.0 42.1 51.8C40.7 53.7 38.8 55.5 36.7 56.9C34.5 58.4 30.3 59.8 29.1 60.3",
  "M32.0 48.0C31.1 48.3 28.7 49.5 26.8 49.7C24.9 50.0 22.7 50.0 20.6 49.6C18.5 49.2 16.2 48.5 14.2 47.3C12.1 46.1 9.2 43.4 8.3 42.6",
  "M18.1 40.0C17.4 39.4 14.8 38.0 13.4 36.5C12.0 34.9 10.7 33.0 9.8 30.9C8.9 28.7 8.2 26.2 8.1 23.6C7.9 21.0 8.8 16.7 8.9 15.3",
  "M18.1 24.0C18.3 23.1 18.5 20.4 19.3 18.6C20.0 16.8 21.0 14.9 22.4 13.3C23.9 11.7 25.6 10.1 27.7 8.9C29.7 7.7 33.5 6.6 34.7 6.1",
] as const;

interface Geometry {
  readonly arms: readonly string[];
  readonly armWidth: number;
  /** Outer radius of the ring (centred at 32, 32). */
  readonly ring: number;
  /** The hole ("eye"), a little off centre like the real nebula. */
  readonly holeX: number;
  readonly holeY: number;
  readonly hole: number;
  readonly pupil: number;
}

const FULL: Geometry = {
  armWidth: 3.6,
  arms: ARMS_FULL,
  hole: 6.3,
  holeX: 33,
  holeY: 31,
  pupil: 1.9,
  ring: 12,
};

const COMPACT: Geometry = {
  armWidth: 5,
  arms: ARMS_COMPACT,
  hole: 6.2,
  holeX: 33.2,
  holeY: 30.8,
  pupil: 2.1,
  ring: 12.5,
};

/** Below this size the compact geometry is used. */
const FULL_FROM = 48;

const circlePath = (cx: number, cy: number, r: number): string =>
  `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${r * 2} 0a${r} ${r} 0 1 0 ${-r * 2} 0Z`;

/** Ring with the eye cut out (even-odd). */
const ringPath = (g: Geometry): string =>
  `${circlePath(32, 32, g.ring)} ${circlePath(g.holeX, g.holeY, g.hole)}`;

/** React ids contain characters that are awkward inside `url(#…)`. */
const useSvgId = (): string => useId().replaceAll(/[^a-zA-Z0-9_-]/gu, "");

// The inside of the eye: the night sky of the nebula and its white star.
// Fixed: the same on light and dark backgrounds.
const EYE = "#0E1240";
const PUPIL = "#9BEBFF";

// Galaxy palette for marks on a known dark background (app icon, official
// avatar): indigo → violet → cyan, one colour per arm.
const ON_DARK_ARMS = [
  "#6E63FF",
  "#886BFF",
  "#A274FF",
  "#83A0FA",
  "#69BAF7",
  "#4FD3F5",
] as const;

const armMix = (a: string, b: string, aShare: number) =>
  `color-mix(in oklab, ${a} ${aShare}%, ${b})`;

const styles = stylex.create({
  arm0: { stroke: colors.n1 },
  arm1: { stroke: armMix(colors.n1, colors.n2, 50) },
  arm2: { stroke: colors.n2 },
  arm3: { stroke: armMix(colors.n2, colors.n3, 66) },
  arm4: { stroke: armMix(colors.n2, colors.n3, 33) },
  arm5: { stroke: colors.n3 },
  flex: { display: "block", flexShrink: 0 },
  ink: { color: colors.fg },
  stopN1: { stopColor: colors.n1 },
  stopN3: { stopColor: colors.n3 },
});

const BRAND_ARMS = [
  styles.arm0,
  styles.arm1,
  styles.arm2,
  styles.arm3,
  styles.arm4,
  styles.arm5,
] as const;

/** Which of the six arm colours arm `index` of `count` gets. */
const armColor = (index: number, count: number): number =>
  Math.round((index * 5) / Math.max(count - 1, 1));

/** "pupil": the central star (helixnebula and Nebula Mail). "none": no pupil. */
export type MarkCore = "pupil" | "none";
/**
 * "ink" (the app's default) is the theme's text colour, "brand" follows the
 * theme in galaxy colours, "onDark" is galaxy fixed for a known dark
 * background, "mono" uses currentColor. Inside the app the mark is ink; the
 * galaxy colours are for the app icon and the favicon.
 */
export type MarkTone = "ink" | "brand" | "onDark" | "mono";

export interface MarkProps {
  /** Pixel size (square). */
  readonly size?: number;
  readonly core?: MarkCore;
  readonly tone?: MarkTone;
  /** Accessible name. `null` makes the mark decorative (aria-hidden). */
  readonly label?: string | null;
  /** Position when the mark is placed inside another SVG. */
  readonly x?: number;
  readonly y?: number;
}

/** The swirl, as SVG children of a 64-unit viewBox (no wrapper). */
const MarkShapes = ({
  id,
  core,
  tone,
  geometry,
}: {
  readonly id: string;
  readonly core: MarkCore;
  readonly tone: MarkTone;
  readonly geometry: Geometry;
}) => {
  const mono = tone === "mono" || tone === "ink";
  const count = geometry.arms.length;
  const armProps = (index: number) => {
    const color = armColor(index, count);
    if (mono) {
      return { stroke: "currentColor" };
    }
    if (tone === "onDark") {
      return { stroke: ON_DARK_ARMS[color] };
    }
    return stylex.props(BRAND_ARMS[color] ?? styles.arm0);
  };
  const stop = (first: boolean) => {
    if (tone === "onDark") {
      return { stopColor: first ? ON_DARK_ARMS[0] : ON_DARK_ARMS[5] };
    }
    return stylex.props(first ? styles.stopN1 : styles.stopN3);
  };
  return (
    <>
      {mono ? null : (
        <defs>
          <linearGradient
            gradientUnits="userSpaceOnUse"
            id={`${id}g`}
            x1="18"
            x2="46"
            y1="46"
            y2="18"
          >
            <stop offset="0" {...stop(true)} />
            <stop offset="1" {...stop(false)} />
          </linearGradient>
        </defs>
      )}
      <g
        fill="none"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={geometry.armWidth}
      >
        {geometry.arms.map((d, index) => (
          <path d={d} key={d} {...armProps(index)} />
        ))}
      </g>
      <path
        d={ringPath(geometry)}
        fill={mono ? "currentColor" : `url(#${id}g)`}
        fillRule="evenodd"
      />
      {mono ? null : (
        <circle
          cx={geometry.holeX}
          cy={geometry.holeY}
          fill={EYE}
          r={geometry.hole}
        />
      )}
      {core === "pupil" ? (
        <circle
          cx={geometry.holeX + geometry.hole * 0.45}
          cy={geometry.holeY - geometry.hole * 0.45}
          fill={mono ? "currentColor" : PUPIL}
          r={geometry.pupil}
        />
      ) : null}
    </>
  );
};

const a11y = (label: string | null) =>
  label === null
    ? ({ "aria-hidden": true, focusable: false } as const)
    : ({ "aria-label": label, role: "img" } as const);

/** The helixnebula / Nebula Mail mark: the swirl around the eye. */
export const Mark = ({
  size = 32,
  core = "pupil",
  tone = "ink",
  label = "Nebula Mail",
  x,
  y,
}: MarkProps) => {
  const id = useSvgId();
  return (
    <svg
      height={size}
      viewBox="0 0 64 64"
      width={size}
      x={x}
      y={y}
      {...a11y(label)}
      {...stylex.props(styles.flex, tone === "ink" ? styles.ink : null)}
    >
      <MarkShapes
        core={core}
        geometry={size >= FULL_FROM ? FULL : COMPACT}
        id={id}
        tone={tone}
      />
    </svg>
  );
};

export interface AppIconProps {
  readonly size?: number;
  /** White tile instead of the night tile. */
  readonly light?: boolean;
  readonly label?: string | null;
}

/** App icon: rounded night tile with a soft indigo bloom and the swirl. */
export const AppIcon = ({
  size = 64,
  light = false,
  label = "Nebula Mail",
}: AppIconProps) => {
  const id = useSvgId();
  return (
    <svg
      height={size}
      viewBox="0 0 64 64"
      width={size}
      {...a11y(label)}
      {...stylex.props(styles.flex)}
    >
      <defs>
        <linearGradient
          gradientUnits="userSpaceOnUse"
          id={`${id}b`}
          x1="0"
          x2="0"
          y1="0"
          y2="64"
        >
          <stop offset="0" stopColor="#1B1B3A" />
          <stop offset="1" stopColor="#0B0C1A" />
        </linearGradient>
        <radialGradient cx="0.5" cy="0.5" id={`${id}h`} r="0.6">
          <stop offset="0" stopColor="#6E63FF" stopOpacity=".34" />
          <stop offset=".55" stopColor="#3B2FD9" stopOpacity=".1" />
          <stop offset="1" stopColor="#3B2FD9" stopOpacity="0" />
        </radialGradient>
      </defs>
      {light ? (
        <>
          <rect fill="#FFFFFF" height="64" rx="16" width="64" />
          <rect
            fill="none"
            height="63"
            rx="15.5"
            stroke="#E4E3EB"
            width="63"
            x=".5"
            y=".5"
          />
        </>
      ) : (
        <>
          <rect fill={`url(#${id}b)`} height="64" rx="16" width="64" />
          <rect fill={`url(#${id}h)`} height="64" rx="16" width="64" />
          <rect
            fill="none"
            height="63"
            rx="15.5"
            stroke="#fff"
            strokeOpacity=".08"
            width="63"
            x=".5"
            y=".5"
          />
        </>
      )}
      <g transform="translate(5 5) scale(.84375)">
        <MarkShapes
          core="pupil"
          geometry={size * 0.84 >= FULL_FROM ? FULL : COMPACT}
          id={`${id}m`}
          tone={light ? "brand" : "onDark"}
        />
      </g>
    </svg>
  );
};

const wordmarkStyles = stylex.create({
  lg: { fontSize: "28px" },
  md: { fontSize: "17px" },
  name: { color: colors.fg, fontWeight: font.weightHeavy },
  product: { color: colors.fg2, fontWeight: 450 },
  root: {
    alignItems: "baseline",
    columnGap: "0.22em",
    display: "inline-flex",
    fontFamily: font.sans,
    letterSpacing: font.trackingBrand,
    lineHeight: 1,
    whiteSpace: "nowrap",
  },
  sm: { fontSize: "14px" },
  xl: { fontSize: "40px" },
});

export interface WordmarkProps {
  /** Lowercase product name after "nebula". */
  readonly product?: string;
  readonly size?: "sm" | "md" | "lg" | "xl";
}

/** "**nebula** mail": lowercase, heavy + regular, tight tracking. */
export const Wordmark = ({ product = "mail", size = "md" }: WordmarkProps) => (
  <span {...stylex.props(wordmarkStyles.root, wordmarkStyles[size])}>
    <b {...stylex.props(wordmarkStyles.name)}>nebula</b>{" "}
    <span {...stylex.props(wordmarkStyles.product)}>{product}</span>
  </span>
);

export interface SparkProps {
  readonly size?: number;
  /** Adds the small companion spark (the compose glyph). */
  readonly double?: boolean;
  /** Accessible name; decorative by default. */
  readonly label?: string | null;
}

/** The four-point star. Uses currentColor. */
export const Spark = ({
  size = 16,
  double = false,
  label = null,
}: SparkProps) => (
  <svg
    fill="currentColor"
    height={size}
    viewBox="0 0 24 24"
    width={size}
    {...a11y(label)}
    {...stylex.props(styles.flex)}
  >
    <path d="M12 3c.4 4.6 2.4 6.6 7 7-4.6.4-6.6 2.4-7 7-.4-4.6-2.4-6.6-7-7 4.6-.4 6.6-2.4 7-7z" />
    {double ? (
      <path d="M19 15.5c.15 1.6.9 2.35 2.5 2.5-1.6.15-2.35.9-2.5 2.5-.15-1.6-.9-2.35-2.5-2.5 1.6-.15 2.35-.9 2.5-2.5z" />
    ) : null}
  </svg>
);

const lineStyles = stylex.create({
  root: {
    backgroundColor: colors.acc,
    borderRadius: "999px",
    display: "block",
    height: "2px",
    width: "100%",
  },
});

/** The active tab underline: flat accent. Fills its width. */
export const NebulaLine = () => (
  <span aria-hidden {...stylex.props(lineStyles.root)} />
);

const shieldStyles = stylex.create({
  outline: {
    fill: `color-mix(in srgb, ${colors.acc} 12%, transparent)`,
    stroke: colors.accText,
    strokeLinejoin: "round",
    strokeWidth: 1.8,
  },
  spark: { fill: colors.core },
  // In the text colour, like the icons around it (nothing to report).
  quietOutline: { fill: "none", stroke: "currentColor" },
  quietSpark: { fill: "currentColor" },
});

/** Shield with the star inside: the privacy glyph. Decorative. */
export const ShieldGlyph = ({
  quiet = false,
  size = 20,
}: {
  readonly quiet?: boolean;
  readonly size?: number;
}) => (
  <svg
    aria-hidden
    focusable={false}
    height={size}
    viewBox="0 0 24 24"
    width={size}
    {...stylex.props(styles.flex)}
  >
    <path
      d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"
      {...stylex.props(
        shieldStyles.outline,
        quiet ? shieldStyles.quietOutline : null
      )}
    />
    <path
      d="M12 7.2c.3 2.6 1.4 3.7 4 4-2.6.3-3.7 1.4-4 4-.3-2.6-1.4-3.7-4-4 2.6-.3 3.7-1.4 4-4z"
      {...stylex.props(
        shieldStyles.spark,
        quiet ? shieldStyles.quietSpark : null
      )}
    />
  </svg>
);
