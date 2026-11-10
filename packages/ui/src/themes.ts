// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";

import { colors, density, font, shadow, tint, touch } from "./tokens.stylex";

// Themes. The interface is "ink" (near black and white, indigo accent) and
// the brand colours n1–n3 are the galaxy of the mark.
//
// Three modes: "light", "dark" and "system". "system" is a single theme whose
// values switch with @media (prefers-color-scheme: dark), so the server can
// render the right class from the preferences cookie with no flash and no
// script. High contrast: TODO, will be a fourth mode.

const DARK = "@media (prefers-color-scheme: dark)";

const lightColors = stylex.createTheme(colors, {
  bg: "#FFFFFF",
  canvas: "#F6F6F9",
  side: "#F4F3F7",
  raised: "#FFFFFF",
  hover: "#EEEDF3",
  line: "#E4E3EB",
  line2: "#D3D2DD",
  fg: "#15152B",
  fg2: "#46475E",
  fg3: "#62637A",
  acc: "#4F46E5",
  accHi: "#4338CA",
  accFg: "#FFFFFF",
  accText: "#4338CA",
  accSoft: "#EEEEFD",
  sel: "#E3E2FA",
  selFg: "#15152B",
  star: "#F29A1F",
  starSoft: "#FFF3E0",
  starText: "#8F4F00",
  ink: "#15152B",
  inkFg: "#FFFFFF",
  ok: "#0E7A4E",
  okSoft: "#E3F5EC",
  warn: "#8F4F00",
  warnSoft: "#FFF1DD",
  danger: "#C2300A",
  dangerFg: "#FFFFFF",
  dangerSoft: "#FDEBE5",
  info: "#2353C9",
  infoSoft: "#E8EEFC",
  paper: "#FFFFFF",
  paperFg: "#1F2033",
  n1: "#3B2FD9",
  n2: "#7A3CF0",
  n3: "#1FB6E6",
  core: "#1A9FD0",
  activeFill: "#FFFFFF",
  subtle: "#F6F6F9",
  focus: "#4F46E5",
  scrim: "rgba(21, 21, 43, 0.28)",
  glass: "rgba(255, 255, 255, 0.82)",
  fgMuted: "#62637A",
  control: "#85869D",
});

const darkColors = stylex.createTheme(colors, {
  bg: "#10111D",
  canvas: "#06070C",
  side: "#07080E",
  raised: "#1B1D31",
  hover: "#181A2B",
  line: "#2A2D45",
  line2: "#323550",
  fg: "#ECECF5",
  fg2: "#B6B7CC",
  fg3: "#9091AB",
  acc: "#5B54E8",
  accHi: "#4F46E5",
  accFg: "#FFFFFF",
  accText: "#A5A0FF",
  accSoft: "#1C1B3D",
  sel: "#27265A",
  selFg: "#EEEDFB",
  star: "#FFB547",
  starSoft: "#2B2010",
  starText: "#FFC56B",
  ink: "#ECECF5",
  inkFg: "#15152B",
  ok: "#44D39A",
  okSoft: "#0F2A20",
  warn: "#FFC56B",
  warnSoft: "#2B2010",
  danger: "#FF7A59",
  dangerFg: "#1A0A05",
  dangerSoft: "#2E140C",
  info: "#8CB0FF",
  infoSoft: "#141E38",
  paper: "#14152A",
  paperFg: "#E2E2EE",
  n1: "#6E63FF",
  n2: "#A274FF",
  n3: "#4FD3F5",
  core: "#7FE3FA",
  activeFill: "#2A2D45",
  subtle: "#171828",
  focus: "#8B85FF",
  scrim: "rgba(5, 6, 14, 0.6)",
  glass: "rgba(23, 24, 40, 0.86)",
  fgMuted: "#9091AB",
  control: "#6B6F92",
});

const systemColors = stylex.createTheme(colors, {
  bg: { default: "#FFFFFF", [DARK]: "#10111D" },
  canvas: { default: "#F6F6F9", [DARK]: "#06070C" },
  side: { default: "#F4F3F7", [DARK]: "#07080E" },
  raised: { default: "#FFFFFF", [DARK]: "#1B1D31" },
  hover: { default: "#EEEDF3", [DARK]: "#181A2B" },
  line: { default: "#E4E3EB", [DARK]: "#2A2D45" },
  line2: { default: "#D3D2DD", [DARK]: "#323550" },
  fg: { default: "#15152B", [DARK]: "#ECECF5" },
  fg2: { default: "#46475E", [DARK]: "#B6B7CC" },
  fg3: { default: "#62637A", [DARK]: "#9091AB" },
  acc: { default: "#4F46E5", [DARK]: "#5B54E8" },
  accHi: { default: "#4338CA", [DARK]: "#4F46E5" },
  accFg: { default: "#FFFFFF", [DARK]: "#FFFFFF" },
  accText: { default: "#4338CA", [DARK]: "#A5A0FF" },
  accSoft: { default: "#EEEEFD", [DARK]: "#1C1B3D" },
  sel: { default: "#E3E2FA", [DARK]: "#27265A" },
  selFg: { default: "#15152B", [DARK]: "#EEEDFB" },
  star: { default: "#F29A1F", [DARK]: "#FFB547" },
  starSoft: { default: "#FFF3E0", [DARK]: "#2B2010" },
  starText: { default: "#8F4F00", [DARK]: "#FFC56B" },
  ink: { default: "#15152B", [DARK]: "#ECECF5" },
  inkFg: { default: "#FFFFFF", [DARK]: "#15152B" },
  ok: { default: "#0E7A4E", [DARK]: "#44D39A" },
  okSoft: { default: "#E3F5EC", [DARK]: "#0F2A20" },
  warn: { default: "#8F4F00", [DARK]: "#FFC56B" },
  warnSoft: { default: "#FFF1DD", [DARK]: "#2B2010" },
  danger: { default: "#C2300A", [DARK]: "#FF7A59" },
  dangerFg: { default: "#FFFFFF", [DARK]: "#1A0A05" },
  dangerSoft: { default: "#FDEBE5", [DARK]: "#2E140C" },
  info: { default: "#2353C9", [DARK]: "#8CB0FF" },
  infoSoft: { default: "#E8EEFC", [DARK]: "#141E38" },
  paper: { default: "#FFFFFF", [DARK]: "#14152A" },
  paperFg: { default: "#1F2033", [DARK]: "#E2E2EE" },
  n1: { default: "#3B2FD9", [DARK]: "#6E63FF" },
  n2: { default: "#7A3CF0", [DARK]: "#A274FF" },
  n3: { default: "#1FB6E6", [DARK]: "#4FD3F5" },
  core: { default: "#1A9FD0", [DARK]: "#7FE3FA" },
  activeFill: { default: "#FFFFFF", [DARK]: "#2A2D45" },
  subtle: { default: "#F6F6F9", [DARK]: "#171828" },
  focus: { default: "#4F46E5", [DARK]: "#8B85FF" },
  scrim: { default: "rgba(21, 21, 43, 0.28)", [DARK]: "rgba(5, 6, 14, 0.6)" },
  glass: {
    default: "rgba(255, 255, 255, 0.82)",
    [DARK]: "rgba(23, 24, 40, 0.86)",
  },
  fgMuted: { default: "#62637A", [DARK]: "#9091AB" },
  control: { default: "#85869D", [DARK]: "#6B6F92" },
});

const lightShadow = stylex.createTheme(shadow, {
  sh1: "0 1px 2px rgba(21, 21, 43, 0.06), 0 1px 1px rgba(21, 21, 43, 0.04)",
  sh2: "0 4px 14px -4px rgba(21, 21, 43, 0.12), 0 2px 4px rgba(21, 21, 43, 0.05)",
  sh3: "0 24px 60px -16px rgba(21, 21, 43, 0.28), 0 8px 20px -8px rgba(21, 21, 43, 0.12)",
  halo: "0 0 0 1px rgba(79, 70, 229, 0.1), 0 10px 30px -10px rgba(79, 70, 229, 0.45)",
  primary:
    "inset 0 1px 0 rgba(255, 255, 255, 0.2), 0 1px 2px rgba(79, 70, 229, 0.3)",
  focusHalo: "0 0 0 4px rgba(79, 70, 229, 0.18)",
  active: "0 0 0 1px #C4C3D3, 0 1px 2px rgba(21, 21, 43, 0.06)",
});

const darkShadow = stylex.createTheme(shadow, {
  sh1: "0 1px 2px rgba(0, 0, 0, 0.4)",
  sh2: "0 6px 18px -6px rgba(0, 0, 0, 0.6), 0 0 0 1px rgba(255, 255, 255, 0.03)",
  sh3: "inset 0 1px 0 rgba(255, 255, 255, 0.04), 0 30px 70px -20px rgba(0, 0, 0, 0.8), 0 0 0 1px rgba(255, 255, 255, 0.05)",
  halo: "0 0 0 1px rgba(139, 133, 255, 0.25), 0 10px 34px -10px rgba(110, 99, 255, 0.5)",
  primary:
    "inset 0 1px 0 rgba(255, 255, 255, 0.2), 0 1px 2px rgba(0, 0, 0, 0.4)",
  focusHalo: "0 0 0 4px rgba(139, 133, 255, 0.3)",
  active: "0 0 0 1px #6B6F92",
});

const systemShadow = stylex.createTheme(shadow, {
  sh1: {
    default:
      "0 1px 2px rgba(21, 21, 43, 0.06), 0 1px 1px rgba(21, 21, 43, 0.04)",
    [DARK]: "0 1px 2px rgba(0, 0, 0, 0.4)",
  },
  sh2: {
    default:
      "0 4px 14px -4px rgba(21, 21, 43, 0.12), 0 2px 4px rgba(21, 21, 43, 0.05)",
    [DARK]:
      "0 6px 18px -6px rgba(0, 0, 0, 0.6), 0 0 0 1px rgba(255, 255, 255, 0.03)",
  },
  sh3: {
    default:
      "0 24px 60px -16px rgba(21, 21, 43, 0.28), 0 8px 20px -8px rgba(21, 21, 43, 0.12)",
    [DARK]:
      "inset 0 1px 0 rgba(255, 255, 255, 0.04), 0 30px 70px -20px rgba(0, 0, 0, 0.8), 0 0 0 1px rgba(255, 255, 255, 0.05)",
  },
  halo: {
    default:
      "0 0 0 1px rgba(79, 70, 229, 0.1), 0 10px 30px -10px rgba(79, 70, 229, 0.45)",
    [DARK]:
      "0 0 0 1px rgba(139, 133, 255, 0.25), 0 10px 34px -10px rgba(110, 99, 255, 0.5)",
  },
  primary: {
    default:
      "inset 0 1px 0 rgba(255, 255, 255, 0.2), 0 1px 2px rgba(79, 70, 229, 0.3)",
    [DARK]:
      "inset 0 1px 0 rgba(255, 255, 255, 0.2), 0 1px 2px rgba(0, 0, 0, 0.4)",
  },
  focusHalo: {
    default: "0 0 0 4px rgba(79, 70, 229, 0.18)",
    [DARK]: "0 0 0 4px rgba(139, 133, 255, 0.3)",
  },
  active: {
    default: "0 0 0 1px #C4C3D3, 0 1px 2px rgba(21, 21, 43, 0.06)",
    [DARK]: "0 0 0 1px #6B6F92",
  },
});

const lightTint = stylex.createTheme(tint, {
  avatarBg: "0.93 0.05",
  avatarFg: "0.42 0.1",
  labelBg: "0.94 0.045",
  labelFg: "0.42 0.11",
});

const darkTint = stylex.createTheme(tint, {
  avatarBg: "0.32 0.06",
  avatarFg: "0.86 0.08",
  labelBg: "0.31 0.06",
  labelFg: "0.86 0.08",
});

const systemTint = stylex.createTheme(tint, {
  avatarBg: { default: "0.93 0.05", [DARK]: "0.32 0.06" },
  avatarFg: { default: "0.42 0.1", [DARK]: "0.86 0.08" },
  labelBg: { default: "0.94 0.045", [DARK]: "0.31 0.06" },
  labelFg: { default: "0.42 0.11", [DARK]: "0.86 0.08" },
});

// On touch screens every control is at least `touch.target` tall, whatever
// the density (rows are already taller).
const comfortable = stylex.createTheme(density, {
  input: { default: "42px", [touch.coarse]: touch.target },
  menuItem: { default: "38px", [touch.coarse]: touch.target },
  navItem: { default: "36px", [touch.coarse]: touch.target },
  row: "58px",
  tab: { default: "42px", [touch.coarse]: touch.target },
});

const compact = stylex.createTheme(density, {
  input: { default: "38px", [touch.coarse]: touch.target },
  menuItem: { default: "34px", [touch.coarse]: touch.target },
  navItem: { default: "32px", [touch.coarse]: touch.target },
  row: "46px",
  tab: { default: "38px", [touch.coarse]: touch.target },
});

const documentStyles = stylex.create({
  dark: { colorScheme: "dark" },
  light: { colorScheme: "light" },
  root: {
    // Scrollbar colours for app.css (custom properties, so plain CSS and
    // the pseudo-elements Safari needs can use the theme). A tint of the
    // text colour: just visible at rest, a little stronger while the
    // pointer is over the area, firmer only while dragged. Never as dark
    // as the text, in either theme.
    "--nb-scrollbar-thumb": `color-mix(in srgb, ${colors.fg} 16%, transparent)`,
    "--nb-scrollbar-thumb-hover": `color-mix(in srgb, ${colors.fg} 30%, transparent)`,
    "--nb-scrollbar-thumb-active": `color-mix(in srgb, ${colors.fg} 42%, transparent)`,
    backgroundColor: colors.canvas,
    color: colors.fg,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
    MozOsxFontSmoothing: "grayscale",
    WebkitFontSmoothing: "antialiased",
  },
  system: { colorScheme: "light dark" },
});

export type ThemeMode = "light" | "dark" | "system";
export type Density = "compact" | "comfortable";

/** Theme bundles (colours, elevation, tints) per mode. */
export const themes = {
  dark: [darkColors, darkShadow, darkTint],
  light: [lightColors, lightShadow, lightTint],
  system: [systemColors, systemShadow, systemTint],
} as const satisfies Record<ThemeMode, unknown>;

export const densities = {
  comfortable,
  compact,
} as const satisfies Record<Density, unknown>;

/** Theme bundles for use with `stylex.props` (e.g. a dark composer). */
export const darkTheme = themes.dark;
export const lightTheme = themes.light;
export const systemTheme = themes.system;

/**
 * StyleX props for a theme root (`<html>` or a ThemeScope): theme variables,
 * density, `color-scheme` and the document base (canvas, text colour, Geist).
 *
 * Server-render it from the preferences cookie: no flash, no script.
 */
export const themeProps = (
  mode: ThemeMode,
  densityMode: Density = "compact"
): ReturnType<typeof stylex.props> =>
  stylex.props(
    themes[mode],
    densities[densityMode],
    documentStyles.root,
    documentStyles[mode]
  );
