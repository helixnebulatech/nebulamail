// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The compose body in plain-text mode: a bare text area in
// the place of the rich editor, with the same look and ⌘↵ to send. What
// is typed is what is sent (text/plain only).

import * as stylex from "@stylexjs/stylex";
import { useEffect, useRef } from "react";
import type { KeyboardEvent } from "react";

import { colors, font } from "./tokens.stylex";

const styles = stylex.create({
  area: {
    backgroundColor: "transparent",
    borderStyle: "none",
    boxSizing: "border-box",
    color: colors.fg,
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeLg,
    lineHeight: 1.6,
    minHeight: "160px",
    outline: "none",
    paddingBlock: "12px",
    paddingInline: "16px",
    resize: "none",
    width: "100%",
  },
});

export interface PlainTextEditorProps {
  readonly value: string;
  readonly onChange: (text: string) => void;
  /** Accessible name of the text area. */
  readonly label: string;
  readonly placeholder?: string;
  /** ⌘↵ / Ctrl+Enter (not with Shift). */
  readonly onSubmit?: () => void;
  /** Focus on mount: at the start (replies) or the end. */
  readonly autoFocus?: "start" | "end" | false;
}

export const PlainTextEditor = ({
  autoFocus = false,
  label,
  onChange,
  onSubmit,
  placeholder,
  value,
}: PlainTextEditorProps) => {
  const area = useRef<HTMLTextAreaElement>(null);

  // Focus once, where the user will type (a DOM side effect).
  useEffect(() => {
    const element = area.current;
    if (element === null || autoFocus === false) {
      return;
    }
    const at = autoFocus === "start" ? 0 : element.value.length;
    element.focus();
    element.setSelectionRange(at, at);
    // Only on mount: `autoFocus` never changes (like the rich editor's).
  }, [autoFocus]);

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>): void => {
    // ⌘⇧↵ is left to the app (the composer's "schedule send").
    if (
      (event.metaKey || event.ctrlKey) &&
      !event.shiftKey &&
      event.key === "Enter"
    ) {
      event.preventDefault();
      onSubmit?.();
    }
  };

  return (
    <textarea
      aria-label={label}
      onChange={(event) => {
        onChange(event.target.value);
      }}
      onKeyDown={onKeyDown}
      ref={area}
      spellCheck
      value={value}
      {...(placeholder === undefined ? {} : { placeholder })}
      {...stylex.props(styles.area)}
    />
  );
};
