// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The address field of "Assign to" and "Pass it on to"
// people from the address book and recent recipients
// while typing, or typed addresses (Enter or a comma). A typo stays in the
// field and is said.

import { cleanDisplayText } from "@nebulamail/mail-render";
import { SuggestInput } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useState } from "react";
import type { KeyboardEvent } from "react";

import { addressOf } from "../../calendar/itip";
import { useRecipientSuggestions } from "../../mail/use-suggestions";
import { m } from "../../paraglide/messages.js";
import type { AssigneeInput } from "../../tasks/assignment";

const styles = stylex.create({
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  input: {
    backgroundColor: colors.bg,
    borderColor: { ":focus": colors.acc, default: colors.line2 },
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    color: colors.fg,
    fontFamily: "inherit",
    fontSize: font.sizeMd,
    height: "34px",
    outline: "none",
    paddingInline: "10px",
    width: "100%",
  },
});

/** Typed addresses: split on spaces, commas and semicolons. */
const typedAddresses = (
  text: string
): { readonly found: string[]; readonly invalid: string | null } => {
  const parts = text
    .split(/[\s,;]+/u)
    .map((part) => part.trim().replaceAll(/^<|>$/gu, ""))
    .filter((part) => part !== "");
  const found: string[] = [];
  let invalid: string | null = null;
  for (const part of parts) {
    const email = addressOf(part);
    if (email === null) {
      invalid ??= part;
    } else {
      found.push(email);
    }
  }
  return { found, invalid };
};

/** The address field: contacts while typing, or typed addresses. */
export const AssignInput = ({
  exclude,
  hint,
  id,
  onAdd,
}: {
  readonly id: string;
  /** The line under the field (default: how assigning works). */
  readonly hint?: string;
  /** Addresses not to suggest (already there, the user's own). */
  readonly exclude: readonly string[];
  readonly onAdd: (people: readonly AssigneeInput[]) => void;
}) => {
  const [text, setText] = useState("");
  const [invalid, setInvalid] = useState<string | null>(null);
  const suggestions = useRecipientSuggestions(text, exclude);
  const commit = (): void => {
    if (text.trim() === "") {
      return;
    }
    const { found, invalid: bad } = typedAddresses(text);
    onAdd(found.map((email) => ({ email, name: null })));
    setInvalid(bad);
    setText(bad ?? "");
  };
  const inputClass = stylex.props(styles.input).className;
  return (
    <>
      <SuggestInput
        id={id}
        inputMode="email"
        onBlur={commit}
        onKeyDown={(event: KeyboardEvent<HTMLInputElement>) => {
          // ⌘/Ctrl ↵ closes the panel: let it through.
          const plainEnter =
            event.key === "Enter" && !event.metaKey && !event.ctrlKey;
          if (plainEnter || event.key === ",") {
            event.preventDefault();
            commit();
          }
        }}
        onSelect={(suggestion) => {
          onAdd([
            {
              email: suggestion.id,
              name:
                suggestion.label === suggestion.id ? null : suggestion.label,
            },
          ]);
          setText("");
          setInvalid(null);
        }}
        onValueChange={(next) => {
          setText(next);
          setInvalid(null);
        }}
        placeholder={m.tasks_assign_placeholder()}
        suggestions={suggestions.map((person) => ({
          avatarKey: person.email,
          id: person.email,
          label: cleanDisplayText(person.name || person.email),
          ...(person.name === "" ? {} : { description: person.email }),
        }))}
        value={text}
        {...(inputClass === undefined ? {} : { className: inputClass })}
      />
      {invalid === null ? (
        <p {...stylex.props(styles.hint)}>{hint ?? m.tasks_assign_hint()}</p>
      ) : (
        <p role="alert" {...stylex.props(styles.error)}>
          {m.tasks_assign_invalid({ text: cleanDisplayText(invalid) })}
        </p>
      )}
    </>
  );
};
