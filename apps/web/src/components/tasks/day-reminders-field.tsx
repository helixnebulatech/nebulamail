// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Reminders of a task due on a day without a time: each one
// is a day ("On the day", "1 day before"…) and a time ("9:00"). New ones
// take the last time the user chose (a synced preference, 9:00 at first).

import { Time } from "@internationalized/date";
import { Button, Field, IconButton, Select, TimeField } from "@nebulamail/ui";
import * as stylex from "@stylexjs/stylex";
import { Bell, Plus, X } from "lucide-react";
import { useEffect, useEffectEvent, useState } from "react";

import { formatTime } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { recordChange, syncedValue } from "../../prefs-sync/sync";
import type { DayReminder } from "../../tasks/day-reminders";
import {
  DAY_REMINDER_CHOICES,
  dayReminderOf,
  minutesBeforeOf,
  reminderMinutes,
} from "../../tasks/day-reminders";
import { fieldStyles as styles } from "../calendar/repeat-field";

/** The time new day reminders get. */
export const defaultReminderMinutes = (): number =>
  reminderMinutes(syncedValue("taskReminderTime"));

/** "On the day", "1 day before", "1 week before", "2 days after". */
export const reminderDayText = (daysBefore: number): string => {
  if (daysBefore === 0) {
    return m.tasks_day_alert_same_day();
  }
  if (daysBefore === 7) {
    return m.tasks_day_alert_week();
  }
  return daysBefore > 0
    ? m.tasks_day_alert_before({ count: daysBefore })
    : m.tasks_day_alert_after({ count: -daysBefore });
};

/** "1 day before at 9:00" (the time in the user's locale). */
export const dayReminderText = (
  reminder: DayReminder,
  locale: string
): string =>
  m.tasks_day_alert({
    day: reminderDayText(reminder.daysBefore),
    // A wall time: any zone works as long as it's the same both ways.
    time: formatTime(
      Date.UTC(
        2026,
        0,
        1,
        Math.floor(reminder.minutes / 60),
        reminder.minutes % 60
      ),
      "UTC",
      locale
    ),
  });

/** How long after the last keystroke a typed time is saved. */
const TIME_SAVE_MS = 700;

/**
 * The time of one reminder. Saved a moment after typing stops: a time is
 * typed digit by digit ("1", "18"), and saving each would cut it short.
 */
const ReminderTime = ({
  disabled,
  minutes,
  onChange,
}: {
  readonly minutes: number;
  readonly disabled: boolean;
  readonly onChange: (minutes: number) => void;
}) => {
  const [draft, setDraft] = useState<number | null>(null);
  const save = useEffectEvent((value: number) => {
    setDraft(null);
    if (value !== minutes) {
      onChange(value);
    }
  });
  useEffect(() => {
    const value = draft;
    const timer =
      value === null
        ? null
        : setTimeout(() => {
            save(value);
          }, TIME_SAVE_MS);
    return () => {
      if (timer !== null) {
        clearTimeout(timer);
      }
    };
  }, [draft]);
  const shown = draft ?? minutes;
  return (
    <TimeField
      hideLabel
      isDisabled={disabled}
      label={m.tasks_reminder_time()}
      onChange={(time) => {
        if (time === null) {
          return;
        }
        setDraft(time.hour * 60 + time.minute);
      }}
      value={new Time(Math.floor(shown / 60), shown % 60)}
    />
  );
};

export const DayRemindersField = ({
  alerts,
  disabled = false,
  hint,
  label,
  locale,
  onChange,
}: {
  /** Minutes before the due midnight. */
  readonly alerts: readonly number[];
  readonly onChange: (next: number[]) => void;
  readonly label: string;
  readonly locale: string;
  readonly disabled?: boolean;
  readonly hint?: string;
}) => {
  const set = (index: number, reminder: DayReminder): void => {
    onChange(
      alerts.map((item, at) =>
        at === index ? minutesBeforeOf(reminder) : item
      )
    );
  };
  return (
    <Field label={label}>
      <div {...stylex.props(styles.form)}>
        {alerts.map((minutes, index) => {
          const reminder = dayReminderOf(minutes);
          const days = [
            ...new Set([...DAY_REMINDER_CHOICES, reminder.daysBefore]),
          ].toSorted((a, b) => a - b);
          return (
            <fieldset
              aria-label={dayReminderText(reminder, locale)}
              // oxlint-disable-next-line react/no-array-index-key -- a reminder has no id; its slot is the index.
              key={index}
              {...stylex.props(styles.fieldset, styles.inline)}
            >
              <Bell aria-hidden size={16} />
              <Select
                disabled={disabled}
                label={m.tasks_reminder_day()}
                onValueChange={(choice) => {
                  const daysBefore = Number(choice);
                  if (Number.isInteger(daysBefore)) {
                    set(index, { ...reminder, daysBefore });
                  }
                }}
                options={days.map((choice) => ({
                  label: reminderDayText(choice),
                  value: String(choice),
                }))}
                value={String(reminder.daysBefore)}
                width="fit"
              />
              <ReminderTime
                disabled={disabled}
                minutes={reminder.minutes}
                onChange={(next) => {
                  // The next reminders start from this time.
                  recordChange("taskReminderTime", next);
                  set(index, { ...reminder, minutes: next });
                }}
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
            </fieldset>
          );
        })}
        {hint === undefined ? null : (
          <p {...stylex.props(styles.hint)}>{hint}</p>
        )}
        {alerts.length < 5 && !disabled ? (
          <div>
            <Button
              icon={Plus}
              onClick={() => {
                onChange([
                  ...alerts,
                  minutesBeforeOf({
                    daysBefore: 0,
                    minutes: defaultReminderMinutes(),
                  }),
                ]);
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
};
