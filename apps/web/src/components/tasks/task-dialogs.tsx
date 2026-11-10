// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Small keyboard dialogs of the task list: `d` sets the
// due date (quick choices or a day), `v` moves the task to another list.

import type { CalendarDate } from "@internationalized/date";
import { parseDate } from "@internationalized/date";
import { cleanDisplayText } from "@nebulamail/mail-render";
import { Button, DatePicker, Dialog } from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Check } from "lucide-react";

import { m } from "../../paraglide/messages.js";
import type { DueChoice } from "../../tasks/due";
import { dueForChoice } from "../../tasks/due";
import type { Task, TaskDue, TaskList } from "../../tasks/model";
import { colorVars } from "../calendar/event-chip";
import { useWeekStart } from "../calendar/use-calendar-prefs";
import { shared } from "../mail/styles";
import { rowStyles } from "./task-row";

const styles = stylex.create({
  choices: { display: "flex", flexWrap: "wrap", gap: "8px" },
  form: { display: "flex", flexDirection: "column", rowGap: "16px" },
  list: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    columnGap: "10px",
    display: "flex",
    fontSize: font.sizeMd,
    minHeight: { default: "36px", [touch.coarse]: touch.target },
    paddingInline: "10px",
    width: "100%",
  },
  lists: { display: "flex", flexDirection: "column", rowGap: "2px" },
  spacer: { flexGrow: 1 },
});

export const DUE_CHOICES: readonly DueChoice[] = [
  "today",
  "tomorrow",
  "next-week",
  "none",
];

export const choiceLabel = (choice: DueChoice): string => {
  switch (choice) {
    case "today": {
      return m.tasks_today();
    }
    case "tomorrow": {
      return m.tasks_tomorrow();
    }
    case "next-week": {
      return m.tasks_next_week();
    }
    case "none": {
      return m.tasks_no_date();
    }
    default: {
      return choice satisfies never;
    }
  }
};

const safeDate = (value: string | undefined): CalendarDate | null => {
  if (value === undefined) {
    return null;
  }
  try {
    return parseDate(value);
  } catch {
    return null;
  }
};

/** The day of a due ("YYYY-MM-DD"). */
export const dueDate = (due: TaskDue | null): string | undefined => {
  if (due === null) {
    return undefined;
  }
  return due.kind === "date" ? due.date : due.local.slice(0, 10);
};

export const DueDialog = ({
  now,
  onChoose,
  onClose,
  task,
  zone,
}: {
  readonly task: Task | null;
  readonly now: number;
  readonly zone: string;
  readonly onChoose: (due: TaskDue | null) => void;
  readonly onClose: () => void;
}) => {
  const [, firstDayOfWeek] = useWeekStart();
  const current = task?.due ?? null;
  return (
    <Dialog.Root
      open={task !== null}
      onOpenChange={(open) => {
        if (!open) {
          onClose();
        }
      }}
    >
      <Dialog.Content closeLabel={m.close()} size="sm">
        <Dialog.Title>{m.tasks_due_title()}</Dialog.Title>
        <Dialog.Body>
          <div {...stylex.props(styles.form)}>
            <div {...stylex.props(styles.choices)}>
              {DUE_CHOICES.map((choice) => (
                <Button
                  key={choice}
                  onClick={() => {
                    onChoose(dueForChoice(choice, current, now, zone));
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {choiceLabel(choice)}
                </Button>
              ))}
            </div>
            <DatePicker
              calendarLabel={m.cal_pick_date()}
              label={m.tasks_due()}
              nextLabel={m.cal_next_month()}
              onChange={(date) => {
                if (date === null) {
                  return;
                }
                const day = date.toString();
                onChoose(
                  current?.kind === "datetime"
                    ? { ...current, local: `${day}${current.local.slice(10)}` }
                    : { date: day, kind: "date" }
                );
              }}
              previousLabel={m.cal_previous_month()}
              value={safeDate(dueDate(current))}
              {...(firstDayOfWeek === undefined ? {} : { firstDayOfWeek })}
            />
          </div>
        </Dialog.Body>
      </Dialog.Content>
    </Dialog.Root>
  );
};

export const MoveDialog = ({
  lists,
  onChoose,
  onClose,
  task,
}: {
  readonly task: Task | null;
  readonly lists: readonly TaskList[];
  readonly onChoose: (list: TaskList) => void;
  readonly onClose: () => void;
}) => {
  // A task stays in its account: only that account's writable lists.
  const choices = lists.filter(
    (list) => list.mayWrite && list.accountId === task?.accountId
  );
  return (
    <Dialog.Root
      open={task !== null}
      onOpenChange={(open) => {
        if (!open) {
          onClose();
        }
      }}
    >
      <Dialog.Content closeLabel={m.close()} size="sm">
        <Dialog.Title>{m.tasks_move_title()}</Dialog.Title>
        <Dialog.Body>
          <ul
            aria-label={m.tasks_lists()}
            {...stylex.props(shared.resetList, styles.lists)}
          >
            {choices.map((list) => (
              <li key={list.key}>
                <button
                  aria-current={list.key === task?.listKey ? "true" : undefined}
                  onClick={() => {
                    onChoose(list);
                  }}
                  type="button"
                  {...stylex.props(
                    shared.resetButton,
                    shared.focusRing,
                    styles.list
                  )}
                >
                  <span
                    aria-hidden
                    {...stylex.props(
                      rowStyles.dot,
                      colorVars.color(list.color)
                    )}
                  />
                  <bdi {...stylex.props(shared.ellipsis)}>
                    {cleanDisplayText(list.name)}
                  </bdi>
                  <span {...stylex.props(styles.spacer)} />
                  {list.key === task?.listKey ? (
                    <Check aria-hidden size={16} />
                  ) : null}
                </button>
              </li>
            ))}
          </ul>
        </Dialog.Body>
      </Dialog.Content>
    </Dialog.Root>
  );
};
