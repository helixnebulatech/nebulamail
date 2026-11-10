// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// One task in the list: a round tick (its ring shows the
// priority), the title, and small marks: due chip ("Tomorrow", "Overdue"),
// repeats, notes, from an email, subtasks done, estimated time, priority
// flag, who has it (a task the user assigned) and, in views that mix
// lists, the list's colour. Subtasks are indented. The title is untrusted
// text: cleaned, inside <bdi>.

import { cleanDisplayText } from "@nebulamail/mail-render";
import { Checkbox } from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  CalendarClock,
  Flag,
  Hourglass,
  ListTree,
  Mail,
  Repeat,
  StickyNote,
  Users,
} from "lucide-react";
import { useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent } from "react";

import { m } from "../../paraglide/messages.js";
import type { TaskAssignee } from "../../tasks/assignment";
import { dueLabel } from "../../tasks/due";
import type { Task, TaskPriority } from "../../tasks/model";
import { colorVars } from "../calendar/event-chip";
import { shared } from "../mail/styles";
import type { DragHandlers } from "./task-drag";
import { useDragSource } from "./task-drag";
import {
  assigneeStatusText,
  dueText,
  estimateText,
  priorityLabel,
} from "./task-format";

export const rowStyles = stylex.create({
  chip: {
    alignItems: "center",
    borderRadius: radius.pill,
    color: colors.fg3,
    columnGap: "4px",
    display: "inline-flex",
    flexShrink: 0,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    lineHeight: "18px",
    whiteSpace: "nowrap",
  },
  chipOverdue: { color: colors.danger, fontWeight: font.weightSemibold },
  chipToday: { color: colors.accText, fontWeight: font.weightSemibold },
  dot: {
    backgroundColor: "var(--event-color)",
    borderRadius: radius.pill,
    flexShrink: 0,
    height: "8px",
    width: "8px",
  },
  done: { color: colors.fg3, textDecorationLine: "line-through" },
  flagHigh: { color: colors.danger },
  flagLow: { color: colors.accText },
  flagMedium: { color: colors.warn },
  icon: { color: colors.fg3, flexShrink: 0 },
  main: {
    alignItems: "center",
    columnGap: "10px",
    display: "flex",
    flexGrow: 1,
    minHeight: { default: "36px", [touch.coarse]: touch.target },
    minWidth: 0,
    paddingInlineEnd: "10px",
    textAlign: "start",
  },
  mark: { alignItems: "center", display: "inline-flex", flexShrink: 0 },
  meta: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexShrink: 0,
    marginInlineStart: "auto",
  },
  row: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.lg,
    columnGap: "10px",
    // A list holds up to MAX_TASKS rows: the browser skips laying out and
    // painting those off screen (they stay in the page: find, focus and
    // screen readers reach them). Paint containment clips what a row draws
    // outside itself, so a row with focus inside (its ring) or a drop
    // line (dropBefore/dropAfter) is drawn as usual.
    containIntrinsicSize: {
      default: "auto 36px",
      // touch.target (44px), the rows' height under a finger.
      [touch.coarse]: "auto 44px",
    },
    contentVisibility: { ":focus-within": "visible", default: "auto" },
    display: "flex",
    paddingInlineStart: "calc(10px + var(--task-depth, 0) * 26px)",
    position: "relative",
  },
  dropAfter: {
    contentVisibility: "visible",
    "::after": {
      backgroundColor: colors.acc,
      borderRadius: radius.pill,
      bottom: "-2px",
      content: '""',
      height: "2px",
      // At the level it lands on (the row's depth).
      insetInlineEnd: "8px",
      insetInlineStart: "calc(8px + var(--task-depth, 0) * 26px)",
      position: "absolute",
    },
  },
  dropBefore: {
    contentVisibility: "visible",
    "::before": {
      backgroundColor: colors.acc,
      borderRadius: radius.pill,
      content: '""',
      height: "2px",
      // At the level it lands on (the row's depth).
      insetInlineEnd: "8px",
      insetInlineStart: "calc(8px + var(--task-depth, 0) * 26px)",
      position: "absolute",
      top: "-2px",
    },
  },
  /** Dropping here makes the dragged task its subtask. */
  dropInside: {
    backgroundColor: { ":hover": colors.accSoft, default: colors.accSoft },
    boxShadow: `inset 0 0 0 2px ${colors.acc}`,
  },
  rowDragging: { opacity: 0.45 },
  rowFocused: {
    backgroundColor: { ":hover": colors.hover, default: colors.subtle },
  },
  rowOpen: {
    backgroundColor: { ":hover": colors.sel, default: colors.sel },
    color: colors.selFg,
  },
  title: {
    fontSize: font.sizeMd,
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
});

const depthVars = stylex.create({
  depth: (depth: number) => ({ "--task-depth": String(Math.min(depth, 6)) }),
});

/** The tick's ring: red, orange, blue for high, medium, low. */
export const priorityTone = (priority: TaskPriority): string | undefined => {
  switch (priority) {
    case "high": {
      return colors.danger;
    }
    case "medium": {
      return colors.warn;
    }
    case "low": {
      return colors.accText;
    }
    case "none": {
      return undefined;
    }
    default: {
      return priority satisfies never;
    }
  }
};

const FLAG_STYLE = {
  high: rowStyles.flagHigh,
  low: rowStyles.flagLow,
  medium: rowStyles.flagMedium,
} as const;

/** The estimated time (RFC 8984 §5.2.3), e.g. "1 h 30 min". */
const EstimateChip = ({ minutes }: { readonly minutes: number | null }) => {
  if (minutes === null) {
    return null;
  }
  const time = estimateText(minutes);
  return (
    <span {...stylex.props(rowStyles.chip)}>
      <Hourglass aria-hidden size={13} />
      <span aria-hidden>{time}</span>
      <span {...stylex.props(shared.srOnly)}>
        {m.tasks_estimate_of({ time })}
      </span>
    </span>
  );
};

/** The due chip ("Overdue · Yesterday", "Today 17:30", "Thu"). */
export const DueChip = ({
  locale,
  now,
  task,
  zone,
}: {
  readonly task: Task;
  readonly now: number;
  readonly zone: string;
  readonly locale: string;
}) => {
  const label = dueLabel(task, now, zone);
  if (label === null) {
    return null;
  }
  const text = dueText(label, locale, zone);
  return (
    <span
      {...stylex.props(
        rowStyles.chip,
        label.overdue ? rowStyles.chipOverdue : null,
        !label.overdue && label.when === "today" && !task.completed
          ? rowStyles.chipToday
          : null
      )}
    >
      <CalendarClock aria-hidden size={13} />
      {label.overdue ? (
        <span {...stylex.props(shared.srOnly)}>{m.tasks_overdue()}: </span>
      ) : null}
      {text}
    </span>
  );
};

export interface TaskRowProps {
  readonly task: Task;
  readonly depth: number;
  /** Subtasks done / all (in this view); null: none. */
  readonly subtasks: { readonly done: number; readonly all: number } | null;
  /** Its button takes Tab (roving focus). */
  readonly focused: boolean;
  /** The keyboard is on it (j/k): shown. */
  readonly highlighted: boolean;
  readonly open: boolean;
  /** Shown as done (also a task just ticked, kept until the view changes). */
  readonly done: boolean;
  /** The list's colour and name (views that mix lists). */
  readonly list: { readonly color: string; readonly name: string } | null;
  readonly canEdit: boolean;
  readonly now: number;
  readonly zone: string;
  readonly locale: string;
  readonly fromEmail: boolean;
  /** Who has it, if the user assigned it (null: not the user's to show). */
  readonly assignees?: readonly TaskAssignee[] | null;
  readonly onToggle: (done: boolean) => void;
  readonly onOpen: () => void;
  readonly onFocus: () => void;
  readonly onKeyDown: (event: ReactKeyboardEvent<HTMLButtonElement>) => void;
  /** Dragging it (to reorder or onto a list); absent: it can't be dragged. */
  readonly drag?: DragHandlers;
  /** It is being dragged. */
  readonly dragging?: boolean;
  /** Where a dragged task would land: next to this one, or under it. */
  readonly dropMark?: "before" | "after" | "inside" | null;
}

const DROP_MARKS = {
  after: rowStyles.dropAfter,
  before: rowStyles.dropBefore,
  inside: rowStyles.dropInside,
} as const;

const ignore = (): void => {
  // Not draggable: nothing to do.
};
const NO_DRAG: DragHandlers = {
  enabled: false,
  onCancel: ignore,
  onDrop: ignore,
  onMove: ignore,
  onStart: ignore,
};

const nameOf = (person: TaskAssignee): string =>
  cleanDisplayText(person.name ?? "") || person.email;

/** "Ana" or "Ana +2", and every name with their status for the tooltip. */
const AssigneesMark = ({
  people,
}: {
  readonly people: readonly TaskAssignee[] | null;
}) => {
  const first = people?.[0];
  if (people === null || first === undefined) {
    return null;
  }
  const shown =
    people.length === 1
      ? nameOf(first)
      : m.tasks_assignees_more({
          count: people.length - 1,
          name: nameOf(first),
        });
  const detail = people
    .map((person) =>
      m.tasks_assignee_status({
        name: nameOf(person),
        status: assigneeStatusText(person.status),
      })
    )
    .join(", ");
  return (
    <span title={detail} {...stylex.props(rowStyles.chip)}>
      <Users aria-hidden size={13} />
      <bdi aria-hidden>{shown}</bdi>
      <span {...stylex.props(shared.srOnly)}>
        {m.tasks_assigned_to_list({ names: detail })}
      </span>
    </span>
  );
};

interface RowList {
  readonly color: string;
  readonly name: string;
}

/** The list's colour, before the title (seen, not read). */
const ListDot = ({ list }: { readonly list: RowList | null }) =>
  list === null ? null : (
    <span title={cleanDisplayText(list.name)} {...stylex.props(rowStyles.mark)}>
      <span
        aria-hidden
        {...stylex.props(rowStyles.dot, colorVars.color(list.color))}
      />
    </span>
  );

/** The list's name for screen readers, after the title and its details. */
const ListName = ({ list }: { readonly list: RowList | null }) =>
  list === null ? null : (
    <span {...stylex.props(shared.srOnly)}>
      {m.tasks_in_list({ name: cleanDisplayText(list.name) })}
    </span>
  );

export const TaskRow = ({
  assignees = null,
  canEdit,
  depth,
  done,
  drag = NO_DRAG,
  dragging = false,
  dropMark = null,
  focused,
  fromEmail,
  highlighted,
  list,
  locale,
  now,
  onFocus,
  onKeyDown,
  onOpen,
  onToggle,
  open,
  subtasks,
  task,
  zone,
}: TaskRowProps) => {
  const title = cleanDisplayText(task.title) || m.tasks_no_title();
  const ref = useRef<HTMLLIElement>(null);
  useDragSource(ref, drag);
  return (
    <li
      data-task-id={task.id}
      ref={ref}
      {...stylex.props(
        rowStyles.row,
        open ? rowStyles.rowOpen : null,
        highlighted && !open ? rowStyles.rowFocused : null,
        dragging ? rowStyles.rowDragging : null,
        dropMark === null ? null : DROP_MARKS[dropMark],
        depthVars.depth(depth)
      )}
    >
      <Checkbox
        aria-label={m.tasks_mark_done({ title })}
        checked={done}
        disabled={!canEdit}
        onCheckedChange={onToggle}
        shape="round"
        tabIndex={-1}
        tone={priorityTone(task.priority)}
      />
      <button
        aria-current={open ? "true" : undefined}
        data-task-button
        onClick={onOpen}
        onFocus={onFocus}
        onKeyDown={onKeyDown}
        tabIndex={focused ? 0 : -1}
        type="button"
        {...stylex.props(shared.resetButton, shared.focusRing, rowStyles.main)}
      >
        {/* Which list: the dot goes before the title (after it, next to a
            red due date, it read as a status); screen readers hear the list
            last, after the title and its details. */}
        <ListDot list={list} />
        <span {...stylex.props(rowStyles.title, done ? rowStyles.done : null)}>
          <bdi>{title}</bdi>
        </span>
        <span {...stylex.props(rowStyles.meta)}>
          {subtasks === null ? null : (
            <span {...stylex.props(rowStyles.chip)}>
              <ListTree aria-hidden size={13} />
              <span aria-hidden>
                {subtasks.done}/{subtasks.all}
              </span>
              <span {...stylex.props(shared.srOnly)}>
                {m.tasks_subtasks_count({
                  all: subtasks.all,
                  done: subtasks.done,
                })}
              </span>
            </span>
          )}
          {task.notes === null ? null : (
            <span {...stylex.props(rowStyles.mark)}>
              <StickyNote
                aria-hidden
                size={14}
                {...stylex.props(rowStyles.icon)}
              />
              <span {...stylex.props(shared.srOnly)}>
                {m.tasks_has_notes()}
              </span>
            </span>
          )}
          {fromEmail ? (
            <span {...stylex.props(rowStyles.mark)}>
              <Mail aria-hidden size={14} {...stylex.props(rowStyles.icon)} />
              <span {...stylex.props(shared.srOnly)}>
                {m.tasks_from_email()}
              </span>
            </span>
          ) : null}
          {task.recurring ? (
            <span {...stylex.props(rowStyles.mark)}>
              <Repeat aria-hidden size={14} {...stylex.props(rowStyles.icon)} />
              <span {...stylex.props(shared.srOnly)}>{m.tasks_repeats()}</span>
            </span>
          ) : null}
          <EstimateChip minutes={task.estimatedMinutes} />
          <DueChip locale={locale} now={now} task={task} zone={zone} />
          <AssigneesMark people={assignees} />
          {task.priority === "none" ? null : (
            <span {...stylex.props(rowStyles.mark)}>
              <Flag
                aria-hidden
                fill="currentColor"
                size={14}
                {...stylex.props(FLAG_STYLE[task.priority])}
              />
              <span {...stylex.props(shared.srOnly)}>
                {m.tasks_priority_of({
                  priority: priorityLabel(task.priority),
                })}
              </span>
            </span>
          )}
        </span>
        <ListName list={list} />
      </button>
    </li>
  );
};
