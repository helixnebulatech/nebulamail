// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Tasks in the calendar (decided 2026-10-28: "Show tasks",
// on by default, per device). Each dated task shows on its day: in the
// all-day row (or a small mark at its time) of the week and day views, in
// the day cell of the month, and in the agenda; with a tick (done ones
// struck through). Clicking the title opens it in Tasks.

import type { CalendarDate } from "@internationalized/date";
import { cleanDisplayText } from "@nebulamail/mail-render";
import { Checkbox, Switch } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import { Fragment, useRef, useState } from "react";
import type { ReactNode } from "react";

import { useTimeZone } from "../../calendar/hooks";
import {
  formatDate,
  formatTime,
  minutesInDay,
  parseLocalDate,
} from "../../calendar/time";
import { useCalendarTasksShown } from "../../lib/use-layout";
import { m } from "../../paraglide/messages.js";
import { SHOW_TASKS_KEY, tasksByDay, tasksOn } from "../../tasks/calendar";
import { useTasks } from "../../tasks/hooks";
import { refParam } from "../../tasks/links";
import type { Task, TaskDue } from "../../tasks/model";
import { dueInstant } from "../../tasks/model";
import type { DueDrop } from "../../tasks/moving";
import { droppedDue, keyNudge, nudgedDue } from "../../tasks/moving";
import { dueDay } from "../../tasks/views";
import { colorVars } from "../calendar/event-chip";
import type { DayExtra, TimedExtra } from "../calendar/time-grid";
import { useDateLocale } from "../calendar/use-calendar-prefs";
import { shared } from "../mail/styles";
import { DragGhost, perFrame, placeGhost, useDragSource } from "./task-drag";
import { priorityTone } from "./task-row";
import { useTaskActions } from "./use-task-actions";
import { useOwnTasksAccount } from "./use-tasks-nav";

const styles = stylex.create({
  chip: {
    alignItems: "center",
    backgroundColor: colors.bg,
    borderColor: colors.line,
    borderInlineStartColor: "var(--event-color)",
    borderInlineStartWidth: "3px",
    borderRadius: radius.sm,
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    columnGap: "6px",
    display: "flex",
    flexGrow: 1,
    fontSize: font.sizeSm,
    height: "22px",
    minWidth: 0,
    paddingInline: "4px",
  },
  chipCompact: { fontSize: font.sizeXs, height: "20px" },
  done: { color: colors.fg3, textDecorationLine: "line-through" },
  dragging: { opacity: 0.5 },
  time: {
    color: colors.fg2,
    flexShrink: 0,
    fontVariantNumeric: "tabular-nums",
  },
  title: {
    color: colors.fg,
    flexGrow: 1,
    minWidth: 0,
    overflow: "hidden",
    textAlign: "start",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  // The switch on the sidebar's icon column (x = 22), with the calendars'
  // boxes and the headings.
  toggle: { paddingInline: "10px" },
});

/**
 * "Show tasks" in the calendar, on this device (on by default), in the
 * preferences cookie so the server renders it as chosen (lib/use-layout.ts;
 * an older choice in localStorage moves there once).
 */
export const useShowTasks = (): readonly [boolean, (on: boolean) => void] =>
  useCalendarTasksShown(SHOW_TASKS_KEY);

/** The switch in the calendar's sidebar. */
export const ShowTasksToggle = () => {
  const [show, setShow] = useShowTasks();
  return (
    <div {...stylex.props(styles.toggle)}>
      <Switch
        checked={show}
        label={m.tasks_show_tasks()}
        onCheckedChange={setShow}
      />
    </div>
  );
};

const STEP = 15;

/** A day column of the week or day view, across (it doesn't scroll so). */
interface DayColumn {
  readonly date: string | undefined;
  readonly left: number;
  readonly right: number;
}

/** The day columns on screen, read once per drag (not on every move). */
const dayColumns = (): readonly DayColumn[] =>
  [...document.querySelectorAll<HTMLElement>('[data-drop-kind="time"]')].map(
    (item) => {
      const box = item.getBoundingClientRect();
      return { date: item.dataset.dropDate, left: box.left, right: box.right };
    }
  );

/**
 * Where a task dropped at (x, y) lands: a time of a day column (15-minute
 * steps; `grab` is how far below the chip's top it was held), the all-day
 * row or header of a day, or a day of the month.
 */
const dropAt = (
  x: number,
  y: number,
  grab: number,
  columns: () => readonly DayColumn[]
): DueDrop | null => {
  for (const element of document.elementsFromPoint(x, y)) {
    const found = element.closest<HTMLElement>("[data-drop-kind]");
    if (found === null) {
      continue;
    }
    const { dropDate, dropKind } = found.dataset;
    if (dropKind === "time" && dropDate !== undefined) {
      const box = found.getBoundingClientRect();
      const minutes =
        Math.round((((y - grab - box.top) / box.height) * 1440) / STEP) * STEP;
      return {
        date: dropDate,
        kind: "time",
        minutes: Math.max(0, Math.min(1440 - STEP, minutes)),
      };
    }
    if (dropKind === "day" && dropDate !== undefined) {
      return { date: dropDate, kind: "day" };
    }
    if (dropKind === "all-day") {
      // The all-day row is one grid: the day is the column under x.
      const column = columns().find((item) => x >= item.left && x < item.right);
      const date = dropDate ?? column?.date;
      return date === undefined ? null : { date, kind: "all-day" };
    }
  }
  return null;
};

/** The hint's date and time formatters by locale and zone (once each). */
const hintFormats = new Map<string, Intl.DateTimeFormat>();

const hintFormat = (locale: string, zone: string): Intl.DateTimeFormat => {
  const key = `${locale}|${zone}`;
  let format = hintFormats.get(key);
  if (format === undefined) {
    format = new Intl.DateTimeFormat(locale, {
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
      month: "short",
      timeZone: zone,
      weekday: "short",
    });
    hintFormats.set(key, format);
  }
  return format;
};

/** "Thu 6 Nov" or "Thu 6 Nov, 10:15" (the ghost's hint). */
const dueHint = (due: TaskDue, zone: string, locale: string): string => {
  if (due.kind === "date") {
    const date = parseLocalDate(due.date);
    return date === null
      ? due.date
      : formatDate(date, locale, {
          day: "numeric",
          month: "short",
          weekday: "short",
        });
  }
  const at = dueInstant(due, zone);
  return at === null ? due.local : hintFormat(locale, zone).format(at);
};

interface ChipDrag {
  /** Where it started (the ghost then follows the pointer by itself). */
  readonly x: number;
  readonly y: number;
  readonly due: TaskDue | null;
}

const TaskChip = ({
  canEdit,
  color,
  compact,
  onMoveDue,
  onOpen,
  onToggle,
  task,
  time,
}: {
  readonly task: Task;
  readonly color: string;
  readonly time: string | null;
  readonly compact: boolean;
  readonly canEdit: boolean;
  readonly onOpen: () => void;
  readonly onToggle: (done: boolean) => void;
  readonly onMoveDue: (due: TaskDue) => void;
}) => {
  const title = cleanDisplayText(task.title) || m.tasks_no_title();
  const zone = useTimeZone();
  const locale = useDateLocale();
  const ref = useRef<HTMLDivElement>(null);
  const ghost = useRef<HTMLDivElement>(null);
  /** The pointer's moves of the current drag, one per frame. */
  const frames = useRef<ReturnType<typeof perFrame> | null>(null);
  const measure = useRef<((x: number, y: number) => void) | null>(null);
  /** How far below the chip's top it was held (set as the drag starts). */
  const grabbed = useRef(0);
  /** The day columns, read when the drag starts. */
  const columns = useRef<readonly DayColumn[] | null>(null);
  const [drag, setDrag] = useState<ChipDrag | null>(null);
  const { due } = task;
  const shownDay = dueDay(task, zone);
  const landing = (x: number, y: number, grab: number): TaskDue | null => {
    const drop = dropAt(x, y, grab, () => {
      const read = columns.current ?? dayColumns();
      columns.current = read;
      return read;
    });
    return drop === null || due === null || shownDay === null
      ? null
      : droppedDue(due, drop, zone, shownDay);
  };
  const end = (): void => {
    frames.current?.cancel();
    frames.current = null;
    measure.current = null;
    columns.current = null;
    setDrag(null);
  };
  // Drag it to another day or time (mouse, pen, or a finger after a
  // moment); Alt + arrows from the keyboard.
  useDragSource(ref, {
    enabled: canEdit && !task.completed && due !== null,
    onCancel: end,
    onDrop: (x, y) => {
      const next = landing(x, y, grabbed.current);
      end();
      if (next !== null && JSON.stringify(next) !== JSON.stringify(due)) {
        onMoveDue(next);
      }
    },
    onMove: (x, y) => {
      placeGhost(ghost.current, x, y);
      // This render's task and zone, for the next frame.
      measure.current = (atX, atY) => {
        const next = landing(atX, atY, grabbed.current);
        // Re-rendered only when where it lands changes.
        setDrag((current) =>
          current === null ||
          JSON.stringify(next) === JSON.stringify(current.due)
            ? current
            : { ...current, due: next }
        );
      };
      frames.current?.move(x, y);
    },
    onStart: (x, y) => {
      const top = ref.current?.getBoundingClientRect().top ?? y;
      frames.current?.cancel();
      frames.current = perFrame((atX, atY) => {
        measure.current?.(atX, atY);
      });
      columns.current = null;
      grabbed.current = y - top;
      setDrag({ due: null, x, y });
    },
  });
  return (
    <div
      data-no-draw
      data-task-chip
      ref={ref}
      {...stylex.props(
        styles.chip,
        compact ? styles.chipCompact : null,
        drag === null ? null : styles.dragging,
        colorVars.color(color)
      )}
    >
      {drag === null ? null : (
        <DragGhost
          hint={drag.due === null ? null : dueHint(drag.due, zone, locale)}
          ref={ghost}
          title={title}
          x={drag.x}
          y={drag.y}
        />
      )}
      <Checkbox
        aria-label={m.tasks_mark_done({ title })}
        checked={task.completed}
        disabled={!canEdit}
        onCheckedChange={onToggle}
        shape="round"
        tone={priorityTone(task.priority)}
      />
      <button
        aria-keyshortcuts={canEdit ? "Alt+ArrowLeft Alt+ArrowRight" : undefined}
        aria-label={m.tasks_calendar_open_task({ title })}
        onClick={onOpen}
        onKeyDown={(event) => {
          const nudge = keyNudge(event);
          if (nudge === null || due === null || !canEdit) {
            return;
          }
          event.preventDefault();
          const next = nudgedDue(due, nudge);
          if (next !== null) {
            onMoveDue(next);
          }
        }}
        type="button"
        {...stylex.props(
          shared.resetButton,
          shared.focusRing,
          styles.title,
          task.completed ? styles.done : null
        )}
      >
        {time === null ? null : (
          <span {...stylex.props(styles.time)}>{time} </span>
        )}
        <bdi>{title}</bdi>
      </button>
    </div>
  );
};

export interface CalendarTasks {
  /** Tasks in the all-day row and marks at their time (week, day). */
  readonly allDay: readonly DayExtra[];
  readonly timed: readonly TimedExtra[];
  /** A day's tasks (month cell, agenda); null: none. */
  readonly forDay: (day: CalendarDate, inList: boolean) => ReactNode[];
  /** Days of `days` with tasks. */
  readonly days: readonly string[];
}

const NONE: CalendarTasks = {
  allDay: [],
  days: [],
  forDay: () => [],
  timed: [],
};

/** The tasks of `days`, ready for the calendar views (none if hidden). */
export const useCalendarTasks = (
  days: readonly CalendarDate[]
): CalendarTasks => {
  const [show] = useShowTasks();
  const zone = useTimeZone();
  const locale = useDateLocale();
  const navigate = useNavigate();
  const own = useOwnTasksAccount();
  const query = useTasks({ enabled: show });
  const actions = useTaskActions({ enabled: show });
  if (!show || query.data === undefined) {
    return NONE;
  }
  const lists = new Map(query.data.lists.map((list) => [list.key, list]));
  const keys = days.map((day) => day.toString());
  const byDay = tasksByDay(query.data.tasks, keys, zone);
  const chip = (task: Task, compact: boolean, timed: boolean) => (
    <TaskChip
      canEdit={actions.canEdit(task)}
      color={
        (task.listKey === null ? undefined : lists.get(task.listKey)?.color) ??
        colors.acc
      }
      compact={compact}
      onMoveDue={(due) => {
        actions.moveDue(task, due);
      }}
      onOpen={() => {
        void navigate({
          search: { task: refParam(task.accountId, task.id, own) },
          to: "/tasks",
        });
      }}
      onToggle={(done) => {
        actions.complete(task, done);
      }}
      task={task}
      time={
        timed && task.dueAt !== null
          ? formatTime(task.dueAt, zone, locale)
          : null
      }
    />
  );
  const allDay: DayExtra[] = [];
  const timed: TimedExtra[] = [];
  for (const [index, key] of keys.entries()) {
    const found = tasksOn(byDay, key);
    for (const task of found.allDay) {
      allDay.push({
        day: index,
        key: `task-${task.accountId}-${task.id}`,
        node: chip(task, true, false),
      });
    }
    for (const task of found.timed) {
      timed.push({
        day: index,
        key: `task-${task.accountId}-${task.id}`,
        minutes: task.dueAt === null ? 0 : minutesInDay(task.dueAt, zone),
        node: chip(task, true, true),
      });
    }
  }
  return {
    allDay,
    days: keys.filter((key) => byDay.has(key)),
    forDay: (day, inList) => {
      const found = tasksOn(byDay, day.toString());
      const all = [...found.allDay, ...found.timed];
      return all.map((task) => {
        const key = `task-${task.accountId}-${task.id}`;
        const node = chip(task, !inList, task.due?.kind === "datetime");
        return inList ? (
          <li key={key}>{node}</li>
        ) : (
          <Fragment key={key}>{node}</Fragment>
        );
      });
    },
    timed,
  };
};
