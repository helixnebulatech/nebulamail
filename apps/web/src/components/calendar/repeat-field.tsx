// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// "Repeat" and "Reminders" of the event editor, also used by the task
// panel (the same repetition and reminder choices).

import type { CalendarDate } from "@internationalized/date";
import { parseDate } from "@internationalized/date";
import { Button, DatePicker, Field, IconButton, Select } from "@nebulamail/ui";
import {
  colors,
  density,
  font,
  radius,
  touch,
} from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { Bell, Plus, X } from "lucide-react";

import type {
  Frequency,
  Recurrence,
  RecurrenceEnd,
  RecurrencePreset,
  Weekday,
} from "../../calendar/recurrence";
import {
  WEEKDAYS,
  describeRecurrence,
  fromPreset,
  presetOf,
  weekdayNames,
  weekdayOf,
} from "../../calendar/recurrence";
import { m } from "../../paraglide/messages.js";
import { shared } from "../mail/styles";
import { alertText, recurrenceWords } from "./format";
import { useDateLocale } from "./use-calendar-prefs";

export const fieldStyles = stylex.create({
  dayToggle: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: colors.bg },
    borderColor: colors.line2,
    borderRadius: radius.pill,
    borderStyle: "solid",
    borderWidth: "1px",
    cursor: "pointer",
    display: "inline-flex",
    fontSize: font.sizeSm,
    height: { default: "30px", [touch.coarse]: touch.target },
    justifyContent: "center",
    minWidth: { default: "30px", [touch.coarse]: touch.target },
  },
  dayToggleOn: {
    backgroundColor: { ":hover": colors.accHi, default: colors.acc },
    borderColor: colors.acc,
    color: colors.accFg,
  },
  fieldset: { borderStyle: "none", margin: 0, padding: 0 },
  form: { display: "flex", flexDirection: "column", rowGap: "14px" },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  inline: {
    alignItems: "center",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "8px",
  },
  number: { width: "72px" },
  select: {
    appearance: "auto",
    backgroundColor: colors.bg,
    borderColor: { ":hover": colors.fg3, default: colors.line2 },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    color: colors.fg,
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    height: { default: density.input, [touch.coarse]: touch.target },
    maxWidth: "100%",
    minWidth: 0,
    paddingInline: "8px",
  },
});

const styles = fieldStyles;

const safeDate = (value: string): CalendarDate | null => {
  try {
    return parseDate(value);
  } catch {
    return null;
  }
};

const ALERT_CHOICES = [0, 5, 10, 15, 30, 60, 120, 1440, 2880, 10_080] as const;
const PRESETS: readonly RecurrencePreset[] = [
  "none",
  "daily",
  "weekdays",
  "weekly",
  "monthly",
  "yearly",
  "custom",
];
const FREQUENCIES: readonly Frequency[] = [
  "daily",
  "weekly",
  "monthly",
  "yearly",
];

const presetLabel = (preset: RecurrencePreset): string => {
  switch (preset) {
    case "none": {
      return m.cal_repeat_none();
    }
    case "daily": {
      return m.cal_repeat_daily();
    }
    case "weekdays": {
      return m.cal_repeat_weekdays();
    }
    case "weekly": {
      return m.cal_repeat_weekly();
    }
    case "monthly": {
      return m.cal_repeat_monthly();
    }
    case "yearly": {
      return m.cal_repeat_yearly();
    }
    case "custom": {
      return m.cal_repeat_custom();
    }
    default: {
      return preset satisfies never;
    }
  }
};

const frequencyLabel = (frequency: Frequency): string => {
  switch (frequency) {
    case "daily": {
      return m.cal_unit_days();
    }
    case "weekly": {
      return m.cal_unit_weeks();
    }
    case "monthly": {
      return m.cal_unit_months();
    }
    case "yearly": {
      return m.cal_unit_years();
    }
    default: {
      return frequency satisfies never;
    }
  }
};

const clampNumber = (value: string, max: number): number =>
  Math.max(1, Math.min(max, Math.trunc(Number(value)) || 1));

const endFor = (kind: string, startDate: string): RecurrenceEnd => {
  if (kind === "count") {
    return { count: 10, kind: "count" };
  }
  if (kind === "until") {
    const start = safeDate(startDate) ?? parseDate("2026-01-01");
    return { date: start.add({ months: 3 }).toString(), kind: "until" };
  }
  return { kind: "never" };
};

type SimpleRecurrence = Extract<Recurrence, { kind: "simple" }>;

const CustomRepeat = ({
  recurrence,
  setRecurrence,
  startDate,
}: {
  readonly recurrence: SimpleRecurrence;
  readonly setRecurrence: (next: Recurrence) => void;
  readonly startDate: string;
}) => {
  const locale = useDateLocale();
  const weekdays = weekdayNames(locale, "short");
  const start = safeDate(startDate);
  const { end } = recurrence;
  return (
    <>
      <div {...stylex.props(styles.inline)}>
        <span>{m.cal_every()}</span>
        <input
          aria-label={m.cal_interval()}
          max={99}
          min={1}
          onChange={(event) => {
            setRecurrence({
              ...recurrence,
              interval: clampNumber(event.target.value, 99),
            });
          }}
          type="number"
          value={recurrence.interval}
          {...stylex.props(styles.select, styles.number)}
        />
        <Select
          label={m.cal_frequency()}
          onValueChange={(frequency) => {
            setRecurrence({ ...recurrence, frequency });
          }}
          options={FREQUENCIES.map((frequency) => ({
            label: frequencyLabel(frequency),
            value: frequency,
          }))}
          value={recurrence.frequency}
          width="fit"
        />
      </div>
      {recurrence.frequency === "weekly" ? (
        <fieldset {...stylex.props(styles.inline, styles.fieldset)}>
          <legend {...stylex.props(shared.srOnly)}>
            {m.cal_on_weekdays()}
          </legend>
          {WEEKDAYS.map((day: Weekday, index) => {
            const on = recurrence.byDay.includes(day);
            return (
              <button
                aria-pressed={on}
                key={day}
                onClick={() => {
                  setRecurrence({
                    ...recurrence,
                    byDay: on
                      ? recurrence.byDay.filter((item) => item !== day)
                      : [...recurrence.byDay, day],
                  });
                }}
                type="button"
                {...stylex.props(
                  shared.resetButton,
                  shared.focusRing,
                  styles.dayToggle,
                  on ? styles.dayToggleOn : null
                )}
              >
                {weekdays[index]}
              </button>
            );
          })}
        </fieldset>
      ) : null}
      <div {...stylex.props(styles.inline)}>
        <Select
          label={m.cal_ends_repeat()}
          onValueChange={(kind) => {
            setRecurrence({
              ...recurrence,
              end: endFor(kind, startDate),
            });
          }}
          options={[
            { label: m.cal_end_never(), value: "never" },
            { label: m.cal_end_count(), value: "count" },
            { label: m.cal_end_until(), value: "until" },
          ]}
          value={end.kind}
          width="fit"
        />
        {end.kind === "count" ? (
          <input
            aria-label={m.cal_occurrences()}
            max={999}
            min={1}
            onChange={(event) => {
              setRecurrence({
                ...recurrence,
                end: {
                  count: clampNumber(event.target.value, 999),
                  kind: "count",
                },
              });
            }}
            type="number"
            value={end.count}
            {...stylex.props(styles.select, styles.number)}
          />
        ) : null}
        {end.kind === "until" ? (
          <DatePicker
            calendarLabel={m.cal_pick_date()}
            hideLabel
            label={m.cal_end_until()}
            nextLabel={m.cal_next_month()}
            onChange={(date) => {
              if (date !== null) {
                setRecurrence({
                  ...recurrence,
                  end: { date: date.toString(), kind: "until" },
                });
              }
            }}
            previousLabel={m.cal_previous_month()}
            value={safeDate(end.date)}
            {...(start === null ? {} : { minValue: start })}
          />
        ) : null}
      </div>
    </>
  );
};

/**
 * "Repeat": the quick choices, a custom rule (interval, weekdays, end) and
 * the rule in words. A rule the editor can't show is kept as it is.
 * Shared by the event editor and the task panel.
 */
export const RepeatField = ({
  disabled = false,
  hint,
  onChange,
  recurrence,
  startDate,
}: {
  readonly recurrence: Recurrence;
  readonly onChange: (next: Recurrence) => void;
  /** "YYYY-MM-DD" the repetition starts (weekday, "until" minimum). */
  readonly startDate: string;
  readonly disabled?: boolean;
  /** Shown under the choice (e.g. why it is disabled). */
  readonly hint?: string;
}) => {
  const locale = useDateLocale();
  const preset = presetOf(recurrence);
  const setRecurrence = onChange;
  const choosePreset = (value: RecurrencePreset): void => {
    if (value !== "custom") {
      setRecurrence(fromPreset(value, recurrence));
      return;
    }
    setRecurrence(
      recurrence.kind === "simple"
        ? recurrence
        : {
            byDay: [weekdayOf(startDate) ?? "mo"],
            end: { kind: "never" },
            frequency: "weekly",
            interval: 1,
            kind: "simple",
          }
    );
  };
  return (
    <Field label={m.cal_repeat()}>
      <div {...stylex.props(styles.form)}>
        {recurrence.kind === "custom" ? (
          <p {...stylex.props(styles.hint)}>{m.cal_repeat_kept()}</p>
        ) : null}
        <Select
          disabled={disabled}
          label={m.cal_repeat()}
          onValueChange={choosePreset}
          options={PRESETS.map((item) => ({
            label: presetLabel(item),
            value: item,
          }))}
          value={preset}
          width="fit"
        />
        {recurrence.kind === "simple" && preset === "custom" ? (
          <CustomRepeat
            recurrence={recurrence}
            setRecurrence={setRecurrence}
            startDate={startDate}
          />
        ) : null}
        {hint === undefined ? null : (
          <p {...stylex.props(styles.hint)}>{hint}</p>
        )}
        {recurrence.kind === "none" ? null : (
          <p {...stylex.props(styles.hint)}>
            {describeRecurrence(recurrence, locale, recurrenceWords)}
          </p>
        )}
      </div>
    </Field>
  );
};

/**
 * Reminders as minutes before the start (or a task's due): up to five,
 * each from the usual choices. Shared by the event editor and the task
 * panel (`textFor` words them for a task).
 */
export const RemindersField = ({
  alerts,
  disabled = false,
  hint,
  label,
  onChange,
  textFor = alertText,
}: {
  readonly alerts: readonly number[];
  readonly onChange: (next: number[]) => void;
  readonly label: string;
  readonly textFor?: (minutes: number) => string;
  readonly disabled?: boolean;
  readonly hint?: string;
}) => (
  <Field label={label}>
    <div {...stylex.props(styles.form)}>
      {alerts.map((minutes, index) => (
        // oxlint-disable-next-line react/no-array-index-key -- a reminder has no id; its slot is the index.
        <div key={index} {...stylex.props(styles.inline)}>
          <Bell aria-hidden size={16} />
          <Select
            disabled={disabled}
            label={m.cal_reminder()}
            onValueChange={(choice) => {
              const value = Number(choice);
              onChange(alerts.map((item, at) => (at === index ? value : item)));
            }}
            options={[...new Set([minutes, ...ALERT_CHOICES])].map(
              (choice) => ({ label: textFor(choice), value: String(choice) })
            )}
            value={String(minutes)}
            width="fit"
          />
          <IconButton
            icon={X}
            label={m.cal_remove_reminder()}
            onClick={() => {
              onChange(alerts.filter((_, at) => at !== index));
            }}
            size="sm"
            tooltip={false}
          />
        </div>
      ))}
      {hint === undefined ? null : <p {...stylex.props(styles.hint)}>{hint}</p>}
      {alerts.length < 5 && !disabled ? (
        <div>
          <Button
            icon={Plus}
            onClick={() => {
              onChange([...alerts, 10]);
            }}
            size="sm"
            variant="ghost"
          >
            {m.cal_add_reminder()}
          </Button>
        </div>
      ) : null}
    </div>
  </Field>
);
