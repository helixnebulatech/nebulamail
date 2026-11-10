// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// More than Yes / Maybe / No: a note for the organizer
// that goes with the answer (COMMENT in the REPLY, and in the email's
// text), or another time to propose (COUNTER, RFC 5546). Hidden behind one
// button so the usual answer stays one click. Times are picked in the
// user's zone and sent in UTC.

import type { CalendarDate, Time } from "@internationalized/date";
import {
  Time as TimeValue,
  toCalendarDateTime,
  toZoned,
} from "@internationalized/date";
import { Button, DatePicker, Field, Textarea, TimeField } from "@nebulamail/ui";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { CalendarClock, ChevronDown, ChevronUp } from "lucide-react";
import { useState } from "react";

import { useTimeZone } from "../../calendar/hooks";
import type { Proposal } from "../../calendar/invitations";
import { MAX_COMMENT } from "../../calendar/itip";
import { dateInZone, dateTimeFormat } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { useWeekStart } from "./use-calendar-prefs";

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
  error: { color: colors.danger, fontSize: font.sizeSm, margin: 0 },
  hint: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  legend: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    marginBottom: "6px",
    padding: 0,
  },
  propose: { borderStyle: "none", margin: 0, minWidth: 0, padding: 0 },
  row: {
    alignItems: "flex-end",
    columnGap: "8px",
    display: "flex",
    flexWrap: "wrap",
    rowGap: "8px",
  },
});

export interface ReplyDraft {
  readonly open: boolean;
  readonly comment: string;
  readonly setOpen: (open: boolean) => void;
  readonly setComment: (comment: string) => void;
  /** What goes with an answer (empty note: nothing). */
  readonly extras: { readonly comment?: string };
}

/** The note typed so far (shared by the answer buttons and the proposal). */
export const useReplyDraft = (): ReplyDraft => {
  const [open, setOpen] = useState(false);
  const [comment, setComment] = useState("");
  const note = open ? comment.trim() : "";
  return {
    comment,
    extras: note === "" ? {} : { comment: note },
    open,
    setComment,
    setOpen,
  };
};

const timeOf = (ms: number, zone: string): Time => {
  const parts = dateTimeFormat("en-GB", {
    hour: "2-digit",
    hourCycle: "h23",
    minute: "2-digit",
    timeZone: zone,
  }).formatToParts(ms);
  const value = (type: string): number =>
    Number(parts.find((part) => part.type === type)?.value ?? "0");
  return new TimeValue(value("hour"), value("minute"));
};

const msAt = (date: CalendarDate, time: Time, zone: string): number =>
  toZoned(toCalendarDateTime(date, time), zone).toDate().getTime();

/** The proposal's times, or why they can't be sent. */
export const proposalOf = (
  date: CalendarDate | null,
  start: Time | null,
  end: Time | null,
  zone: string
): Proposal | "invalid" => {
  if (date === null || start === null || end === null) {
    return "invalid";
  }
  const from = msAt(date, start, zone);
  const to = msAt(date, end, zone);
  return to > from ? { end: to, start: from } : "invalid";
};

export const ReplyOptions = ({
  draft,
  end,
  onPropose,
  proposing,
  start,
}: {
  readonly draft: ReplyDraft;
  /** The event's current times (the proposal starts from them). */
  readonly start: number;
  readonly end: number;
  readonly onPropose: (proposal: Proposal) => void;
  readonly proposing: boolean;
}) => {
  const zone = useTimeZone();
  const [, firstDayOfWeek] = useWeekStart();
  const [date, setDate] = useState<CalendarDate | null>(() =>
    dateInZone(start, zone)
  );
  const [from, setFrom] = useState<Time | null>(() => timeOf(start, zone));
  const [to, setTo] = useState<Time | null>(() => timeOf(end, zone));
  const [invalid, setInvalid] = useState(false);
  const weekStart = firstDayOfWeek === undefined ? {} : { firstDayOfWeek };
  return (
    <div>
      <Button
        aria-expanded={draft.open}
        iconEnd={draft.open ? ChevronUp : ChevronDown}
        onClick={() => {
          draft.setOpen(!draft.open);
        }}
        size="sm"
        variant="ghost"
      >
        {m.cal_reply_more()}
      </Button>
      {draft.open ? (
        <div {...stylex.props(styles.box)}>
          <Field label={m.cal_reply_note()}>
            <Textarea
              dir="auto"
              maxLength={MAX_COMMENT}
              onChange={(event) => {
                draft.setComment(event.target.value);
              }}
              rows={2}
              value={draft.comment}
            />
          </Field>
          <p {...stylex.props(styles.hint)}>{m.cal_reply_note_hint()}</p>
          <fieldset {...stylex.props(styles.propose)}>
            <legend {...stylex.props(styles.legend)}>
              {m.cal_propose_time()}
            </legend>
            <div {...stylex.props(styles.row)}>
              <DatePicker
                calendarLabel={m.cal_pick_date()}
                label={m.cal_propose_date()}
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
                label={m.cal_start_time()}
                onChange={(value) => {
                  setFrom(value);
                  setInvalid(false);
                }}
                value={from}
              />
              <TimeField
                isInvalid={invalid}
                label={m.cal_end_time()}
                onChange={(value) => {
                  setTo(value);
                  setInvalid(false);
                }}
                value={to}
              />
              <Button
                icon={CalendarClock}
                loading={proposing}
                onClick={() => {
                  const proposal = proposalOf(date, from, to, zone);
                  if (proposal === "invalid") {
                    setInvalid(true);
                  } else {
                    onPropose(proposal);
                  }
                }}
                size="sm"
                variant="secondary"
              >
                {m.cal_propose_send()}
              </Button>
            </div>
            {invalid ? (
              <p role="alert" {...stylex.props(styles.error)}>
                {m.cal_problem_order()}
              </p>
            ) : null}
          </fieldset>
        </div>
      ) : null}
    </div>
  );
};
