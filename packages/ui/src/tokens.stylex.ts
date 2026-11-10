// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";

// Helix design tokens. Colour pairs meet WCAG contrast. Defaults are the
// light theme; see themes.ts.

/** Semantic colours (ink interface with an indigo accent, galaxy brand colours, stardust amber). Light values; themes.ts overrides them. */
export const colors = stylex.defineVars({
  /** Main surface (panels, inputs). */
  bg: "#FFFFFF",
  /** App background behind floating panels. */
  canvas: "#F6F6F9",
  /** Sidebar background. */
  side: "#F4F3F7",
  /** Menus, popovers, dialogs, composer. */
  raised: "#FFFFFF",
  /** Hover fill; also neutral chips and keys. */
  hover: "#EEEDF3",
  /** Hairline borders and separators. */
  line: "#E4E3EB",
  /** Stronger borders (inputs, secondary buttons). */
  line2: "#D3D2DD",
  /** Primary text. */
  fg: "#15152B",
  /** Secondary text. */
  fg2: "#46475E",
  /** Tertiary text: times, previews, placeholders. */
  fg3: "#62637A",
  /** Accent indigo: primary buttons, compose, checked controls. */
  acc: "#4F46E5",
  /** Accent hover. */
  accHi: "#4338CA",
  /** Text on acc. */
  accFg: "#FFFFFF",
  /** Accent-coloured text and links. */
  accText: "#4338CA",
  /** Soft accent fill (Oficial badge, active filter). */
  accSoft: "#EEEEFD",
  /** Selected row. */
  sel: "#E3E2FA",
  /** Text on sel. */
  selFg: "#15152B",
  /** Stardust amber: stars, sparks. */
  star: "#F29A1F",
  /** Soft amber fill (VIP). */
  starSoft: "#FFF3E0",
  /** Amber text on starSoft. */
  starText: "#8F4F00",
  /** Night ink: toasts, tooltips, ink button. */
  ink: "#15152B",
  /** Text on ink. */
  inkFg: "#FFFFFF",
  /** Success / verified. */
  ok: "#0E7A4E",
  /** Success fill. */
  okSoft: "#E3F5EC",
  /** Warning. */
  warn: "#8F4F00",
  /** Warning fill. */
  warnSoft: "#FFF1DD",
  /** Danger (orange-red, far from the accent). */
  danger: "#C2300A",
  /** Text on danger. */
  dangerFg: "#FFFFFF",
  /** Danger fill. */
  dangerSoft: "#FDEBE5",
  /** Information. */
  info: "#2353C9",
  /** Information fill. */
  infoSoft: "#E8EEFC",
  /** Email body surface. */
  paper: "#FFFFFF",
  /** Email body text. */
  paperFg: "#1F2033",
  /** Galaxy start (indigo): the logo's arms. */
  n1: "#3B2FD9",
  /** Galaxy middle (violet). */
  n2: "#7A3CF0",
  /** Galaxy end (cyan). */
  n3: "#1FB6E6",
  /** Stars of the illustrations. */
  core: "#1A9FD0",
  /** Active nav item / segmented option fill. */
  activeFill: "#FFFFFF",
  /** Quiet inset fill (collapsed messages, info lines). */
  subtle: "#F6F6F9",
  /** Focus outline. */
  focus: "#4F46E5",
  /** Dialog backdrop. */
  scrim: "rgba(21, 21, 43, 0.28)",
  /** Translucent sticky bars. */
  glass: "rgba(255, 255, 255, 0.82)",
  /** Alias of fg3 (older name, still used by app routes). */
  fgMuted: "#62637A",
  /**
   * Boundary of interactive controls: unchecked checkbox border, switch
   * track when off. At least 3:1 against bg, side, canvas, hover and sel in
   * both themes (WCAG 1.4.11); line2 is only for decorative borders.
   */
  control: "#85869D",
});

/** Elevation. Light uses soft shadows; dark uses deep shadows plus a hairline ring. `halo` is brand-only (compose, palette, primary hover). */
export const shadow = stylex.defineVars({
  sh1: "0 1px 2px rgba(21, 21, 43, 0.06), 0 1px 1px rgba(21, 21, 43, 0.04)",
  sh2: "0 4px 14px -4px rgba(21, 21, 43, 0.12), 0 2px 4px rgba(21, 21, 43, 0.05)",
  sh3: "0 24px 60px -16px rgba(21, 21, 43, 0.28), 0 8px 20px -8px rgba(21, 21, 43, 0.12)",
  halo: "0 0 0 1px rgba(79, 70, 229, 0.1), 0 10px 30px -10px rgba(79, 70, 229, 0.45)",
  primary:
    "inset 0 1px 0 rgba(255, 255, 255, 0.2), 0 1px 2px rgba(79, 70, 229, 0.3)",
  focusHalo: "0 0 0 4px rgba(79, 70, 229, 0.18)",
  /**
   * The chosen option of a segmented control: a 1px ring that keeps 3:1
   * against the track (WCAG 1.4.11), where the fill alone can't in dark.
   */
  active: "0 0 0 1px #C4C3D3, 0 1px 2px rgba(21, 21, 43, 0.06)",
});

/**
 * OKLCH lightness and chroma ("L C") for generated avatar and label colours;
 * the hue comes from the data. OKLCH keeps every hue equally light, so
 * yellows don't turn olive in dark mode. Text on fill is at least 6.4:1 for
 * every hue in both themes.
 */
export const tint = stylex.defineVars({
  avatarBg: "0.93 0.05",
  avatarFg: "0.42 0.1",
  labelBg: "0.94 0.045",
  labelFg: "0.42 0.11",
});

/** Spacing scale (4 px grid, plus 2/6 px for tight chips). */
export const space = stylex.defineVars({
  xxs: "2px",
  xs: "4px",
  s6: "6px",
  sm: "8px",
  s10: "10px",
  md: "12px",
  s14: "14px",
  lg: "16px",
  s20: "20px",
  xl: "24px",
  xxl: "32px",
  xxxl: "48px",
});

/** Radii: rounded with hierarchy. 8 controls / 12 cards and menus / 16 dialogs and composer / pill. */
export const radius = stylex.defineVars({
  /** Keys, checkboxes. */
  xs: "5px",
  sm: "6px",
  /** Buttons, inputs, nav items. */
  md: "8px",
  /** Buttons (normal size), fields. */
  control: "9px",
  /** Rows, cards, menus, toasts. */
  lg: "12px",
  /** Panels, dialogs, composer, palette. */
  xl: "16px",
  /** Filters, labels, badges, switches. */
  pill: "999px",
});

/**
 * Typography: one family, Geist Sans (OFL), self-hosted (fonts.css in
 * this package). Until it loads: an installed Geist, else
 * "Geist Variable fallback", a system font resized to Geist's metrics
 * (generated by fontaine, apps/web/vite.config.ts) so the text doesn't
 * move when Geist arrives.
 */
export const font = stylex.defineVars({
  sans: '"Geist Variable", "Geist", "Geist Variable fallback", ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif',
  mono: 'ui-monospace, "SF Mono", "Cascadia Mono", Menlo, Consolas, monospace',
  /** 11: keys, tiny badges. */
  sizeXs: "11px",
  /** 12: meta (times, counts, captions). */
  sizeSm: "12px",
  /** 13.5: interface (default). */
  sizeMd: "13.5px",
  /** 14.5: reading (message bodies, dialogs). */
  sizeLg: "14.5px",
  /** 18: section titles. */
  sizeXl: "18px",
  /** 22: open subject. */
  sizeXxl: "22px",
  /** 28: page title. */
  sizeTitle: "28px",
  /** 34: hero / empty-state display. */
  sizeDisplay: "34px",
  weightNormal: "400",
  weightMedium: "500",
  /** 550: buttons. */
  weightButton: "550",
  weightSemibold: "600",
  weightBold: "700",
  /** 750: the wordmark. */
  weightHeavy: "750",
  lineHeight: "1.5",
  lineHeightTight: "1.2",
  lineHeightReading: "1.62",
  trackingTight: "-0.01em",
  trackingTitle: "-0.03em",
  trackingBrand: "-0.045em",
});

const REDUCED = "@media (prefers-reduced-motion: reduce)";

/**
 * Motion. Durations collapse to ~0 with prefers-reduced-motion, so every
 * transition built on these tokens respects it automatically. Looping
 * animations (Spinner) also check the media query themselves.
 */
export const motion = stylex.defineVars({
  fast: { default: "120ms", [REDUCED]: "0.01ms" },
  normal: { default: "180ms", [REDUCED]: "0.01ms" },
  slow: { default: "300ms", [REDUCED]: "0.01ms" },
  /** The one-time privacy glow (never loops). */
  pulse: { default: "1200ms", [REDUCED]: "0.01ms" },
  /** Standard: quick out, soft landing. */
  easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
  easingIn: "cubic-bezier(0.5, 0, 0.75, 0)",
  /** Small overshoot for pops (shield, switch thumb). */
  easingSpring: "cubic-bezier(0.3, 1.6, 0.5, 1)",
});

/** Stacking order. */
export const zIndex = stylex.defineVars({
  base: "0",
  raised: "1",
  sticky: "10",
  popover: "50",
  overlay: "60",
  dialog: "70",
  /** A select's list: it opens from forms, also inside dialogs. */
  picker: "75",
  toast: "80",
  tooltip: "90",
});

/** Density-dependent sizes. Compact is the default; see themes.ts for comfortable. */
export const density = stylex.defineVars({
  /** Sidebar nav rows. */
  navItem: "32px",
  /** Single-line message rows. */
  row: "46px",
  /** Text inputs. */
  input: "38px",
  /** Split-inbox tabs. */
  tab: "38px",
  /** Menu items. */
  menuItem: "34px",
});

/**
 * Touch screens. `coarse`: the main pointer is a finger, so controls get
 * `target`-sized hit areas (44 px: WCAG 2.5.5, Apple HIG). `noHover`: no
 * mouse, so keyboard hints and hover-only UI are hidden.
 */
export const touch = stylex.defineConsts({
  coarse: "@media (pointer: coarse)",
  noHover: "@media (hover: none)",
  target: "44px",
});
