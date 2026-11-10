// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// A text input with a list of suggestions under it (Base UI Autocomplete):
// arrow keys move through them, Enter or a click picks one, Escape closes
// the list. The app filters and ranks the suggestions; picking one calls
// `onSelect` and empties the input (e.g. it becomes a recipient chip).

import { Autocomplete } from "@base-ui/react/autocomplete";
import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { ClipboardEvent, FocusEvent, KeyboardEvent } from "react";

import { Avatar } from "./avatar";
import { cls, usePortalContainer } from "./internal";
import { colors, font, motion, radius, shadow, zIndex } from "./tokens.stylex";

const styles = stylex.create({
  description: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  hidden: { opacity: 0, transform: "scale(0.98) translateY(-4px)" },
  highlighted: { backgroundColor: colors.hover },
  item: {
    alignItems: "center",
    borderRadius: radius.md,
    color: colors.fg,
    columnGap: "10px",
    cursor: "pointer",
    display: "flex",
    minHeight: "40px",
    outline: "none",
    paddingBlock: "4px",
    paddingInline: "8px",
    userSelect: "none",
  },
  label: {
    fontSize: font.sizeMd,
    fontWeight: font.weightMedium,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  popup: {
    backgroundColor: colors.raised,
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh3,
    boxSizing: "border-box",
    fontFamily: font.sans,
    maxHeight: "min(320px, var(--available-height))",
    maxWidth: "calc(100vw - 24px)",
    minWidth: "280px",
    outline: "none",
    overflowY: "auto",
    padding: "5px",
    transformOrigin: "var(--transform-origin)",
    transitionDuration: motion.fast,
    transitionProperty: "opacity, transform",
    transitionTimingFunction: motion.easing,
  },
  positioner: { outline: "none", zIndex: zIndex.popover },
  text: { display: "flex", flexDirection: "column", minWidth: 0 },
});

export interface Suggestion {
  /** Unique; also what identifies the pick (e.g. the email address). */
  readonly id: string;
  readonly label: string;
  readonly description?: string;
  /** Key for the avatar colour; no avatar if absent. */
  readonly avatarKey?: string;
}

export interface SuggestInputProps {
  readonly value: string;
  readonly onValueChange: (value: string) => void;
  /** Already filtered and ranked, at most a handful. */
  readonly suggestions: readonly Suggestion[];
  readonly onSelect: (suggestion: Suggestion) => void;
  readonly id?: string;
  readonly className?: string;
  readonly autoFocus?: boolean;
  readonly inputMode?: "email" | "text" | "search";
  readonly placeholder?: string;
  /** Not called for the Enter that picks a highlighted suggestion. */
  readonly onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
  readonly onPaste?: (event: ClipboardEvent<HTMLInputElement>) => void;
  readonly onBlur?: (event: FocusEvent<HTMLInputElement>) => void;
}

export const SuggestInput = ({
  autoFocus = false,
  className,
  id,
  inputMode = "text",
  onBlur,
  onKeyDown,
  onPaste,
  onSelect,
  onValueChange,
  placeholder,
  suggestions,
  value,
}: SuggestInputProps) => {
  const container = usePortalContainer();
  const [highlighted, setHighlighted] = useState<Suggestion | undefined>();
  const [open, setOpen] = useState(false);
  return (
    <Autocomplete.Root
      filteredItems={[...suggestions]}
      itemToStringValue={(item: Suggestion) => item.id}
      items={[...suggestions]}
      onOpenChange={(next) => {
        setOpen(next);
      }}
      onItemHighlighted={(item: Suggestion | undefined) => {
        setHighlighted(item);
      }}
      onValueChange={(next, details) => {
        if (details.reason === "item-press") {
          const picked = suggestions.find((item) => item.id === next);
          if (picked !== undefined) {
            onSelect(picked);
          }
          onValueChange("");
          return;
        }
        onValueChange(next);
      }}
      // An empty list is not shown.
      open={open && suggestions.length > 0}
      value={value}
    >
      <Autocomplete.Input
        autoComplete="off"
        // oxlint-disable-next-line jsx-a11y/no-autofocus -- the caller decides (a new message starts in To).
        autoFocus={autoFocus}
        inputMode={inputMode}
        onBlur={onBlur}
        onKeyDown={(event) => {
          // Enter on a highlighted suggestion belongs to the list.
          const listShown = open && suggestions.length > 0;
          const plainEnter =
            event.key === "Enter" && !event.metaKey && !event.ctrlKey;
          if (plainEnter && listShown && highlighted !== undefined) {
            return;
          }
          onKeyDown?.(event);
        }}
        onPaste={onPaste}
        spellCheck={false}
        {...(id === undefined ? {} : { id })}
        {...(className === undefined ? {} : { className })}
        {...(placeholder === undefined ? {} : { placeholder })}
      />
      <Autocomplete.Portal container={container}>
        <Autocomplete.Positioner
          align="start"
          className={cls(styles.positioner)}
          sideOffset={6}
        >
          <Autocomplete.Popup
            className={(state) =>
              cls(
                styles.popup,
                state.transitionStatus === "starting" ||
                  state.transitionStatus === "ending"
                  ? styles.hidden
                  : null
              )
            }
          >
            <Autocomplete.List>
              {(item: Suggestion) => (
                <Autocomplete.Item
                  className={(state) =>
                    cls(
                      styles.item,
                      state.highlighted ? styles.highlighted : null
                    )
                  }
                  key={item.id}
                  value={item}
                >
                  {item.avatarKey === undefined ? null : (
                    <Avatar
                      colorKey={item.avatarKey}
                      name={item.label}
                      size={26}
                    />
                  )}
                  <span {...stylex.props(styles.text)}>
                    <span {...stylex.props(styles.label)}>{item.label}</span>
                    {item.description === undefined ? null : (
                      <span {...stylex.props(styles.description)}>
                        {item.description}
                      </span>
                    )}
                  </span>
                </Autocomplete.Item>
              )}
            </Autocomplete.List>
          </Autocomplete.Popup>
        </Autocomplete.Positioner>
      </Autocomplete.Portal>
    </Autocomplete.Root>
  );
};
