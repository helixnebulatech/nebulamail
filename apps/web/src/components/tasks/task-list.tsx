// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The task list of a view or list: the quick add at the
// top, then the sections (Overdue, Today, the days of Upcoming…).
// Keyboard: j/k move, x or Space ticks, Enter opens, e edits the title,
// # deletes, ! then 0–3 sets the priority, d the due date, v the list;
// Tab / Shift+Tab on a task indent and outdent it (only when it can move:
// otherwise Tab leaves the list as usual); in a list, Alt + ↑/↓ changes
// its place (manual order). Dragging a task does the same, or moves it to
// a list of the sidebar. A task ticked here stays (struck through) until
// the view changes, with "Undo". The parent mounts one per view (key), so
// each view starts fresh.

import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Button,
  EmptyState,
  IllustrationInboxZero,
  IllustrationNoResults,
  Spinner,
} from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { ListIndentDecrease, ListIndentIncrease } from "lucide-react";
import { useDeferredValue, useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, RefObject } from "react";

import { useCalendars, useTimeZone } from "../../calendar/hooks";
import { dateInZone } from "../../calendar/time";
import { useShortcut } from "../../lib/shortcuts";
import { useNow } from "../../lib/use-now";
import { m } from "../../paraglide/messages.js";
import type { DropZone, TaskDrop } from "../../tasks/dropping";
import { dropOf, levelOf, zoneAt } from "../../tasks/dropping";
import { dueForChoice } from "../../tasks/due";
import type {
  ListModel,
  PlacedRow,
  SubtaskCount,
} from "../../tasks/list-model";
import {
  fallbackList,
  listModel,
  listsByKey,
  stepIndex,
  subtaskCounts,
} from "../../tasks/list-model";
import { defaultListKey } from "../../tasks/lists";
import type { Task, TaskList as List, TaskPriority } from "../../tasks/model";
import type { TaskScope, TaskSection } from "../../tasks/nav";
import { quickAddTarget } from "../../tasks/nav";
import { dropIndex, stepTarget } from "../../tasks/ordering";
import { indentTarget, outdentTarget } from "../../tasks/outline";
import { useDateLocale } from "../calendar/use-calendar-prefs";
import { bp } from "../mail/breakpoints.stylex";
import type { Command } from "../mail/commands";
import { useCommands } from "../mail/commands";
import { isInOverlay } from "../mail/mail-ui";
import { shared } from "../mail/styles";
import type { Added } from "./quick-add";
import { QuickAdd } from "./quick-add";
import { DueDialog, MoveDialog } from "./task-dialogs";
import type { DragHandlers } from "./task-drag";
import {
  DragGhost,
  listAt,
  perFrame,
  placeGhost,
  setDragInfo,
} from "./task-drag";
import { PRIORITY_KEYS, dayHeading } from "./task-format";
import { TaskRow } from "./task-row";
import type { TaskRowProps } from "./task-row";
import { requestTitleEdit } from "./title-edit";
import type { TaskActions } from "./use-task-actions";
import type { TasksNav } from "./use-tasks-nav";

const styles = stylex.create({
  center: {
    alignItems: "center",
    display: "flex",
    flexGrow: 1,
    justifyContent: "center",
    padding: "24px",
  },
  count: { color: colors.fg3, fontWeight: font.weightMedium },
  heading: {
    alignItems: "baseline",
    color: colors.fg2,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    margin: 0,
    paddingBlock: "14px 4px",
    paddingInline: "10px",
    textTransform: "capitalize",
  },
  headingOverdue: { color: colors.danger },
  list: { display: "flex", flexDirection: "column", rowGap: "1px" },
  rows: {
    flexGrow: 1,
    minHeight: 0,
    overflowY: "auto",
    paddingBlock: "0 24px",
    // A readable column of Settings' width (settingsMeasure, 720 px),
    // centred in the panel like Settings' content, the scrollbar still at
    // the panel's edge; quick-add.tsx lines up with it.
    paddingInlineEnd: {
      default: "max(18px, calc((100% - 720px) / 2))",
      [bp.mobile]: "6px",
    },
    paddingInlineStart: {
      default: "max(18px, calc((100% - 720px) / 2))",
      [bp.mobile]: "6px",
    },
  },
  showDone: { paddingBlock: "8px", paddingInline: "4px" },
});

/** Runs `run` unless the key press happened in a dialog or menu. */
const guard =
  (run: () => void) =>
  (event: KeyboardEvent): void => {
    if (!isInOverlay(event.target)) {
      run();
    }
  };

const sectionTitle = (
  section: TaskSection,
  today: string,
  locale: string
): string | null => {
  const { heading } = section;
  if (heading === null) {
    return null;
  }
  switch (heading.kind) {
    case "overdue": {
      return m.tasks_overdue();
    }
    case "today": {
      return m.tasks_today();
    }
    case "completed": {
      return m.tasks_view_done();
    }
    case "day": {
      return dayHeading(heading.day, today, locale);
    }
    default: {
      return heading satisfies never;
    }
  }
};

const emptyText = (
  scope: TaskScope
): { readonly title: string; readonly body: string } => {
  if (scope.kind === "list") {
    return { body: m.tasks_empty_list_body(), title: m.tasks_empty_list() };
  }
  switch (scope.view) {
    case "today": {
      return { body: m.tasks_empty_today_body(), title: m.tasks_empty_today() };
    }
    case "upcoming": {
      return {
        body: m.tasks_empty_upcoming_body(),
        title: m.tasks_empty_upcoming(),
      };
    }
    case "all": {
      return { body: m.tasks_empty_all_body(), title: m.tasks_empty_all() };
    }
    case "done": {
      return { body: m.tasks_empty_done_body(), title: m.tasks_empty_done() };
    }
    default: {
      return scope.view satisfies never;
    }
  }
};

const focusRow = (id: string): void => {
  const button = document.querySelector<HTMLButtonElement>(
    `[data-task-id="${CSS.escape(id)}"] [data-task-button]`
  );
  button?.focus();
  button?.scrollIntoView({ block: "nearest" });
};

/** Our own email links (and `mid:` ones): the task came from an email. */
const fromEmail = (task: Task): boolean =>
  task.links.some(
    (link) =>
      link.href.startsWith("mid:") ||
      link.href.startsWith(`${globalThis.location.origin}/mail/`)
  );

/** The keyboard of the list (see the file's comment). */
const useListKeys = ({
  focused,
  onDue,
  onMove,
  actions,
  nav,
  move,
  toggle,
}: {
  readonly focused: PlacedRow | undefined;
  readonly onDue: ((task: Task) => void) | null;
  readonly onMove: ((task: Task) => void) | null;
  readonly actions: TaskActions;
  readonly nav: TasksNav;
  readonly move: (step: number) => void;
  readonly toggle: (row: PlacedRow) => void;
}): void => {
  const enabled = { enabled: onDue !== null };
  const withFocused = (run: (task: Task) => void) =>
    guard(() => {
      if (focused !== undefined) {
        run(focused.task);
      }
    });
  const priority = (level: TaskPriority) =>
    withFocused((task) => {
      actions.setPriority(task, level);
    });
  useShortcut(
    "j",
    guard(() => {
      move(1);
    }),
    enabled
  );
  useShortcut(
    "k",
    guard(() => {
      move(-1);
    }),
    enabled
  );
  useShortcut(
    "x",
    guard(() => {
      if (focused !== undefined) {
        toggle(focused);
      }
    }),
    enabled
  );
  useShortcut(
    "#",
    withFocused((task) => {
      if (nav.open?.id === task.id) {
        nav.openTask(null);
      }
      actions.remove(task);
    }),
    enabled
  );
  useShortcut(
    "e",
    withFocused((task) => {
      nav.openTask(task);
      requestTitleEdit();
    }),
    enabled
  );
  useShortcut(
    "d",
    withFocused((task) => {
      onDue?.(task);
    }),
    enabled
  );
  useShortcut(
    "v",
    withFocused((task) => {
      onMove?.(task);
    }),
    enabled
  );
  useShortcut(`! ${PRIORITY_KEYS.none}`, priority("none"), enabled);
  useShortcut(`! ${PRIORITY_KEYS.low}`, priority("low"), enabled);
  useShortcut(`! ${PRIORITY_KEYS.medium}`, priority("medium"), enabled);
  useShortcut(`! ${PRIORITY_KEYS.high}`, priority("high"), enabled);
};

/** What a row needs to be dragged (see `useListDrag`). */
type RowDrag = (
  row: PlacedRow
) => Pick<TaskRowProps, "drag" | "dragging" | "dropMark">;

const Section = ({
  actions,
  counts,
  rowDrag,
  focused,
  lists,
  mixed,
  nav,
  onFocus,
  onKeyDown,
  onToggle,
  openId,
  rows,
  section,
  title,
}: {
  readonly section: TaskSection;
  readonly title: string | null;
  readonly rows: readonly PlacedRow[];
  readonly focused: PlacedRow | undefined;
  /** The keyboard has moved: show where it is. */
  readonly openId: string | undefined;
  readonly mixed: boolean;
  readonly lists: ReadonlyMap<string, List>;
  readonly counts: ReadonlyMap<string, SubtaskCount>;
  readonly actions: TaskActions;
  readonly nav: TasksNav;
  readonly onFocus: (row: PlacedRow | null) => void;
  readonly onToggle: (row: PlacedRow, done: boolean) => void;
  readonly onKeyDown: (
    row: PlacedRow,
    event: ReactKeyboardEvent<HTMLButtonElement>
  ) => void;
  readonly rowDrag: RowDrag;
}) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const now = useNow();
  const [first] = rows;
  return (
    // Only a titled group is a region of its own.
    <section {...(title === null ? {} : { "aria-label": title })}>
      {title === null ? null : (
        <h2
          {...stylex.props(
            styles.heading,
            section.heading?.kind === "overdue" ? styles.headingOverdue : null
          )}
        >
          {title}
          <span {...stylex.props(styles.count)}>{rows.length}</span>
        </h2>
      )}
      <ul
        aria-label={title ?? m.tasks_title_app()}
        {...stylex.props(shared.resetList, styles.list)}
      >
        {rows.map((row) => {
          const list =
            row.task.listKey === null ? undefined : lists.get(row.task.listKey);
          const count =
            row.task.uid === null
              ? undefined
              : counts.get(`${row.task.accountId}/${row.task.uid}`);
          return (
            <TaskRow
              {...rowDrag(row)}
              assignees={actions.assigneesOf(row.task)}
              canEdit={actions.canEdit(row.task)}
              depth={row.depth}
              done={row.done}
              focused={(focused ?? first) === row}
              fromEmail={fromEmail(row.task)}
              highlighted={focused === row}
              key={`${row.task.accountId}/${row.task.id}`}
              list={
                mixed && list !== undefined
                  ? { color: list.color, name: list.name }
                  : null
              }
              locale={locale}
              now={now}
              onFocus={() => {
                onFocus(row);
              }}
              onKeyDown={(event) => {
                onKeyDown(row, event);
              }}
              onOpen={() => {
                onFocus(row);
                nav.openTask(row.task);
              }}
              onToggle={(done) => {
                onToggle(row, done);
              }}
              open={openId === row.task.id}
              subtasks={count ?? null}
              task={row.task}
              zone={zone}
            />
          );
        })}
      </ul>
    </section>
  );
};

/** Tab / Shift+Tab; false when the task can't move (Tab goes on). */
const indentRow = (
  row: PlacedRow,
  out: boolean,
  model: ListModel,
  actions: TaskActions
): boolean => {
  if (!actions.canEdit(row.task) || row.done) {
    return false;
  }
  const section = model.bySection.get(row.section) ?? [];
  if (out) {
    const parentUid = outdentTarget(section, row.index);
    if (parentUid !== undefined) {
      actions.setParent(row.task, parentUid);
    }
    return parentUid !== undefined;
  }
  const above = indentTarget(section, row.index)?.uid ?? null;
  if (above !== null) {
    actions.setParent(row.task, above);
  }
  return above !== null;
};

/** Tab and Shift+Tab of the focused task in ⌘K (with the parent's name). */
const nestCommands = (
  row: PlacedRow | undefined,
  model: ListModel,
  actions: TaskActions
): Command[] => {
  if (row === undefined || !actions.canEdit(row.task) || row.done) {
    return [];
  }
  const section = model.bySection.get(row.section) ?? [];
  const above = indentTarget(section, row.index);
  const title = cleanDisplayText(row.task.title) || m.tasks_no_title();
  return [
    ...(above === null
      ? []
      : [
          {
            group: "actions" as const,
            icon: ListIndentIncrease,
            id: "task-indent",
            keywords: "subtask subtarea indent sangrar",
            label: m.tasks_indent_command({
              parent: cleanDisplayText(above.title) || m.tasks_no_title(),
            }),
            run: () => {
              indentRow(row, false, model, actions);
            },
            shortcut: "tab",
          },
        ]),
    ...(outdentTarget(section, row.index) === undefined
      ? []
      : [
          {
            group: "actions" as const,
            icon: ListIndentDecrease,
            id: "task-outdent",
            keywords: "subtask subtarea outdent level nivel",
            label: m.tasks_outdent_command({ title }),
            run: () => {
              indentRow(row, true, model, actions);
            },
            shortcut: "shift+tab",
          },
        ]),
  ];
};

/** `d` and `v`: which task's due date or list is being picked. */
const useRowDialogs = (actions: TaskActions) => {
  const [dueFor, setDueFor] = useState<Task | null>(null);
  const [moveFor, setMoveFor] = useState<Task | null>(null);
  return {
    closeDue: () => {
      setDueFor(null);
    },
    closeMove: () => {
      setMoveFor(null);
    },
    dueFor,
    idle: dueFor === null && moveFor === null,
    moveFor,
    openDue: (task: Task) => {
      if (actions.canEdit(task)) {
        setDueFor(task);
      }
    },
    openMove: (task: Task) => {
      if (actions.canEdit(task)) {
        setMoveFor(task);
      }
    },
  };
};

/** The due date and list dialogs of `useRowDialogs`. */
const RowDialogs = ({
  actions,
  dialogs,
  now,
  zone,
}: {
  readonly actions: TaskActions;
  readonly dialogs: ReturnType<typeof useRowDialogs>;
  readonly now: number;
  readonly zone: string;
}) => (
  <>
    <DueDialog
      now={now}
      onChoose={(due) => {
        if (dialogs.dueFor !== null) {
          actions.setDue(dialogs.dueFor, due);
          const { id } = dialogs.dueFor;
          dialogs.closeDue();
          setTimeout(() => {
            focusRow(id);
          }, 0);
        }
      }}
      onClose={() => {
        dialogs.closeDue();
      }}
      task={dialogs.dueFor}
      zone={zone}
    />
    <MoveDialog
      lists={actions.snapshot?.lists ?? []}
      onChoose={(list) => {
        if (dialogs.moveFor !== null) {
          actions.moveTo(dialogs.moveFor, list.id);
          dialogs.closeMove();
        }
      }}
      onClose={() => {
        dialogs.closeMove();
      }}
      task={dialogs.moveFor}
    />
  </>
);

/** Nothing to add to in Completed, nor without a list to put it in. */
const canQuickAdd = (scope: TaskScope, listKey: string | null): boolean =>
  listKey !== null && !(scope.kind === "view" && scope.view === "done");

/** A task's own keys: Tab / Shift+Tab, Space, ↑/↓. */
const rowKeyHandler =
  ({
    indent,
    move,
    reorder,
    toggle,
  }: {
    readonly indent: (row: PlacedRow, out: boolean) => boolean;
    readonly move: (step: number) => void;
    readonly toggle: (row: PlacedRow, done: boolean) => void;
    readonly reorder: (row: PlacedRow, step: -1 | 1) => void;
  }) =>
  (row: PlacedRow, event: ReactKeyboardEvent<HTMLButtonElement>): void => {
    const plain = !event.altKey && !event.metaKey && !event.ctrlKey;
    const arrow = event.key === "ArrowDown" || event.key === "ArrowUp";
    if (arrow && event.altKey && !event.metaKey && !event.ctrlKey) {
      // Alt + ↑/↓: the manual order of a list (dragging without a mouse).
      event.preventDefault();
      reorder(row, event.key === "ArrowDown" ? 1 : -1);
    } else if (event.key === "Tab" && plain) {
      if (indent(row, event.shiftKey)) {
        event.preventDefault();
        setTimeout(() => {
          focusRow(row.task.id);
        }, 0);
      }
    } else if (event.key === " ") {
      event.preventDefault();
      toggle(row, !row.done);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      move(event.key === "ArrowDown" ? 1 : -1);
    }
  };

/** Nothing to show: the view's empty state, or no search results. */
const Placeholder = ({
  scope,
  searching,
}: {
  readonly scope: TaskScope;
  readonly searching: boolean;
}) => {
  const empty = emptyText(scope);
  return (
    <div {...stylex.props(styles.center)}>
      {searching ? (
        <EmptyState
          illustration={<IllustrationNoResults />}
          title={m.tasks_no_results()}
        />
      ) : (
        <EmptyState
          illustration={<IllustrationInboxZero />}
          title={empty.title}
        >
          {empty.body}
        </EmptyState>
      )}
    </div>
  );
};

/** The centre (y) of a row on screen; NaN if it isn't there. */
const middleOf = (id: string): number => {
  const box = document
    .querySelector(`[data-task-id="${CSS.escape(id)}"]`)
    ?.getBoundingClientRect();
  return box === undefined ? Number.NaN : box.top + box.height / 2;
};

const rowsByIdCache = new WeakMap<
  readonly PlacedRow[],
  ReadonlyMap<string, PlacedRow>
>();

/** Rows by task id, once per rows array (asked on every drag frame). */
const rowsById = (
  rows: readonly PlacedRow[]
): ReadonlyMap<string, PlacedRow> => {
  let byId = rowsByIdCache.get(rows);
  if (byId === undefined) {
    // The first row of an id wins, as `rows.find` did.
    byId = new Map(rows.toReversed().map((row) => [row.task.id, row]));
    rowsByIdCache.set(rows, byId);
  }
  return byId;
};

/** The task row under a point, and the part of it the point is on. */
const rowAt = (
  rows: readonly PlacedRow[],
  x: number,
  y: number
): { readonly row: PlacedRow; readonly zone: DropZone } | null => {
  for (const element of document.elementsFromPoint(x, y)) {
    const found = element.closest<HTMLElement>("[data-task-id]");
    const id = found?.dataset.taskId;
    const row = id === undefined ? undefined : rowsById(rows).get(id);
    if (found !== null && row !== undefined) {
      const box = found.getBoundingClientRect();
      return { row, zone: zoneAt(box.top, box.height, y) };
    }
  }
  return null;
};

interface ListDrag {
  readonly row: PlacedRow;
  /** Where it started (the ghost then follows the pointer by itself). */
  readonly x: number;
  readonly y: number;
  /** Over a task: under it, or next to it (dropping.ts). */
  readonly drop: TaskDrop | null;
  /** Between rows: its new place among its siblings; null: none. */
  readonly to: number | null;
  /** A list of the sidebar that takes it. */
  readonly list: List | null;
}

type DragMark = {
  readonly id: string;
  readonly at: "before" | "after" | "inside";
} | null;

/** Where the drop shows: a line next to a row, or the row lit up. */
const markOf = (
  drag: ListDrag | null,
  rows: readonly PlacedRow[]
): DragMark => {
  if (drag?.drop?.kind === "nest") {
    return { at: "inside", id: drag.drop.parent.task.id };
  }
  if (drag?.drop?.kind === "place") {
    return { at: drag.drop.at, id: drag.drop.target.task.id };
  }
  if (drag === null || drag.to === null) {
    return null;
  }
  // Before the sibling it goes in front of, or after the last row
  // (subtasks too) of the one it goes behind.
  const siblings = levelOf(rows, drag.row);
  const from = siblings.indexOf(drag.row);
  const target = siblings[drag.to];
  if (target !== undefined && drag.to < from) {
    return { at: "before", id: target.task.id };
  }
  if (target === undefined || drag.to <= from) {
    return null;
  }
  let last = target;
  for (const next of rows.slice(rows.indexOf(target) + 1)) {
    if (next.section !== target.section || next.depth <= target.depth) {
      break;
    }
    last = next;
  }
  return { at: "after", id: last.task.id };
};

/** What a drag target looks like on screen; equal: nothing to redraw. */
const targetKey = (target: Pick<ListDrag, "drop" | "list" | "to">): string => {
  const { drop } = target;
  let place = "";
  if (drop?.kind === "nest") {
    place = `nest:${drop.parent.task.id}`;
  } else if (drop?.kind === "place") {
    place = `place:${drop.target.task.id}:${drop.at}:${String(drop.to)}:${String(drop.moves)}`;
  }
  return `${place}|${target.list?.key ?? ""}|${String(target.to)}`;
};

/** The subtasks shown under `parent` (where a nested task goes last). */
const childrenOf = (
  rows: readonly PlacedRow[],
  parent: PlacedRow
): PlacedRow[] =>
  rows.filter(
    (item) =>
      item.section === parent.section && item.parent?.id === parent.task.id
  );

/**
 * Dragging rows: onto another task (its middle: a subtask; its edges:
 * next to it, at its level), to another place at the same level (manual
 * order of a list) or onto a list of the sidebar (same account,
 * writable).
 */
const useListDrag = ({
  actions,
  canReorder,
  lists,
  rows,
}: {
  readonly actions: TaskActions;
  readonly canReorder: (row: PlacedRow) => boolean;
  readonly lists: readonly List[];
  readonly rows: readonly PlacedRow[];
}) => {
  const [drag, setDrag] = useState<ListDrag | null>(null);
  const ghost = useRef<HTMLDivElement>(null);
  /** The pointer's moves of the current drag, one per frame. */
  const frames = useRef<ReturnType<typeof perFrame> | null>(null);
  const measure = useRef<((x: number, y: number) => void) | null>(null);
  const tasks = actions.snapshot?.tasks ?? [];

  const targetOf = (
    row: PlacedRow,
    x: number,
    y: number
  ): Pick<ListDrag, "drop" | "list" | "to"> => {
    const key = listAt(x, y);
    const list =
      lists.find(
        (item) =>
          item.key === key &&
          item.mayWrite &&
          item.accountId === row.task.accountId &&
          item.key !== row.task.listKey
      ) ?? null;
    if (list !== null || key !== null) {
      return { drop: null, list, to: null };
    }
    const over = rowAt(rows, x, y);
    if (over !== null && over.row !== row) {
      const drop = dropOf({
        dragged: row,
        over: over.row,
        rows,
        tasks,
        zone: over.zone,
      });
      // Only a list has a manual order to change.
      const keeps = drop?.kind === "place" && !drop.moves && !canReorder(row);
      return { drop: keeps ? null : drop, list: null, to: null };
    }
    if (!canReorder(row)) {
      return { drop: null, list: null, to: null };
    }
    const siblings = levelOf(rows, row);
    const from = siblings.indexOf(row);
    const middles = siblings.map((item) => middleOf(item.task.id));
    return { drop: null, list: null, to: dropIndex(middles, from, y) };
  };

  const end = (): void => {
    frames.current?.cancel();
    frames.current = null;
    measure.current = null;
    setDrag(null);
    setDragInfo(null);
  };

  const dropAt = (row: PlacedRow, target: Pick<ListDrag, "drop" | "to">) => {
    const { drop } = target;
    if (drop?.kind === "nest") {
      // Last among the subtasks it joins (in a list's manual order).
      const children = childrenOf(rows, drop.parent);
      actions.place(
        row.task,
        drop.parent.task.uid,
        canReorder(row)
          ? { siblings: children.map((item) => item.task), to: children.length }
          : null,
        drop.parent.task
      );
      return;
    }
    const siblings = levelOf(rows, row);
    const from = siblings.indexOf(row);
    if (drop?.kind === "place" && drop.moves) {
      actions.place(
        row.task,
        drop.parentUid,
        canReorder(row)
          ? { siblings: drop.siblings.map((item) => item.task), to: drop.to }
          : null,
        drop.target.parent
      );
      return;
    }
    const to = drop?.kind === "place" ? drop.to : target.to;
    if (to !== null && to !== from) {
      actions.reorder(
        siblings.map((item) => item.task),
        from,
        to
      );
    }
  };

  const handlers = (row: PlacedRow): DragHandlers => ({
    enabled: actions.canEdit(row.task) && !row.done,
    onCancel: end,
    onDrop: (x, y) => {
      const target = targetOf(row, x, y);
      end();
      if (target.list === null) {
        dropAt(row, target);
      } else {
        actions.moveTo(row.task, target.list.id);
      }
    },
    onMove: (x, y) => {
      placeGhost(ghost.current, x, y);
      // This render's rows and lists, for the next frame.
      measure.current = (atX, atY) => {
        const target = targetOf(row, atX, atY);
        // The list re-renders only when the landing place changes.
        setDrag((current) =>
          current === null || targetKey(current) === targetKey(target)
            ? current
            : { ...current, ...target }
        );
        setDragInfo({ overList: target.list?.key ?? null });
      };
      frames.current?.move(x, y);
    },
    onStart: (x, y) => {
      frames.current?.cancel();
      frames.current = perFrame((atX, atY) => {
        measure.current?.(atX, atY);
      });
      setDrag({ drop: null, list: null, row, to: null, x, y });
      setDragInfo({ overList: null });
    },
  });

  const mark = markOf(drag, rows);
  const rowDrag: RowDrag = (row) => ({
    drag: handlers(row),
    dragging: drag?.row.task.id === row.task.id,
    dropMark: mark?.id === row.task.id ? mark.at : null,
  });

  let hint: string | null = null;
  if (drag !== null && drag.list !== null) {
    hint = m.tasks_drop_on_list({ list: cleanDisplayText(drag.list.name) });
  } else if (drag?.drop?.kind === "nest") {
    hint = m.tasks_drop_nest({
      title:
        cleanDisplayText(drag.drop.parent.task.title) || m.tasks_no_title(),
    });
  }

  return {
    ghost:
      drag === null ? null : (
        <DragGhost
          hint={hint}
          ref={ghost}
          title={cleanDisplayText(drag.row.task.title) || m.tasks_no_title()}
          x={drag.x}
          y={drag.y}
        />
      ),
    rowDrag,
  };
};

export const TaskList = ({
  actions,
  nav,
  quickAddRef,
  search: typed,
}: {
  readonly actions: TaskActions;
  readonly nav: TasksNav;
  readonly search: string;
  readonly quickAddRef: RefObject<HTMLInputElement | null>;
}) => {
  // Typing stays quick: the list filters (up to MAX_TASKS) after it.
  const search = useDeferredValue(typed);
  const zone = useTimeZone();
  const locale = useDateLocale();
  const now = useNow();
  const calendars = useCalendars();
  const { snapshot } = actions;
  const [kept, setKept] = useState<ReadonlySet<string>>(new Set());
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [showDone, setShowDone] = useState(false);

  const tasks = snapshot?.tasks ?? [];
  const lists = snapshot?.lists ?? [];
  const model = listModel({
    kept,
    now,
    scope: nav.scope,
    search,
    showDone,
    tasks,
    zone,
  });
  const { rows } = model;
  const openRow = rows.find(
    (row) =>
      row.task.id === nav.open?.id && row.task.accountId === nav.open.accountId
  );
  const focused = rows.find((row) => row.task.id === focusedId) ?? openRow;
  const target = quickAddTarget(
    nav.scope,
    lists,
    fallbackList(
      defaultListKey(lists, snapshot?.saved ?? []),
      calendars.data ?? []
    )
  );

  const toggle = (row: PlacedRow, done: boolean): void => {
    if (done) {
      setKept((current) => new Set([...current, row.task.id]));
    }
    actions.complete(row.task, done);
  };

  const focusOn = (row: PlacedRow | undefined): void => {
    if (row !== undefined) {
      setFocusedId(row.task.id);
      focusRow(row.task.id);
    }
  };

  const move = (step: number): void => {
    const next =
      rows[
        stepIndex(
          focused === undefined ? -1 : rows.indexOf(focused),
          step,
          rows.length
        )
      ];
    focusOn(next);
    if (next !== undefined && nav.open !== null) {
      nav.openTask(next.task);
    }
  };

  const canReorder = (row: PlacedRow): boolean =>
    nav.scope.kind === "list" &&
    row.section === "open" &&
    search.trim() === "" &&
    actions.canEdit(row.task);
  const listDrag = useListDrag({
    actions,
    canReorder,
    lists,
    rows,
  });

  const rowKeys = rowKeyHandler({
    indent: (row, out) => indentRow(row, out, model, actions),
    move,
    reorder: (row, step) => {
      if (!canReorder(row)) {
        return;
      }
      const siblings = levelOf(rows, row);
      const from = siblings.indexOf(row);
      const to = stepTarget(from, step, siblings.length);
      if (to !== null) {
        actions.reorder(
          siblings.map((item) => item.task),
          from,
          to
        );
        setTimeout(() => {
          focusRow(row.task.id);
        }, 0);
      }
    },
    toggle,
  });

  const dialogs = useRowDialogs(actions);
  useCommands(nestCommands(focused, model, actions));
  useListKeys({
    actions,
    focused,
    move,
    nav,
    onDue: dialogs.idle ? dialogs.openDue : null,
    onMove: dialogs.idle ? dialogs.openMove : null,
    toggle: (row) => {
      toggle(row, !row.done);
    },
  });

  // The open task scrolls into view (from a link or the calendar).
  const openId = openRow?.task.id;
  useEffect(() => {
    if (openId !== undefined) {
      document
        .querySelector(`[data-task-id="${CSS.escape(openId)}"]`)
        ?.scrollIntoView({ block: "nearest" });
    }
  }, [openId]);

  const add = async (
    title: string,
    parent: Added | null
  ): Promise<Added | null> => {
    const listKey = parent?.listKey ?? target.listKey;
    if (listKey === null) {
      return null;
    }
    const created = await actions.create({
      due:
        target.due === null ? null : dueForChoice(target.due, null, now, zone),
      listKey,
      parentUid: parent?.uid ?? null,
      title,
    });
    return created === null ? null : { listKey, title, uid: created.uid };
  };

  const today = dateInZone(now, zone).toString();
  const listKey = nav.scope.kind === "list" ? nav.scope.key : null;
  const listDone = tasks.filter(
    (task) => task.completed && listKey !== null && task.listKey === listKey
  ).length;
  const searching = search.trim() !== "";
  const canAdd = canQuickAdd(nav.scope, target.listKey);

  let body = <Placeholder scope={nav.scope} searching={searching} />;
  if (snapshot === undefined) {
    body = (
      <div {...stylex.props(styles.center)}>
        <Spinner label={m.tasks_loading()} />
      </div>
    );
  } else if (rows.length > 0) {
    const counts = subtaskCounts(tasks);
    const byKey = listsByKey(lists);
    body = (
      <>
        {model.sections.map((section) => (
          <Section
            actions={actions}
            counts={counts}
            focused={focusedId === null ? undefined : focused}
            key={section.id}
            lists={byKey}
            // Which list a task is in only matters when there is more
            // than one to tell apart.
            mixed={nav.scope.kind === "view" && byKey.size > 1}
            nav={nav}
            onFocus={(row) => {
              setFocusedId(row?.task.id ?? null);
            }}
            onKeyDown={rowKeys}
            onToggle={toggle}
            rowDrag={listDrag.rowDrag}
            openId={openId}
            rows={model.placedBySection.get(section.id) ?? []}
            section={section}
            title={sectionTitle(section, today, locale)}
          />
        ))}
      </>
    );
  }

  return (
    <>
      {canAdd ? (
        <QuickAdd
          inputRef={quickAddRef}
          onAdd={add}
          onDown={() => {
            focusOn(rows[0]);
          }}
        />
      ) : null}
      <div aria-busy={snapshot === undefined} {...stylex.props(styles.rows)}>
        {body}
        {listDone > 0 && !searching ? (
          <div {...stylex.props(styles.showDone)}>
            <Button
              onClick={() => {
                setShowDone(!showDone);
              }}
              size="sm"
              variant="ghost"
            >
              {showDone
                ? m.tasks_hide_done()
                : m.tasks_show_done({ count: listDone })}
            </Button>
          </div>
        ) : null}
      </div>
      <RowDialogs actions={actions} dialogs={dialogs} now={now} zone={zone} />
      {listDrag.ghost}
    </>
  );
};
