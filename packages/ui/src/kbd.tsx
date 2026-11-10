// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

import * as stylex from "@stylexjs/stylex";
import {
  ArrowBigUp,
  ArrowDown,
  ArrowUp,
  Command,
  CornerDownLeft,
  Option,
} from "lucide-react";
import type { ReactNode } from "react";

import { shared } from "./internal";
import { colors, font, radius, touch } from "./tokens.stylex";

export type KbdTone = "default" | "onAccent" | "onInk";

const styles = stylex.create({
  // Keyboard hints mean nothing without a keyboard and mouse.
  group: {
    alignItems: "center",
    columnGap: "2px",
    display: { default: "inline-flex", [touch.noHover]: "none" },
    flexShrink: 0,
  },
  icon: { height: "11px", strokeWidth: 2.2, width: "11px" },
  key: {
    backgroundColor: colors.hover,
    borderBottomWidth: "2px",
    borderColor: colors.line,
    borderInlineWidth: "1px",
    borderStyle: "solid",
    borderRadius: radius.xs,
    borderTopWidth: "1px",
    boxSizing: "border-box",
    color: colors.fg3,
    display: "inline-grid",
    fontFamily: font.sans,
    fontSize: font.sizeXs,
    fontStyle: "normal",
    fontWeight: font.weightMedium,
    height: "18px",
    lineHeight: 1,
    minWidth: "18px",
    paddingInline: "4px",
    placeItems: "center",
  },
  // On the accent fill: a lighter fill and a firmer edge, so the key
  // still reads as a key next to the label.
  onAccent: {
    backgroundColor: "rgba(255, 255, 255, 0.22)",
    borderColor: "rgba(255, 255, 255, 0.32)",
    color: colors.accFg,
  },
  onInk: {
    backgroundColor: `color-mix(in srgb, ${colors.inkFg} 16%, transparent)`,
    borderColor: "transparent",
    color: colors.inkFg,
  },
});

const toneStyles = {
  default: null,
  onAccent: styles.onAccent,
  onInk: styles.onInk,
} as const;

export interface KbdProps {
  readonly children: ReactNode;
  readonly tone?: KbdTone;
}

/** One keyboard key chip. */
export const Kbd = ({ children, tone = "default" }: KbdProps) => (
  <kbd {...stylex.props(styles.key, toneStyles[tone])}>{children}</kbd>
);

// Symbols rendered as icons (Geist has no ⌘/⇧ glyphs), with a hidden name.
const SYMBOLS: Record<string, { icon: typeof Command; name: string }> = {
  "↑": { icon: ArrowUp, name: "Up" },
  "↓": { icon: ArrowDown, name: "Down" },
  "↵": { icon: CornerDownLeft, name: "Enter" },
  "⇧": { icon: ArrowBigUp, name: "Shift" },
  "⌘": { icon: Command, name: "Cmd" },
  "⌥": { icon: Option, name: "Option" },
};

export interface ShortcutProps {
  /** Keys in order, e.g. `["⌘", "K"]` or `["Ctrl", "K"]`. ⌘ ⇧ ⌥ ↵ ↑ ↓ become icons. */
  readonly keys: readonly string[];
  readonly tone?: KbdTone;
}

/** A key combination: one Kbd per key. */
export const Shortcut = ({ keys, tone = "default" }: ShortcutProps) => (
  <span {...stylex.props(styles.group)}>
    {keys.map((key, index) => {
      const symbol = SYMBOLS[key];
      return (
        // Keys can repeat ("G G"), so the position is part of the identity.
        // oxlint-disable-next-line react/no-array-index-key -- static list, never reordered.
        <Kbd key={`${key}-${index}`} tone={tone}>
          {symbol === undefined ? (
            key
          ) : (
            <>
              <symbol.icon aria-hidden {...stylex.props(styles.icon)} />
              <span {...stylex.props(shared.visuallyHidden)}>
                {symbol.name}
              </span>
            </>
          )}
        </Kbd>
      );
    })}
  </span>
);
