// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The quick add at the top of the task list: type, Enter
// adds; it stays for the next one. Tab makes the next one a subtask of the
// task just added, Shift+Tab takes it back to the top (only then: Tab
// otherwise moves on as usual). Esc clears it; ↓ goes to the list.

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Spinner } from "@nebulamail/ui";
import {
  colors,
  font,
  motion,
  radius,
  shadow,
  touch,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { CornerDownRight, Plus } from "lucide-react";
import { useState } from "react";
import type { KeyboardEvent, RefObject } from "react";

import { m } from "../../paraglide/messages.js";
import { bp } from "../mail/breakpoints.stylex";

const styles = stylex.create({
  // Under the typed text (border 1 + padding 10 + icon 18 + gap 10).
  hint: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    margin: 0,
    paddingInlineStart: "39px",
  },
  hintSub: { paddingInlineStart: "65px" },
  // "Enter adds it" only while typing: until then it is noise. It keeps
  // its space (only the opacity changes): collapsing it on blur moved the
  // list under the pointer between mousedown and click, so the click on a
  // task missed. Opacity also keeps it in the accessibility tree (the input
  // points to it).
  hintIdle: {
    opacity: { default: 0, [stylex.when.ancestor(":focus-within")]: 1 },
    transitionDuration: motion.fast,
    transitionProperty: "opacity",
  },
  icon: { color: colors.fg3, flexShrink: 0 },
  input: {
    backgroundColor: "transparent",
    borderStyle: "none",
    color: colors.fg,
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    minWidth: 0,
    outline: "none",
    paddingBlock: 0,
  },
  // The standard text field's box (packages/ui input.tsx).
  row: {
    alignItems: "center",
    backgroundColor: colors.bg,
    borderColor: {
      ":focus-within": colors.acc,
      ":hover": colors.fg3,
      default: colors.line2,
    },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: { ":focus-within": shadow.focusHalo, default: "none" },
    boxSizing: "border-box",
    transitionDuration: motion.fast,
    transitionProperty: "border-color, box-shadow",
    transitionTimingFunction: motion.easing,
    columnGap: "10px",
    display: "flex",
    minHeight: { default: "40px", [touch.coarse]: touch.target },
    paddingInlineEnd: "8px",
    // 1px border + 9px: the "+" on the rows' tick column (10px in).
    paddingInlineStart: "9px",
  },
  rowSub: { marginInlineStart: "26px" },
  wrap: {
    position: "relative",
    display: "flex",
    flexDirection: "column",
    flexShrink: 0,
    // The same top margin as Settings' and Contacts' content.
    paddingBlock: "24px 6px",
    // In line with the list's centred column (task-list.tsx `rows`): the
    // field's "+" sits on the tasks' tick column and its text on their
    // titles' column.
    // The field spans exactly that 720px column (settingsMeasure): its
    // edges on the column's edges, its "+" and text on the rows' ticks and
    // titles.
    paddingInline: {
      default: "max(18px, calc((100% - 720px) / 2))",
      [bp.mobile]: "12px",
    },
    rowGap: "4px",
  },
});

/** The task just added (Tab makes the next one its subtask). */
export interface Added {
  readonly uid: string;
  readonly title: string;
  readonly listKey: string;
}

const plainKey = (event: KeyboardEvent): boolean =>
  !event.altKey && !event.metaKey && !event.ctrlKey;

export const QuickAdd = ({
  inputRef,
  onAdd,
  onDown,
}: {
  readonly inputRef: RefObject<HTMLInputElement | null>;
  /** Adds a task (under `parent`); answers what it added. */
  readonly onAdd: (
    title: string,
    parent: Added | null
  ) => Promise<Added | null>;
  /** ↓: to the first task. */
  readonly onDown: () => void;
}) => {
  const [text, setText] = useState("");
  const [added, setAdded] = useState<Added | null>(null);
  const [asSubtask, setAsSubtask] = useState(false);
  const [adding, setAdding] = useState(false);
  const parent = asSubtask ? added : null;

  const submit = async (): Promise<void> => {
    const title = text.trim();
    if (title === "" || adding) {
      return;
    }
    setAdding(true);
    setText("");
    const created = await onAdd(title, parent);
    setAdding(false);
    if (created !== null && parent === null) {
      setAdded(created);
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    if (event.key === "Enter" && plainKey(event)) {
      event.preventDefault();
      void submit();
    } else if (event.key === "Tab" && plainKey(event)) {
      if (!event.shiftKey && !asSubtask && added !== null) {
        event.preventDefault();
        setAsSubtask(true);
      } else if (event.shiftKey && asSubtask) {
        event.preventDefault();
        setAsSubtask(false);
      }
    } else if (event.key === "Escape") {
      setText("");
      event.currentTarget.blur();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      onDown();
    }
  };

  let hint = (
    <>
      {m.tasks_add_subtask_hint()}{" "}
      <bdi>{cleanDisplayText(parent?.title ?? "")}</bdi>
    </>
  );
  if (parent === null) {
    hint = (
      <span>
        {added === null ? m.tasks_add_hint() : m.tasks_add_hint_tab()}
      </span>
    );
  }

  return (
    <div {...stylex.props(styles.wrap, stylex.defaultMarker())}>
      <div
        {...stylex.props(styles.row, parent === null ? null : styles.rowSub)}
      >
        {parent === null ? (
          <Plus aria-hidden size={18} {...stylex.props(styles.icon)} />
        ) : (
          <CornerDownRight
            aria-hidden
            size={18}
            {...stylex.props(styles.icon)}
          />
        )}
        <input
          aria-describedby="task-add-hint"
          aria-label={
            parent === null
              ? m.tasks_add_label()
              : m.tasks_add_subtask_of({
                  title: cleanDisplayText(parent.title),
                })
          }
          dir="auto"
          enterKeyHint="done"
          maxLength={500}
          onChange={(event) => {
            setText(event.target.value);
          }}
          onKeyDown={onKeyDown}
          placeholder={
            parent === null
              ? m.tasks_add_placeholder()
              : m.tasks_add_subtask_placeholder()
          }
          ref={inputRef}
          value={text}
          {...stylex.props(styles.input)}
        />
        {adding ? <Spinner label={m.tasks_saving()} size="sm" /> : null}
      </div>
      <p
        id="task-add-hint"
        {...stylex.props(
          styles.hint,
          parent === null ? styles.hintIdle : styles.hintSub
        )}
      >
        {hint}
      </p>
    </div>
  );
};
