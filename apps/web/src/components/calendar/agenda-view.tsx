// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Agenda: the next 30 days that have events, as a list (the best view on a
// phone and for screen readers).

import type { CalendarDate } from "@internationalized/date";
import { EmptyState } from "@nebulamail/ui";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { CalendarDays } from "lucide-react";
import type { ReactNode } from "react";

import { agendaDays, daysOf } from "../../calendar/layout";
import type { DateRange } from "../../calendar/layout";
import type { Occurrence } from "../../calendar/model";
import { formatDate } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { shared } from "../mail/styles";
import {
  chipStyles,
  colorVars,
  eventLabel,
  looksDeclined,
  looksPending,
  timeRangeText,
} from "./event-chip";
import { SafeText, titleText } from "./event-text";

const styles = stylex.create({
  day: {
    display: "grid",
    gap: "4px 16px",
    gridTemplateColumns: "minmax(88px, 140px) minmax(0, 1fr)",
    paddingBlock: "10px",
    paddingInline: "16px",
  },
  dayLine: {
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
  },
  dayName: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontWeight: font.weightSemibold,
    paddingTop: "8px",
    textTransform: "capitalize",
  },
  dayToday: { color: colors.accText },
  events: { display: "flex", flexDirection: "column", rowGap: "2px" },
  list: { flexGrow: 1, minHeight: 0, overflowY: "auto" },
  location: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  row: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    columnGap: "10px",
    display: "grid",
    gridTemplateColumns: "auto minmax(120px, 160px) minmax(0, 1fr)",
    minHeight: { default: "36px", [touch.coarse]: touch.target },
    paddingInline: "8px",
  },
  text: { display: "flex", flexDirection: "column", minWidth: 0 },
  time: {
    color: colors.fg2,
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
  },
});

export interface AgendaViewProps {
  readonly range: DateRange;
  readonly occurrences: readonly Occurrence[];
  readonly zone: string;
  readonly locale: string;
  readonly today: CalendarDate;
  readonly onOpen: (occurrence: Occurrence) => void;
  /** Days with other things to list (tasks), "YYYY-MM-DD". */
  readonly extraDays?: readonly string[];
  /** Those things for a day, listed before its events. */
  readonly dayExtras?: (day: CalendarDate) => ReactNode;
}

const NO_DAYS: readonly string[] = [];

export const AgendaView = ({
  dayExtras,
  extraDays = NO_DAYS,
  locale,
  occurrences,
  onOpen,
  range,
  today,
  zone,
}: AgendaViewProps) => {
  const withEvents = agendaDays(occurrences, range, zone);
  const listed = new Set(withEvents.map((entry) => entry.day.toString()));
  const days = [
    ...withEvents,
    ...daysOf(range)
      .filter(
        (day) =>
          extraDays.includes(day.toString()) && !listed.has(day.toString())
      )
      .map((day) => ({ day, events: [] as Occurrence[] })),
  ].toSorted((a, b) => a.day.compare(b.day));
  if (days.length === 0) {
    return (
      <EmptyState
        illustration={<CalendarDays aria-hidden size={40} />}
        title={m.cal_agenda_empty()}
      >
        {m.cal_agenda_empty_hint()}
      </EmptyState>
    );
  }
  return (
    <ol {...stylex.props(shared.resetList, styles.list)}>
      {days.map(({ day, events }) => {
        const label = formatDate(day, locale, {
          day: "numeric",
          month: "short",
          weekday: "long",
        });
        return (
          <li
            key={day.toString()}
            {...stylex.props(styles.day, styles.dayLine)}
          >
            <h3
              {...stylex.props(
                shared.resetButton,
                styles.dayName,
                day.compare(today) === 0 ? styles.dayToday : null
              )}
            >
              {label}
            </h3>
            <ul {...stylex.props(shared.resetList, styles.events)}>
              {dayExtras?.(day)}
              {events.map((occurrence) => (
                <li key={occurrence.id}>
                  <button
                    aria-label={eventLabel(occurrence, zone, locale, label)}
                    onClick={() => {
                      onOpen(occurrence);
                    }}
                    type="button"
                    {...stylex.props(
                      shared.resetButton,
                      shared.focusRing,
                      styles.row,
                      looksDeclined(occurrence) ? chipStyles.declined : null,
                      colorVars.color(occurrence.color)
                    )}
                  >
                    <span
                      {...stylex.props(
                        chipStyles.dot,
                        looksPending(occurrence) ? chipStyles.dashed : null
                      )}
                    />
                    <span {...stylex.props(styles.time)}>
                      {timeRangeText(occurrence, zone, locale)}
                    </span>
                    <span {...stylex.props(styles.text)}>
                      <span {...stylex.props(chipStyles.title)}>
                        {titleText(occurrence.title)}
                      </span>
                      {occurrence.location === null ? null : (
                        <span {...stylex.props(styles.location)}>
                          <SafeText text={occurrence.location} />
                        </span>
                      )}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </li>
        );
      })}
    </ol>
  );
};
