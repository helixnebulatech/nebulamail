// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A light emoji picker for the compose toolbar: a short,
// fixed list of common emoji in a grid, no search and no dependency. The
// system's own picker (⌃⌘Space, Win+.) covers the rest. Arrow keys move
// in the grid (one tab stop), Enter or a click inserts, Esc closes.

import { Popover as BasePopover } from "@base-ui/react/popover";
import * as stylex from "@stylexjs/stylex";
import { useRef, useState } from "react";
import type { KeyboardEvent, ReactElement } from "react";

import { cls, shared, usePortalContainer } from "./internal";
import { colors, font, motion, radius, shadow, zIndex } from "./tokens.stylex";
import { Tooltip } from "./tooltip";

/** The emoji offered, in grid order (8 per row). */
export const EMOJI: readonly string[] = [
  "😀",
  "😃",
  "😄",
  "😁",
  "😆",
  "😅",
  "😂",
  "🙂",
  "😉",
  "😊",
  "😇",
  "😍",
  "😘",
  "😋",
  "😎",
  "🤔",
  "😐",
  "😏",
  "😴",
  "😮",
  "😢",
  "😭",
  "😡",
  "🥳",
  "🙏",
  "👍",
  "👎",
  "👏",
  "🙌",
  "👋",
  "🤝",
  "💪",
  "👀",
  "❤️",
  "💔",
  "✨",
  "🔥",
  "🎉",
  "🎂",
  "🎁",
  "☕",
  "🍕",
  "🍻",
  "✅",
  "❌",
  "⚠️",
  "❓",
  "❗",
  "💡",
  "📌",
  "📎",
  "📅",
  "⏰",
  "📞",
  "✉️",
  "🚀",
  "⭐",
  "☀️",
  "🌧️",
  "🌈",
  "🐶",
  "🐱",
  "🌹",
  "🌍",
];

const COLUMNS = 8;

const styles = stylex.create({
  positioner: { outline: "none", zIndex: zIndex.popover },
  popup: {
    backgroundColor: colors.raised,
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh3,
    boxSizing: "border-box",
    outline: "none",
    padding: "6px",
    transformOrigin: "var(--transform-origin)",
    transitionDuration: motion.fast,
    transitionProperty: "opacity, transform",
    transitionTimingFunction: motion.easing,
  },
  hidden: { opacity: 0, transform: "scale(0.96)" },
  grid: {
    borderStyle: "none",
    display: "grid",
    margin: 0,
    minWidth: 0,
    padding: 0,
    gap: "2px",
    gridTemplateColumns: `repeat(${COLUMNS}, 32px)`,
  },
  emoji: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.sm,
    cursor: "pointer",
    display: "inline-flex",
    // Colour emoji fonts, not the app's.
    fontFamily: `"Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji", ${font.sans}`,
    fontSize: "20px",
    height: "32px",
    justifyContent: "center",
    lineHeight: 1,
    width: "32px",
  },
});

export interface EmojiPickerProps {
  /** The toolbar button that opens it. */
  readonly trigger: ReactElement;
  /** Its name (tooltip and the grid's label). */
  readonly label: string;
  readonly onPick: (emoji: string) => void;
}

/** Where an arrow key moves from `index` in the grid (or null: stays). */
const moveFrom = (index: number, key: string): number | null => {
  const last = EMOJI.length - 1;
  switch (key) {
    case "ArrowRight": {
      return Math.min(last, index + 1);
    }
    case "ArrowLeft": {
      return Math.max(0, index - 1);
    }
    case "ArrowDown": {
      return index + COLUMNS <= last ? index + COLUMNS : index;
    }
    case "ArrowUp": {
      return index - COLUMNS >= 0 ? index - COLUMNS : index;
    }
    case "Home": {
      return 0;
    }
    case "End": {
      return last;
    }
    default: {
      return null;
    }
  }
};

/** Right and left swap in right-to-left pages. */
const visualKey = (key: string, rtl: boolean): string => {
  if (!rtl) {
    return key;
  }
  if (key === "ArrowRight") {
    return "ArrowLeft";
  }
  return key === "ArrowLeft" ? "ArrowRight" : key;
};

export const EmojiPicker = ({ label, onPick, trigger }: EmojiPickerProps) => {
  const container = usePortalContainer();
  const [open, setOpen] = useState(false);
  // The grid's single tab stop (roving tabindex).
  const [active, setActive] = useState(0);
  const grid = useRef<HTMLFieldSetElement>(null);
  // Closed by picking: the editor takes the focus (onPick), not the trigger.
  const picked = useRef(false);

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>): void => {
    const rtl =
      grid.current !== null &&
      getComputedStyle(grid.current).direction === "rtl";
    const next = moveFrom(active, visualKey(event.key, rtl));
    if (next === null) {
      return;
    }
    event.preventDefault();
    setActive(next);
    grid.current?.querySelectorAll("button")[next]?.focus();
  };

  return (
    <BasePopover.Root
      onOpenChange={(next) => {
        if (next) {
          picked.current = false;
        }
        setOpen(next);
      }}
      open={open}
    >
      <Tooltip content={label}>
        <BasePopover.Trigger render={trigger} />
      </Tooltip>
      <BasePopover.Portal container={container}>
        <BasePopover.Positioner
          align="start"
          className={cls(styles.positioner)}
          sideOffset={6}
        >
          <BasePopover.Popup
            className={(state) =>
              cls(
                styles.popup,
                state.transitionStatus === "starting" ||
                  state.transitionStatus === "ending"
                  ? styles.hidden
                  : null
              )
            }
            finalFocus={() => !picked.current}
            initialFocus={() =>
              grid.current?.querySelectorAll("button")[active] ?? true
            }
          >
            <fieldset
              aria-label={label}
              ref={grid}
              {...stylex.props(styles.grid)}
            >
              {EMOJI.map((emoji, index) => (
                <button
                  // Screen readers read the emoji's own name.
                  aria-label={emoji}
                  key={emoji}
                  onKeyDown={onKeyDown}
                  onClick={() => {
                    setActive(index);
                    picked.current = true;
                    setOpen(false);
                    onPick(emoji);
                  }}
                  tabIndex={index === active ? 0 : -1}
                  type="button"
                  {...stylex.props(
                    shared.resetButton,
                    styles.emoji,
                    shared.focusRing
                  )}
                >
                  {emoji}
                </button>
              ))}
            </fieldset>
          </BasePopover.Popup>
        </BasePopover.Positioner>
      </BasePopover.Portal>
    </BasePopover.Root>
  );
};
