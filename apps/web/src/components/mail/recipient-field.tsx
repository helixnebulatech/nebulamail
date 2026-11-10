// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// To/Cc/Bcc: chips plus a text input. Comma, semicolon, Enter or leaving
// the field turns what was typed into chips; Backspace on an empty input
// removes the last one. Invalid addresses stay, marked, so nothing is lost.
// While typing, people from the address book and recent recipients are
// suggested (arrow keys + Enter, or a click).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Avatar, SuggestInput } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { X } from "lucide-react";
import { useId, useState } from "react";
import type { ClipboardEvent, KeyboardEvent, ReactNode } from "react";

import type { Recipient } from "../../mail/compose";
import { isValidEmail } from "../../mail/compose";
import { mergeRecipients, splitRecipientInput } from "../../mail/recipients";
import { useRecipientSuggestions } from "../../mail/use-suggestions";
import { m } from "../../paraglide/messages.js";
import { shared } from "./styles";

const styles = stylex.create({
  chip: {
    alignItems: "center",
    backgroundColor: colors.hover,
    borderRadius: radius.pill,
    columnGap: "6px",
    display: "inline-flex",
    fontSize: "13px",
    fontWeight: font.weightMedium,
    height: "28px",
    maxWidth: "100%",
    paddingInlineEnd: "4px",
    paddingInlineStart: "4px",
    whiteSpace: "nowrap",
  },
  chipInvalid: {
    backgroundColor: colors.dangerSoft,
    color: colors.danger,
  },
  field: {
    alignItems: "center",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: "8px",
    display: "flex",
    minHeight: "44px",
    paddingInline: "16px",
  },
  input: {
    backgroundColor: "transparent",
    borderStyle: "none",
    color: colors.fg,
    flexGrow: 1,
    width: "100%",
    fontFamily: "inherit",
    fontSize: font.sizeMd,
    minWidth: "80px",
    outline: "none",
    paddingBlock: "6px",
  },
  inputItem: {
    display: "flex",
    flexBasis: "80px",
    flexGrow: 1,
    minWidth: "80px",
  },
  // Label and toggles stay on the first line when the chips wrap.
  firstLine: {
    alignItems: "center",
    alignSelf: "flex-start",
    display: "flex",
    flexShrink: 0,
    height: "44px",
  },
  // As wide as its text: "Para", "CC" and "CCO" (or "To", "Cc", "Bcc")
  // differ, and a fixed width cut the longer ones.
  label: { color: colors.fg3, flexShrink: 0, fontSize: font.sizeMd },
  list: {
    alignItems: "center",
    display: "flex",
    flexGrow: 1,
    flexWrap: "wrap",
    gap: "4px 6px",
    minWidth: 0,
    paddingBlock: "6px",
  },
  remove: {
    alignItems: "center",
    borderRadius: radius.pill,
    color: colors.fg3,
    display: "inline-grid",
    height: "20px",
    justifyContent: "center",
    width: "20px",
    backgroundColor: { ":hover": colors.line, default: "transparent" },
  },
});

export interface RecipientFieldProps {
  readonly label: string;
  readonly value: readonly Recipient[];
  readonly onChange: (value: readonly Recipient[]) => void;
  /** Extra controls at the end (the Cc/Bcc toggles). */
  readonly end?: ReactNode;
  readonly autoFocus?: boolean;
  /** Extra key handling (the composer's ⌘↵ / Ctrl Enter). */
  readonly onKeyDown?: (event: KeyboardEvent<HTMLInputElement>) => void;
}

export const RecipientField = ({
  autoFocus = false,
  end,
  label,
  onChange,
  onKeyDown: onExtraKeyDown,
  value,
}: RecipientFieldProps) => {
  const [text, setText] = useState("");
  const id = useId();
  const suggestions = useRecipientSuggestions(
    text,
    value.map((recipient) => recipient.email)
  );
  const inputClass = stylex.props(styles.input).className;

  const commit = (input: string, final: boolean): void => {
    const { entries, rest } = splitRecipientInput(final ? `${input},` : input);
    if (entries.length > 0) {
      onChange(mergeRecipients(value, entries));
    }
    setText(rest);
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    onExtraKeyDown?.(event);
    if (event.defaultPrevented) {
      return;
    }
    if (event.key === "Enter" && text.trim() !== "") {
      event.preventDefault();
      commit(text, true);
    } else if (event.key === "Backspace" && text === "" && value.length > 0) {
      onChange(value.slice(0, -1));
    }
  };

  const onPaste = (event: ClipboardEvent<HTMLInputElement>): void => {
    const pasted = event.clipboardData.getData("text");
    if (/[,;\n]/u.test(pasted)) {
      event.preventDefault();
      commit(`${text}${pasted}`, true);
    }
  };

  return (
    <div {...stylex.props(styles.field)}>
      <label htmlFor={id} {...stylex.props(styles.firstLine, styles.label)}>
        {label}
      </label>
      <ul aria-label={label} {...stylex.props(shared.resetList, styles.list)}>
        {value.map((recipient) => {
          const valid = isValidEmail(recipient.email);
          const name = cleanDisplayText(recipient.name ?? recipient.email);
          return (
            <li
              key={recipient.email}
              title={
                valid
                  ? recipient.email
                  : m.composer_invalid_recipient({ email: recipient.email })
              }
              {...stylex.props(styles.chip, valid ? null : styles.chipInvalid)}
            >
              <Avatar colorKey={recipient.email} name={name} size={20} />
              <span {...stylex.props(shared.ellipsis)}>{name}</span>
              {valid ? null : (
                <span {...stylex.props(shared.srOnly)}>
                  {m.composer_invalid_recipient({ email: recipient.email })}
                </span>
              )}
              <button
                aria-label={m.composer_remove_recipient({
                  email: recipient.email,
                })}
                onClick={() => {
                  onChange(value.filter((other) => other !== recipient));
                }}
                type="button"
                {...stylex.props(
                  shared.resetButton,
                  styles.remove,
                  shared.focusRing
                )}
              >
                <X aria-hidden size={12} />
              </button>
            </li>
          );
        })}
        <li {...stylex.props(styles.inputItem)}>
          <SuggestInput
            autoFocus={autoFocus}
            id={id}
            inputMode="email"
            onBlur={() => {
              commit(text, true);
            }}
            onKeyDown={handleKeyDown}
            onPaste={onPaste}
            onSelect={(suggestion) => {
              onChange(
                mergeRecipients(value, [
                  suggestion.label === suggestion.id
                    ? { email: suggestion.id }
                    : { email: suggestion.id, name: suggestion.label },
                ])
              );
              setText("");
            }}
            onValueChange={(next) => {
              commit(next, false);
            }}
            suggestions={suggestions.map((person) => ({
              avatarKey: person.email,
              id: person.email,
              label: cleanDisplayText(person.name || person.email),
              ...(person.name === "" ? {} : { description: person.email }),
            }))}
            value={text}
            {...(inputClass === undefined ? {} : { className: inputClass })}
          />
        </li>
      </ul>
      {end === undefined ? null : (
        <span {...stylex.props(styles.firstLine)}>{end}</span>
      )}
    </div>
  );
};
