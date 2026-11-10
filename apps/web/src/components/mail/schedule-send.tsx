// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Choosing when a message goes out: the
// "Pick date and time" dialog, also used to change the time of a scheduled
// message. Times are in the user's time zone; anything past the server's
// limit is disabled and explained. The send menu's quick times are in
// schedule-menu.tsx, so this dialog (and its date picker) loads only when
// it opens.

import type { CalendarDate, Time } from "@internationalized/date";
import {
  Time as TimeValue,
  fromAbsolute,
  toCalendarDate,
} from "@internationalized/date";
import {
  Button,
  DatePicker,
  DateLocale,
  Dialog,
  TimeField,
} from "@nebulamail/ui";
import { colors, font } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { CalendarClock, Clock } from "lucide-react";
import { useState } from "react";

import { browserTimeZone } from "../../calendar/time";
import type { ScheduleLimit, ScheduleProblem } from "../../mail/schedule";
import {
  checkSendAt,
  formatSendAt,
  latestAllowed,
  pickedTime,
  quickTimes,
} from "../../mail/schedule";
import { m } from "../../paraglide/messages.js";
import { getLocale } from "../../paraglide/runtime.js";
import { useDateLocale, useWeekStart } from "../calendar/use-calendar-prefs";
import { quickLabel, scheduleChoices } from "./schedule-menu";

const styles = stylex.create({
  body: { display: "flex", flexDirection: "column", rowGap: "12px" },
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  fields: {
    alignItems: "flex-end",
    columnGap: "10px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "8px",
  },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  quick: { display: "flex", flexDirection: "column", rowGap: "4px" },
  quickTime: { color: colors.fg3, marginInlineStart: "auto" },
});

const PROBLEM_TEXT: Readonly<
  Record<ScheduleProblem, (latest: string) => string>
> = {
  invalid: () => m.schedule_invalid(),
  past: () => m.schedule_past(),
  tooFar: (latest) => m.schedule_too_far({ date: latest }),
};

export interface ScheduleDialogProps {
  /**
   * When it was opened (null: closed). The quick times and the limit are
   * counted from it.
   */
  readonly openedAt: Date | null;
  readonly onClose: () => void;
  readonly limit: ScheduleLimit;
  readonly title: string;
  readonly submitLabel: string;
  /** Preselected time, ISO (the current one when changing it). */
  readonly initial?: string;
  readonly busy?: boolean;
  readonly onSubmit: (at: Date) => void;
}

/** Where the fields start: the given time, else the first quick one. */
const startTime = (
  initial: string | undefined,
  now: Date,
  zone: string
): Date => {
  const given = initial === undefined ? Number.NaN : Date.parse(initial);
  return Number.isNaN(given)
    ? (quickTimes(now, zone)[0]?.at ?? now)
    : new Date(given);
};

/** Checked against the clock when submitting, not when the dialog opened. */
const checkNow = (
  at: Date | null,
  limit: ScheduleLimit
): ScheduleProblem | null =>
  at === null ? "invalid" : checkSendAt(at, new Date(), limit);

/** The date and time fields, with the quick times as shortcuts. */
const ScheduleForm = ({
  busy = false,
  initial,
  limit,
  now,
  onSubmit,
  submitLabel,
}: Omit<ScheduleDialogProps, "openedAt" | "onClose" | "title"> & {
  readonly now: Date;
}) => {
  const zone = browserTimeZone();
  const locale = getLocale();
  const dateLocale = useDateLocale();
  const [, firstDayOfWeek] = useWeekStart();
  const start = startTime(initial, now, zone);
  const zoned = fromAbsolute(start.getTime(), zone);
  const [date, setDate] = useState<CalendarDate | null>(toCalendarDate(zoned));
  const [time, setTime] = useState<Time | null>(
    new TimeValue(zoned.hour, zoned.minute)
  );
  const [problem, setProblem] = useState<ScheduleProblem | null>(null);
  const latest = formatSendAt(latestAllowed(now, limit), locale);

  const submit = (at: Date | null): void => {
    const found = checkNow(at, limit);
    setProblem(found);
    if (found === null && at !== null) {
      onSubmit(at);
    }
  };

  return (
    <div {...stylex.props(styles.body)}>
      <div {...stylex.props(styles.quick)}>
        {scheduleChoices(now, limit).map((choice) => (
          <Button
            disabled={!choice.allowed || busy}
            icon={Clock}
            key={choice.id}
            onClick={() => {
              submit(choice.at);
            }}
            size="sm"
            variant="ghost"
          >
            {quickLabel(choice.id)}
            <span {...stylex.props(styles.quickTime)}>
              {formatSendAt(choice.at, locale)}
            </span>
          </Button>
        ))}
      </div>
      <DateLocale locale={dateLocale}>
        <div {...stylex.props(styles.fields)}>
          <DatePicker
            calendarLabel={m.cal_pick_date()}
            isInvalid={problem !== null}
            label={m.schedule_date()}
            minValue={toCalendarDate(fromAbsolute(now.getTime(), zone))}
            nextLabel={m.cal_next_month()}
            onChange={(value) => {
              setDate(value);
              setProblem(null);
            }}
            previousLabel={m.cal_previous_month()}
            value={date}
            {...(firstDayOfWeek === undefined ? {} : { firstDayOfWeek })}
          />
          <TimeField
            isInvalid={problem !== null}
            label={m.schedule_time()}
            onChange={(value) => {
              setTime(value);
              setProblem(null);
            }}
            value={time}
          />
          <Button
            icon={CalendarClock}
            loading={busy}
            onClick={() => {
              submit(pickedTime(date, time, zone));
            }}
            variant="primary"
          >
            {submitLabel}
          </Button>
        </div>
      </DateLocale>
      {problem === null ? (
        <p {...stylex.props(styles.hint)}>
          {m.schedule_limit({ date: latest })}
        </p>
      ) : (
        <p role="alert" {...stylex.props(styles.error)}>
          {PROBLEM_TEXT[problem](latest)}
        </p>
      )}
    </div>
  );
};

/** «Elegir fecha y hora» (and «Cambiar hora» in the Scheduled view). */
export const ScheduleDialog = ({
  onClose,
  openedAt,
  title,
  ...form
}: ScheduleDialogProps) => (
  <Dialog.Root
    open={openedAt !== null}
    onOpenChange={(open) => {
      if (!open) {
        onClose();
      }
    }}
  >
    <Dialog.Content closeLabel={m.close()} size="sm">
      <Dialog.Title>{title}</Dialog.Title>
      <Dialog.Body>
        {/* Mounted only while open: the fields start from the time given. */}
        {openedAt === null ? null : <ScheduleForm {...form} now={openedAt} />}
      </Dialog.Body>
    </Dialog.Content>
  </Dialog.Root>
);
