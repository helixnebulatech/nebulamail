// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// The open task, next to the list: every field saves by
// itself (title and notes a moment after typing stops, the rest at once):
// title, done, list, due (a day, or a day and time), priority,
// estimated time, repeat, reminders, who it is assigned to, notes (plain
// text), subtasks and links. ⌘/Ctrl ↵ or Esc
// save what is pending and close. Notes and titles are untrusted: plain
// text, cleaned, inside <bdi>; links open after the same confirmation as
// in mail, except our own email links, which open the email here.

import type { CalendarDate, Time } from "@internationalized/date";
import { parseDate, parseTime } from "@internationalized/date";
import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Button,
  Checkbox,
  DatePicker,
  Field,
  IconButton,
  Input,
  Menu,
  MenuItem,
  SegmentedControl,
  Select,
  Spinner,
  TimeField,
} from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useNavigate } from "@tanstack/react-router";
import {
  ArrowLeft,
  CalendarPlus,
  Clock,
  Ellipsis,
  ExternalLink,
  Hash,
  Link2,
  Mail,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import { useEffect, useEffectEvent, useRef, useState } from "react";
import type {
  FocusEvent,
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent,
} from "react";

import { useTimeZone } from "../../calendar/hooks";
import { fromRule, toRule } from "../../calendar/recurrence";
import { useShortcut } from "../../lib/shortcuts";
import { useNow } from "../../lib/use-now";
import { m } from "../../paraglide/messages.js";
import { dueForChoice } from "../../tasks/due";
import type { AlertInput } from "../../tasks/editing";
import { classifyLink } from "../../tasks/links";
import type { Task, TaskDue, TaskList, TaskPriority } from "../../tasks/model";
import { panelStyles } from "../calendar/event-panel";
import { useEventLinks } from "../calendar/event-text";
import {
  RemindersField,
  RepeatField,
  fieldStyles,
} from "../calendar/repeat-field";
import { useDateLocale, useWeekStart } from "../calendar/use-calendar-prefs";
import { isInOverlay } from "../mail/mail-ui";
import { shared } from "../mail/styles";
import { useKeys } from "../mail/use-apple";
import { AssigneesField } from "./assignees-field";
import { DayRemindersField } from "./day-reminders-field";
import { DUE_CHOICES, choiceLabel, dueDate } from "./task-dialogs";
import { PRIORITIES, priorityLabel, taskAlertText } from "./task-format";
import { DueChip, priorityTone } from "./task-row";
import { useTitleEditRequest } from "./title-edit";
import type { TaskActions } from "./use-task-actions";

const styles = stylex.create({
  chips: { display: "flex", flexWrap: "wrap", gap: "6px" },
  dueRow: {
    alignItems: "end",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "8px",
  },
  dueTime: { width: "120px" },
  estimate: { width: "110px" },
  unit: { color: colors.fg3, fontSize: font.sizeSm },
  group: { display: "flex", flexDirection: "column", rowGap: "6px" },
  groupLabel: {
    color: colors.fg2,
    fontSize: "12.5px",
    fontWeight: font.weightButton,
  },
  link: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: colors.subtle },
    borderRadius: radius.md,
    color: colors.fg,
    columnGap: "8px",
    display: "flex",
    fontSize: font.sizeSm,
    minHeight: { default: "34px", [touch.coarse]: touch.target },
    paddingInline: "10px",
    textAlign: "start",
    textDecoration: "none",
    width: "100%",
  },
  linkIcon: { color: colors.fg3, flexShrink: 0 },
  linkText: { flexGrow: 1, minWidth: 0 },
  links: { display: "flex", flexDirection: "column", rowGap: "4px" },
  muted: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  note: {
    backgroundColor: colors.subtle,
    borderRadius: radius.md,
    color: colors.fg2,
    fontSize: font.sizeSm,
    lineHeight: 1.45,
    margin: 0,
    paddingBlock: "8px",
    paddingInline: "10px",
  },
  saving: {
    color: colors.fg3,
    fontSize: font.sizeXs,
    paddingInline: "6px",
  },
  subAdd: {
    backgroundColor: "transparent",
    borderStyle: "none",
    color: colors.fg,
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    minWidth: 0,
    outline: "none",
  },
  subRow: {
    alignItems: "center",
    columnGap: "10px",
    display: "flex",
    minHeight: { default: "32px", [touch.coarse]: touch.target },
  },
  subTitle: {
    color: colors.fg,
    flexGrow: 1,
    fontSize: font.sizeMd,
    minWidth: 0,
    textAlign: "start",
  },
  subs: { display: "flex", flexDirection: "column", rowGap: "2px" },
  textarea: {
    backgroundColor: colors.bg,
    borderColor: { ":focus": colors.acc, default: colors.line2 },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    color: colors.fg,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    lineHeight: font.lineHeight,
    minHeight: "120px",
    outline: "none",
    paddingBlock: "8px",
    paddingInline: "12px",
    resize: "vertical",
    width: "100%",
  },
  titleInput: {
    backgroundColor: "transparent",
    borderBottomColor: { ":focus": colors.acc, default: "transparent" },
    borderBottomStyle: "solid",
    borderBottomWidth: "2px",
    borderInlineWidth: 0,
    borderTopWidth: 0,
    color: colors.fg,
    flexGrow: 1,
    fontFamily: font.sans,
    fontSize: font.sizeXl,
    fontWeight: font.weightSemibold,
    letterSpacing: font.trackingTight,
    minWidth: 0,
    outline: "none",
    paddingBlock: "4px",
    paddingInline: 0,
  },
  titleDone: { color: colors.fg3, textDecorationLine: "line-through" },
  titleRow: { alignItems: "center", columnGap: "12px", display: "flex" },
});

/** How long typing pauses before the title or notes are saved. */
const SAVE_DELAY_MS = 600;

/**
 * Text that saves itself: `save` runs once typing pauses, on `flush` (blur,
 * close) and when unmounted, only if the text changed.
 */
const useAutosave = (
  initial: string,
  save: (text: string) => void
): {
  readonly value: string;
  readonly set: (text: string) => void;
  readonly flush: () => void;
} => {
  const [value, setValue] = useState(initial);
  const saved = useRef(initial);
  const latest = useRef(initial);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const write = (): void => {
    if (timer.current !== null) {
      clearTimeout(timer.current);
      timer.current = null;
    }
    if (latest.current !== saved.current) {
      saved.current = latest.current;
      save(latest.current);
    }
  };
  // Closing the panel saves what was typed last.
  const onUnmount = useEffectEvent(write);
  useEffect(
    () => () => {
      onUnmount();
    },
    []
  );
  return {
    flush: () => {
      write();
    },
    set: (text) => {
      setValue(text);
      latest.current = text;
      if (timer.current !== null) {
        clearTimeout(timer.current);
      }
      timer.current = setTimeout(() => {
        write();
      }, SAVE_DELAY_MS);
    },
    value,
  };
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

const timeOf = (due: TaskDue | null): Time | null => {
  if (due?.kind !== "datetime") {
    return null;
  }
  try {
    return parseTime(due.local.slice(11, 16));
  } catch {
    return null;
  }
};

const pad = (value: number): string => String(value).padStart(2, "0");

/** The alerts of a task as the editor's minutes, and the ones it keeps. */
const alertMinutes = (task: Task): number[] =>
  task.alerts.flatMap((alert) =>
    alert.minutesBefore === null ? [] : [alert.minutesBefore]
  );

/** "Add a time" to a day, or "No time" for a due with one. */
const TimeToggle = ({
  due,
  onChange,
  zone,
}: {
  readonly due: TaskDue;
  readonly zone: string;
  readonly onChange: (due: TaskDue) => void;
}) =>
  due.kind === "date" ? (
    <Button
      icon={Clock}
      onClick={() => {
        // 9:00 in the user's zone, a usual first guess.
        onChange({
          kind: "datetime",
          local: `${due.date}T09:00:00`,
          timeZone: zone,
        });
      }}
      size="sm"
      variant="ghost"
    >
      {m.tasks_add_time()}
    </Button>
  ) : (
    <Button
      onClick={() => {
        onChange({ date: due.local.slice(0, 10), kind: "date" });
      }}
      size="sm"
      variant="ghost"
    >
      {m.tasks_remove_time()}
    </Button>
  );

const DueField = ({
  disabled,
  now,
  onChange,
  task,
  zone,
}: {
  readonly task: Task;
  readonly disabled: boolean;
  readonly now: number;
  readonly zone: string;
  readonly onChange: (due: TaskDue | null) => void;
}) => {
  const [, firstDayOfWeek] = useWeekStart();
  const { due } = task;
  const day = dueDate(due);
  const time = timeOf(due);
  return (
    <div {...stylex.props(fieldStyles.form)}>
      <div {...stylex.props(styles.dueRow)}>
        <DatePicker
          calendarLabel={m.cal_pick_date()}
          isDisabled={disabled}
          label={m.tasks_due()}
          nextLabel={m.cal_next_month()}
          onChange={(date) => {
            if (date === null) {
              return;
            }
            const next = date.toString();
            onChange(
              due?.kind === "datetime"
                ? { ...due, local: `${next}${due.local.slice(10)}` }
                : { date: next, kind: "date" }
            );
          }}
          previousLabel={m.cal_previous_month()}
          value={safeDate(day)}
          {...(firstDayOfWeek === undefined ? {} : { firstDayOfWeek })}
        />
        {due?.kind === "datetime" ? (
          <span {...stylex.props(styles.dueTime)}>
            <TimeField
              isDisabled={disabled}
              label={m.tasks_due_time()}
              onChange={(value) => {
                if (value !== null && day !== undefined) {
                  onChange({
                    ...due,
                    local: `${day}T${pad(value.hour)}:${pad(value.minute)}:00`,
                  });
                }
              }}
              value={time}
            />
          </span>
        ) : null}
        {due === null || disabled ? null : (
          <TimeToggle due={due} onChange={onChange} zone={zone} />
        )}
      </div>
      {disabled ? null : (
        <div {...stylex.props(styles.chips)}>
          {DUE_CHOICES.filter(
            (choice) => choice !== "none" || due !== null
          ).map((choice) => (
            <Button
              key={choice}
              onClick={() => {
                onChange(dueForChoice(choice, due, now, zone));
              }}
              size="sm"
              variant="secondary"
            >
              {choiceLabel(choice)}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
};

const Subtasks = ({
  actions,
  canEdit,
  onOpen,
  task,
}: {
  readonly task: Task;
  readonly actions: TaskActions;
  readonly canEdit: boolean;
  readonly onOpen: (task: Task) => void;
}) => {
  const [text, setText] = useState("");
  const all = actions.snapshot?.tasks ?? [];
  const seen = new Set<string>();
  const children =
    task.uid === null
      ? []
      : all.filter((item) => {
          if (
            item.parentUid !== task.uid ||
            item.accountId !== task.accountId ||
            seen.has(item.baseId)
          ) {
            return false;
          }
          seen.add(item.baseId);
          return true;
        });
  const add = async (): Promise<void> => {
    const title = text.trim();
    if (title === "" || task.uid === null || task.listKey === null) {
      return;
    }
    setText("");
    await actions.create({
      listKey: task.listKey,
      parentUid: task.uid,
      title,
    });
  };
  return (
    <Field label={m.tasks_subtasks()}>
      <div {...stylex.props(styles.subs)}>
        {children.length === 0 && !canEdit ? (
          <p {...stylex.props(styles.muted)}>{m.tasks_no_subtasks()}</p>
        ) : null}
        <ul
          aria-label={m.tasks_subtasks()}
          {...stylex.props(shared.resetList, styles.subs)}
        >
          {children.map((child) => {
            const title = cleanDisplayText(child.title) || m.tasks_no_title();
            return (
              <li key={child.id} {...stylex.props(styles.subRow)}>
                <Checkbox
                  aria-label={m.tasks_mark_done({ title })}
                  checked={child.completed}
                  disabled={!canEdit}
                  onCheckedChange={(done) => {
                    actions.complete(child, done);
                  }}
                  shape="round"
                />
                <button
                  onClick={() => {
                    onOpen(child);
                  }}
                  type="button"
                  {...stylex.props(
                    shared.resetButton,
                    shared.focusRing,
                    styles.subTitle,
                    child.completed ? styles.titleDone : null
                  )}
                >
                  <bdi>{title}</bdi>
                </button>
              </li>
            );
          })}
        </ul>
        {canEdit && task.uid !== null ? (
          <div {...stylex.props(styles.subRow)}>
            <Plus aria-hidden size={16} {...stylex.props(styles.linkIcon)} />
            <input
              aria-label={m.tasks_add_subtask()}
              dir="auto"
              maxLength={500}
              onChange={(event) => {
                setText(event.target.value);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.metaKey && !event.ctrlKey) {
                  event.preventDefault();
                  void add();
                }
              }}
              placeholder={m.tasks_add_subtask()}
              value={text}
              {...stylex.props(styles.subAdd)}
            />
          </div>
        ) : null}
      </div>
    </Field>
  );
};

const Links = ({ task }: { readonly task: Task }) => {
  const navigate = useNavigate();
  const links = useEventLinks();
  if (task.links.length === 0) {
    return null;
  }
  const { origin } = globalThis.location;
  return (
    <Field label={m.tasks_links()}>
      <ul
        aria-label={m.tasks_links()}
        {...stylex.props(shared.resetList, styles.links)}
      >
        {task.links.map((link) => {
          const kind = classifyLink(link.href, origin);
          const title = cleanDisplayText(link.title ?? "");
          if (kind.kind === "email") {
            return (
              <li key={link.key}>
                <button
                  onClick={() => {
                    void navigate({ href: kind.path });
                  }}
                  type="button"
                  {...stylex.props(
                    shared.resetButton,
                    shared.focusRing,
                    styles.link
                  )}
                >
                  <Mail
                    aria-hidden
                    size={16}
                    {...stylex.props(styles.linkIcon)}
                  />
                  <span {...stylex.props(styles.linkText, shared.ellipsis)}>
                    <bdi>{title === "" ? m.tasks_open_email() : title}</bdi>
                  </span>
                  <span {...stylex.props(styles.muted)}>
                    {m.tasks_open_email()}
                  </span>
                </button>
              </li>
            );
          }
          if (kind.kind === "web") {
            return (
              <li key={link.key}>
                <a
                  href={kind.href}
                  onClick={(event: MouseEvent<HTMLAnchorElement>) => {
                    event.preventDefault();
                    links.handleOpen(kind.href, title, event.currentTarget);
                  }}
                  rel="noopener noreferrer"
                  {...stylex.props(shared.focusRing, styles.link)}
                >
                  <ExternalLink
                    aria-hidden
                    size={16}
                    {...stylex.props(styles.linkIcon)}
                  />
                  <span {...stylex.props(styles.linkText, shared.ellipsis)}>
                    <bdi>{title === "" ? kind.href : title}</bdi>
                  </span>
                </a>
              </li>
            );
          }
          return (
            <li key={link.key} {...stylex.props(styles.link)}>
              {kind.kind === "mid" ? (
                <Hash
                  aria-hidden
                  size={16}
                  {...stylex.props(styles.linkIcon)}
                />
              ) : (
                <Link2
                  aria-hidden
                  size={16}
                  {...stylex.props(styles.linkIcon)}
                />
              )}
              <span {...stylex.props(styles.linkText, shared.ellipsis)}>
                <bdi>
                  {kind.kind === "mid"
                    ? m.tasks_message_id({
                        id: cleanDisplayText(kind.messageId),
                      })
                    : cleanDisplayText(link.href)}
                </bdi>
              </span>
            </li>
          );
        })}
      </ul>
    </Field>
  );
};

const ListField = ({
  canEdit,
  list,
  onMove,
  task,
  writable,
}: {
  readonly task: Task;
  readonly list: TaskList | undefined;
  readonly writable: readonly TaskList[];
  readonly canEdit: boolean;
  readonly onMove: (list: TaskList) => void;
}) => {
  if (writable.length <= 1 && list !== undefined) {
    return null;
  }
  // A list the task can't leave (read only) still shows as chosen.
  const choices =
    list === undefined || list.mayWrite ? writable : [list, ...writable];
  return (
    <Field label={m.tasks_list()}>
      <Select
        disabled={!canEdit}
        label={m.tasks_list()}
        onValueChange={(key) => {
          const next = writable.find((item) => item.key === key);
          if (next !== undefined) {
            onMove(next);
          }
        }}
        options={choices.map((item) => ({
          label: cleanDisplayText(item.name),
          value: item.key,
        }))}
        value={task.listKey ?? ""}
      />
    </Field>
  );
};

const PriorityField = ({
  onChange,
  value,
}: {
  readonly value: TaskPriority;
  readonly onChange: (priority: TaskPriority) => void;
}) => (
  <div {...stylex.props(styles.group)}>
    <span aria-hidden {...stylex.props(styles.groupLabel)}>
      {m.tasks_priority()}
    </span>
    <SegmentedControl
      label={m.tasks_priority()}
      onValueChange={onChange}
      options={PRIORITIES.map((priority) => ({
        label: priorityLabel(priority),
        value: priority,
      }))}
      value={value}
    />
  </div>
);

/** Whole, non-negative minutes typed in one box ("" or junk: 0). */
const typedNumber = (text: string): number => {
  const value = Math.trunc(Number(text));
  return Number.isFinite(value) && value > 0 ? value : 0;
};

/**
 * The estimated time (RFC 8984 §5.2.3) in hours and minutes. Saves when
 * focus leaves the two boxes or on Enter; 0 h 0 min (or ✕) removes it.
 * Keyed by the stored value, so a change from elsewhere shows at once.
 */
const EstimateField = ({
  disabled,
  minutes,
  onChange,
}: {
  readonly minutes: number | null;
  readonly disabled: boolean;
  readonly onChange: (minutes: number | null) => void;
}) => {
  const [hours, setHours] = useState(
    minutes === null ? "" : String(Math.floor(minutes / 60))
  );
  const [rest, setRest] = useState(
    minutes === null ? "" : String(minutes % 60)
  );
  const commit = () => {
    const total = typedNumber(hours) * 60 + typedNumber(rest);
    const next = total > 0 ? total : null;
    if (next !== minutes) {
      onChange(next);
    }
  };
  const boxes = useRef<HTMLDivElement>(null);
  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
    }
  };
  // Saves when focus leaves both boxes (not when it moves between them).
  const onBlur = (event: FocusEvent<HTMLInputElement>) => {
    if (!(boxes.current?.contains(event.relatedTarget) ?? false)) {
      commit();
    }
  };
  return (
    <fieldset {...stylex.props(fieldStyles.fieldset, styles.group)}>
      <legend {...stylex.props(styles.groupLabel)}>{m.tasks_estimate()}</legend>
      <div ref={boxes} {...stylex.props(fieldStyles.inline)}>
        <span {...stylex.props(styles.estimate)}>
          <Input
            aria-label={m.tasks_estimate_hours()}
            disabled={disabled}
            end={
              <span aria-hidden {...stylex.props(styles.unit)}>
                h
              </span>
            }
            inputMode="numeric"
            max={999}
            min={0}
            onChange={(event) => {
              setHours(event.target.value);
            }}
            onBlur={onBlur}
            onKeyDown={onKeyDown}
            placeholder="0"
            type="number"
            value={hours}
          />
        </span>
        <span {...stylex.props(styles.estimate)}>
          <Input
            aria-label={m.tasks_estimate_minutes()}
            disabled={disabled}
            end={
              <span aria-hidden {...stylex.props(styles.unit)}>
                min
              </span>
            }
            inputMode="numeric"
            max={59}
            min={0}
            onChange={(event) => {
              setRest(event.target.value);
            }}
            onBlur={onBlur}
            onKeyDown={onKeyDown}
            placeholder="0"
            step={5}
            type="number"
            value={rest}
          />
        </span>
        {minutes === null || disabled ? null : (
          <IconButton
            icon={X}
            label={m.tasks_estimate_clear()}
            onClick={() => {
              setHours("");
              setRest("");
              onChange(null);
            }}
            size="sm"
          />
        )}
      </div>
    </fieldset>
  );
};

const ReadOnlyNote = ({ list }: { readonly list: TaskList | undefined }) => (
  <p {...stylex.props(styles.note)}>
    {list === undefined || list.owner === null
      ? m.tasks_read_only()
      : m.tasks_read_only_shared({ owner: cleanDisplayText(list.owner) })}
  </p>
);

const Details = ({
  actions,
  list,
  lists,
  onClose,
  onOpen,
  task,
}: {
  readonly task: Task;
  readonly list: TaskList | undefined;
  readonly lists: readonly TaskList[];
  readonly actions: TaskActions;
  readonly onClose: () => void;
  readonly onOpen: (task: Task) => void;
}) => {
  const zone = useTimeZone();
  const locale = useDateLocale();
  const keys = useKeys();
  const now = useNow();
  const navigate = useNavigate();
  const canEdit = actions.canEdit(task);
  const series = actions.seriesOf(task);
  const titleRef = useRef<HTMLInputElement>(null);
  const title = useAutosave(task.title, (text) => {
    actions.setTitle(task, text);
  });
  const notes = useAutosave(task.notes ?? "", (text) => {
    actions.setNotes(task, text);
  });
  useTitleEditRequest(() => {
    titleRef.current?.focus();
    titleRef.current?.select();
  });

  const close = (): void => {
    title.flush();
    notes.flush();
    onClose();
  };
  useShortcut("mod+enter", (event) => {
    if (!isInOverlay(event.target)) {
      close();
    }
  });
  useShortcut("escape", (event) => {
    if (!isInOverlay(event.target)) {
      close();
    }
  });

  const recurrence = fromRule(
    series?.recurrenceRule,
    task.due?.kind === "date"
  );
  const minutes = alertMinutes(task);
  const kept = task.alerts.filter((alert) => alert.minutesBefore === null);
  const writable = lists.filter(
    (item) => item.mayWrite && item.accountId === task.accountId
  );
  const setAlerts = (next: readonly number[]): void => {
    const tied = task.alerts.filter((alert) => alert.minutesBefore !== null);
    const inputs: AlertInput[] = [
      ...next.map((value, index) => {
        const key = tied[index]?.key;
        return key === undefined
          ? { minutesBefore: value }
          : { key, minutesBefore: value };
      }),
      ...kept.flatMap((alert) =>
        alert.at === null ? [] : [{ at: alert.at, key: alert.key }]
      ),
    ];
    actions.setAlerts(task, inputs);
  };
  const shownTitle = cleanDisplayText(task.title) || m.tasks_no_title();

  return (
    <>
      <div {...stylex.props(panelStyles.header)}>
        <IconButton
          icon={ArrowLeft}
          label={m.tasks_back()}
          onClick={close}
          shortcut={keys("escape")}
        />
        <span {...stylex.props(panelStyles.spacer)} />
        <span aria-live="polite" {...stylex.props(styles.saving)}>
          {actions.saving ? m.tasks_saving() : ""}
        </span>
        {task.due === null ? null : (
          <DueChip locale={locale} now={now} task={task} zone={zone} />
        )}
        <Menu
          align="end"
          trigger={<IconButton icon={Ellipsis} label={m.more_actions()} />}
        >
          {task.due === null ? null : (
            <MenuItem
              icon={CalendarPlus}
              onClick={() => {
                void navigate({
                  search: {
                    date: dueDate(task.due),
                    view: "day",
                  },
                  to: "/calendar",
                });
              }}
            >
              {m.tasks_show_in_calendar()}
            </MenuItem>
          )}
          {canEdit ? (
            <MenuItem
              danger
              icon={Trash2}
              onClick={() => {
                onClose();
                actions.remove(task);
              }}
              shortcut={keys("#")}
            >
              {m.tasks_delete()}
            </MenuItem>
          ) : null}
        </Menu>
        <IconButton icon={X} label={m.close()} onClick={close} />
      </div>
      <div {...stylex.props(panelStyles.body)}>
        <div {...stylex.props(styles.titleRow)}>
          <Checkbox
            aria-label={m.tasks_mark_done({ title: shownTitle })}
            checked={task.completed}
            disabled={!canEdit}
            onCheckedChange={(done) => {
              actions.complete(task, done);
            }}
            shape="round"
            tone={priorityTone(task.priority)}
          />
          <input
            aria-label={m.tasks_title()}
            dir="auto"
            maxLength={500}
            onBlur={() => {
              title.flush();
            }}
            onChange={(event) => {
              title.set(event.target.value);
            }}
            onKeyDown={(event: ReactKeyboardEvent<HTMLInputElement>) => {
              if (event.key === "Enter" && !event.metaKey && !event.ctrlKey) {
                event.preventDefault();
                title.flush();
              }
            }}
            placeholder={m.tasks_no_title()}
            readOnly={!canEdit}
            ref={titleRef}
            value={title.value}
            {...stylex.props(
              styles.titleInput,
              task.completed ? styles.titleDone : null
            )}
          />
        </div>
        {canEdit ? null : <ReadOnlyNote list={list} />}
        {task.recurring ? (
          <p {...stylex.props(styles.note)}>{m.tasks_repeating_note()}</p>
        ) : null}
        <ListField
          canEdit={canEdit}
          list={list}
          onMove={(next) => {
            actions.moveTo(task, next.id);
          }}
          task={task}
          writable={writable}
        />
        <DueField
          disabled={!canEdit}
          now={now}
          onChange={(due) => {
            actions.setDue(task, due);
          }}
          task={task}
          zone={zone}
        />
        <PriorityField
          onChange={(priority) => {
            if (canEdit) {
              actions.setPriority(task, priority);
            }
          }}
          value={task.priority}
        />
        <EstimateField
          disabled={!canEdit}
          key={`${task.baseId}:${String(task.estimatedMinutes)}`}
          minutes={task.estimatedMinutes}
          onChange={(next) => {
            actions.setEstimate(task, next);
          }}
        />
        <RepeatField
          disabled={!canEdit || task.due === null}
          onChange={(next) => {
            actions.setRecurrence(
              task,
              toRule(next, task.due?.kind === "date")
            );
          }}
          recurrence={recurrence}
          startDate={dueDate(task.due) ?? ""}
          {...(task.due === null ? { hint: m.tasks_repeat_needs_due() } : {})}
        />
        {task.due?.kind === "date" ? (
          <DayRemindersField
            alerts={minutes}
            disabled={!canEdit}
            label={m.tasks_reminders()}
            locale={locale}
            onChange={setAlerts}
          />
        ) : (
          <RemindersField
            alerts={minutes}
            disabled={!canEdit || task.due === null}
            label={m.tasks_reminders()}
            onChange={setAlerts}
            textFor={taskAlertText}
            {...(task.due === null && kept.length === 0
              ? { hint: m.tasks_reminders_need_due() }
              : {})}
          />
        )}
        <AssigneesField
          actions={actions}
          canEdit={canEdit}
          series={series}
          task={task}
        />
        <Field label={m.tasks_notes()}>
          <textarea
            dir="auto"
            maxLength={20_000}
            onBlur={() => {
              notes.flush();
            }}
            onChange={(event) => {
              notes.set(event.target.value);
            }}
            placeholder={m.tasks_notes_placeholder()}
            readOnly={!canEdit}
            value={notes.value}
            {...stylex.props(styles.textarea)}
          />
        </Field>
        <Subtasks
          actions={actions}
          canEdit={canEdit}
          onOpen={onOpen}
          task={task}
        />
        <Links task={task} />
      </div>
    </>
  );
};

export const TaskPanel = ({
  actions,
  onClose,
  onOpen,
  open,
}: {
  readonly open: { readonly accountId: string; readonly id: string };
  readonly actions: TaskActions;
  readonly onClose: () => void;
  readonly onOpen: (task: Task) => void;
}) => {
  const { snapshot } = actions;
  if (snapshot === undefined) {
    return (
      <div {...stylex.props(panelStyles.center)}>
        <Spinner label={m.tasks_loading()} />
      </div>
    );
  }
  // By its id, or (a task that now repeats, a link to a series) its next
  // occurrence.
  const task =
    snapshot.tasks.find(
      (item) => item.id === open.id && item.accountId === open.accountId
    ) ??
    snapshot.tasks
      .filter(
        (item) => item.baseId === open.id && item.accountId === open.accountId
      )
      .toSorted(
        (a, b) =>
          Number(a.completed) - Number(b.completed) ||
          (a.dueAt ?? 0) - (b.dueAt ?? 0)
      )[0];
  if (task === undefined) {
    return (
      <>
        <div {...stylex.props(panelStyles.header)}>
          <IconButton
            icon={ArrowLeft}
            label={m.tasks_back()}
            onClick={onClose}
          />
        </div>
        <div {...stylex.props(panelStyles.center)}>
          <p>{m.tasks_gone()}</p>
        </div>
      </>
    );
  }
  return (
    <Details
      actions={actions}
      key={`${task.accountId}/${task.baseId}`}
      list={snapshot.lists.find((item) => item.key === task.listKey)}
      lists={snapshot.lists}
      onClose={onClose}
      onOpen={onOpen}
      task={task}
    />
  );
};
