// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// More than Accept / Decline for a task someone assigned to the user
//propose another due date to the
// organizer (COUNTER, RFC 5546 §3.4.7) or pass the task on to someone else
// (delegating), with an optional note. Hidden behind one button, like the
// event's (components/calendar/reply-options.tsx). The date and time are
// picked in the user's zone.
//
// Compound: <TaskMoreOptions> holds the shared note; the caller composes
// the parts it offers (<ProposeDueOption>, <DelegateOption>) inside.

import type { CalendarDate, Time } from "@internationalized/date";
import { Time as TimeValue, fromAbsolute } from "@internationalized/date";
import { cleanDisplayText } from "@nebulamail/mail-render";
import {
  Avatar,
  Button,
  DatePicker,
  Field,
  IconButton,
  Textarea,
  TimeField,
} from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import {
  CalendarClock,
  ChevronDown,
  ChevronUp,
  Forward,
  X,
} from "lucide-react";
import type { ReactNode } from "react";
import { createContext, use, useId, useState } from "react";

import { useTimeZone } from "../../calendar/hooks";
import { MAX_COMMENT } from "../../calendar/itip";
import { dateInZone, parseLocalDate } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import type { AssigneeInput } from "../../tasks/assignment";
import type { TaskDue } from "../../tasks/model";
import { dueInstant } from "../../tasks/model";
import { useWeekStart } from "../calendar/use-calendar-prefs";
import { shared } from "../mail/styles";
import { AssignInput } from "./assign-input";

const styles = stylex.create({
  box: {
    backgroundColor: colors.subtle,
    borderRadius: radius.lg,
    display: "flex",
    flexDirection: "column",
    marginTop: "8px",
    paddingBlock: "10px",
    paddingInline: "12px",
    rowGap: "10px",
  },
  chip: {
    alignItems: "center",
    backgroundColor: colors.hover,
    borderRadius: radius.pill,
    columnGap: "6px",
    display: "inline-flex",
    fontSize: font.sizeSm,
    maxWidth: "100%",
    minHeight: "28px",
    paddingInlineEnd: "2px",
    paddingInlineStart: "4px",
  },
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  legend: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    marginBottom: "6px",
    padding: 0,
  },
  part: {
    borderStyle: "none",
    display: "flex",
    flexDirection: "column",
    margin: 0,
    minWidth: 0,
    padding: 0,
    rowGap: "8px",
  },
  row: {
    alignItems: "flex-end",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "8px",
  },
});

/** Where the proposal starts: the current due in the user's zone. */
const startOf = (
  due: TaskDue | null,
  zone: string
): { readonly date: CalendarDate | null; readonly time: Time | null } => {
  if (due === null) {
    return { date: dateInZone(Date.now(), zone), time: null };
  }
  if (due.kind === "date") {
    return { date: parseLocalDate(due.date), time: null };
  }
  const at = dueInstant(due, zone);
  if (at === null) {
    return { date: null, time: null };
  }
  const local = fromAbsolute(at, zone);
  return {
    date: dateInZone(at, zone),
    time: new TimeValue(local.hour, local.minute),
  };
};

const pad = (value: number): string => String(value).padStart(2, "0");

/** The due picked: a day, or a day and time in the user's zone. */
export const proposedDue = (
  date: CalendarDate | null,
  time: Time | null,
  zone: string
): TaskDue | null => {
  if (date === null) {
    return null;
  }
  if (time === null) {
    return { date: date.toString(), kind: "date" };
  }
  return {
    kind: "datetime",
    local: `${date.toString()}T${pad(time.hour)}:${pad(time.minute)}:00`,
    timeZone: zone,
  };
};

/** What the parts share: who assigned the task and the user's note. */
interface MoreOptions {
  /** Who assigned the task (clean text, for the hints). */
  readonly organizer: string;
  /** The note, trimmed; undefined when empty. */
  readonly note: () => string | undefined;
}

const MoreOptionsContext = createContext<MoreOptions | null>(null);

const useMoreOptions = (): MoreOptions => {
  const value = use(MoreOptionsContext);
  if (value === null) {
    throw new Error("Task option parts must be inside TaskMoreOptions");
  }
  return value;
};

/** Pass the task on to someone else (delegating). */
export const DelegateOption = ({
  busy,
  exclude,
  onDelegate,
  repeating = false,
}: {
  /** Addresses that can't be the delegate (already in the task, own). */
  readonly exclude: readonly string[];
  /** A repeating task: passing it on passes the whole series. */
  readonly repeating?: boolean;
  readonly busy: boolean;
  readonly onDelegate: (
    person: AssigneeInput,
    comment: string | undefined
  ) => void;
}) => {
  const { note, organizer } = useMoreOptions();
  const hint = repeating
    ? m.tasks_delegate_hint_series({ name: organizer })
    : m.tasks_delegate_hint({ name: organizer });
  const id = useId();
  const [person, setPerson] = useState<AssigneeInput | null>(null);
  const [invalid, setInvalid] = useState(false);
  const name = cleanDisplayText(person?.name ?? person?.email ?? "");
  return (
    <fieldset {...stylex.props(styles.part)}>
      <legend {...stylex.props(styles.legend)}>
        <label htmlFor={id}>{m.tasks_delegate()}</label>
      </legend>
      {person === null ? (
        <AssignInput
          exclude={exclude}
          hint={hint}
          id={id}
          onAdd={(people) => {
            const [first] = people;
            if (people.length === 1 && first !== undefined) {
              setPerson(first);
              setInvalid(false);
            } else if (people.length > 1) {
              setInvalid(true);
            }
          }}
        />
      ) : (
        <div {...stylex.props(styles.row)}>
          <span title={person.email} {...stylex.props(styles.chip)}>
            <Avatar colorKey={person.email} name={name} size={20} />
            <span {...stylex.props(shared.ellipsis)}>
              <bdi>{name}</bdi>
            </span>
            <IconButton
              icon={X}
              label={m.tasks_assign_remove({ name })}
              onClick={() => {
                setPerson(null);
              }}
              size="xs"
              tooltip={false}
            />
          </span>
          <Button
            icon={Forward}
            loading={busy}
            onClick={() => {
              onDelegate(person, note());
            }}
            size="sm"
            variant="secondary"
          >
            {m.tasks_delegate_send()}
          </Button>
        </div>
      )}
      {invalid ? (
        <p role="alert" {...stylex.props(styles.error)}>
          {m.tasks_delegate_invalid()}
        </p>
      ) : null}
    </fieldset>
  );
};

/** Propose another due date to the organizer (COUNTER). */
export const ProposeDueOption = ({
  busy,
  due,
  onPropose,
}: {
  /** The current due (the proposal starts from it). */
  readonly due: TaskDue | null;
  readonly busy: boolean;
  readonly onPropose: (due: TaskDue, comment: string | undefined) => void;
}) => {
  const { note } = useMoreOptions();
  const zone = useTimeZone();
  const [, firstDayOfWeek] = useWeekStart();
  const [date, setDate] = useState<CalendarDate | null>(
    () => startOf(due, zone).date
  );
  const [time, setTime] = useState<Time | null>(() => startOf(due, zone).time);
  const [invalid, setInvalid] = useState(false);
  const weekStart = firstDayOfWeek === undefined ? {} : { firstDayOfWeek };
  return (
    <fieldset {...stylex.props(styles.part)}>
      <legend {...stylex.props(styles.legend)}>{m.tasks_propose_due()}</legend>
      <div {...stylex.props(styles.row)}>
        <DatePicker
          calendarLabel={m.cal_pick_date()}
          isInvalid={invalid}
          label={m.tasks_propose_date()}
          nextLabel={m.cal_next_month()}
          onChange={(value) => {
            setDate(value);
            setInvalid(false);
          }}
          previousLabel={m.cal_previous_month()}
          value={date}
          {...weekStart}
        />
        <TimeField
          label={m.tasks_propose_time()}
          onChange={setTime}
          value={time}
        />
        <Button
          icon={CalendarClock}
          loading={busy}
          onClick={() => {
            const picked = proposedDue(date, time, zone);
            if (picked === null) {
              setInvalid(true);
            } else {
              onPropose(picked, note());
            }
          }}
          size="sm"
          variant="secondary"
        >
          {m.tasks_propose_send()}
        </Button>
      </div>
      {invalid ? (
        <p role="alert" {...stylex.props(styles.error)}>
          {m.tasks_propose_invalid()}
        </p>
      ) : null}
    </fieldset>
  );
};

/**
 * The "More" button and its box: the note for the organizer, then the
 * parts the caller puts inside.
 */
export const TaskMoreOptions = ({
  children,
  organizer,
}: {
  /** Who assigned the task (clean text, for the hints). */
  readonly organizer: string;
  /** ProposeDueOption, DelegateOption. */
  readonly children: ReactNode;
}) => {
  const [open, setOpen] = useState(false);
  const [comment, setComment] = useState("");
  const note = (): string | undefined => {
    const text = comment.trim();
    return text === "" ? undefined : text;
  };
  return (
    <div>
      <Button
        aria-expanded={open}
        iconEnd={open ? ChevronUp : ChevronDown}
        onClick={() => {
          setOpen(!open);
        }}
        size="sm"
        variant="ghost"
      >
        {m.tasks_more()}
      </Button>
      {open ? (
        <div {...stylex.props(styles.box)}>
          <Field label={m.tasks_more_note({ name: organizer })}>
            <Textarea
              dir="auto"
              maxLength={MAX_COMMENT}
              onChange={(event) => {
                setComment(event.target.value);
              }}
              rows={2}
              value={comment}
            />
          </Field>
          <p {...stylex.props(styles.hint)}>{m.tasks_more_note_hint()}</p>
          <MoreOptionsContext
            // oxlint-disable-next-line react/jsx-no-constructed-context-values -- the React Compiler memoizes this object and its function.
            value={{ note, organizer }}
          >
            {children}
          </MoreOptionsContext>
        </div>
      ) : null}
    </div>
  );
};
