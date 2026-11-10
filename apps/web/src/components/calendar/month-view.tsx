// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Month view: six weeks; each day lists its first events (all-day and
// multi-day ones as bars across the days), then "+N more", which opens
// that day. The day number opens the day view.

import type { CalendarDate } from "@internationalized/date";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import type { ReactNode } from "react";

import {
  isBanner,
  occurrencesByWeek,
  occursOn,
  placeBanners,
} from "../../calendar/layout";
import type { Banner } from "../../calendar/layout";
import type { Occurrence } from "../../calendar/model";
import { formatDate, formatTime } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { bp } from "../mail/breakpoints.stylex";
import { shared } from "../mail/styles";
import {
  chipStyles,
  colorVars,
  eventLabel,
  looksDeclined,
  looksPending,
} from "./event-chip";
import { titleText } from "./event-text";

/** Event lanes per week row (the rest goes into "+N more"). */
const LANES = 3;

const styles = stylex.create({
  cell: {
    borderInlineStartColor: colors.line,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: "1px",
    borderTopColor: colors.line,
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    gridRow: "1 / -1",
    minWidth: 0,
  },
  cellOutside: { backgroundColor: colors.subtle },
  chip: {
    alignItems: "center",
    borderRadius: radius.sm,
    boxSizing: "border-box",
    columnGap: "5px",
    display: "flex",
    fontSize: { default: font.sizeSm, [bp.mobile]: font.sizeXs },
    height: "20px",
    marginInline: "2px",
    minWidth: 0,
    paddingInline: "5px",
    zIndex: 1,
  },
  chipHover: {
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
  },
  chipCut: { borderEndStartRadius: 0, borderStartStartRadius: 0 },
  chipCutEnd: { borderEndEndRadius: 0, borderStartEndRadius: 0 },
  day: {
    alignItems: "center",
    alignSelf: "start",
    borderRadius: radius.pill,
    display: "inline-flex",
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    fontWeight: font.weightSemibold,
    gridRow: "1",
    height: { default: "24px", [touch.coarse]: "32px" },
    justifyContent: "center",
    justifySelf: "start",
    marginInlineStart: "4px",
    marginTop: "3px",
    minWidth: { default: "24px", [touch.coarse]: "32px" },
    paddingInline: "4px",
    zIndex: 1,
  },
  dayOutside: { color: colors.fg3 },
  extras: {
    display: "flex",
    minHeight: 0,
    minWidth: 0,
    paddingInline: "2px",
    zIndex: 1,
  },
  dayToday: { backgroundColor: colors.acc, color: colors.accFg },
  grid: {
    display: "grid",
    flexGrow: 1,
    gridTemplateRows: "auto repeat(6, minmax(0, 1fr))",
    minHeight: 0,
  },
  head: {
    color: colors.fg3,
    display: "grid",
    fontSize: font.sizeSm,
    gridTemplateColumns: "repeat(7, minmax(0, 1fr))",
    paddingBlock: "6px",
    textAlign: "center",
    textTransform: "capitalize",
  },
  more: {
    color: colors.fg2,
    fontSize: font.sizeXs,
    fontWeight: font.weightSemibold,
    height: "18px",
    marginInlineStart: "6px",
    textAlign: "start",
    zIndex: 1,
  },
  week: {
    display: "grid",
    gridTemplateColumns: "repeat(7, minmax(0, 1fr))",
    // Day number, the event lanes, "+N more", then the rest of the cell.
    gridTemplateRows: {
      default: "32px repeat(4, 22px) minmax(0, 1fr)",
      [touch.coarse]: "38px repeat(4, 22px) minmax(0, 1fr)",
    },
    minHeight: 0,
    overflow: "hidden",
    rowGap: "1px",
  },
});

const place = stylex.create({
  at: (column: string, row: number) => ({
    gridColumn: column,
    gridRow: String(row),
  }),
  cell: (column: number) => ({ gridColumn: String(column) }),
});

export interface MonthViewProps {
  readonly weeks: readonly (readonly CalendarDate[])[];
  readonly month: number;
  readonly occurrences: readonly Occurrence[];
  readonly zone: string;
  readonly locale: string;
  readonly today: CalendarDate;
  readonly onOpen: (occurrence: Occurrence) => void;
  readonly onDay: (day: CalendarDate) => void;
  /** More things a day lists in its free lanes (tasks due that day). */
  readonly dayExtras?: (day: CalendarDate) => readonly ReactNode[];
}

const Chip = ({
  banner,
  locale,
  onOpen,
  zone,
}: {
  readonly banner: Banner;
  readonly zone: string;
  readonly locale: string;
  readonly onOpen: (occurrence: Occurrence) => void;
}) => {
  const { occurrence } = banner;
  const bar = isBanner(occurrence);
  return (
    <button
      aria-label={eventLabel(occurrence, zone, locale)}
      onClick={() => {
        onOpen(occurrence);
      }}
      type="button"
      {...stylex.props(
        shared.resetButton,
        shared.focusRing,
        styles.chip,
        bar ? chipStyles.filled : styles.chipHover,
        bar && looksPending(occurrence)
          ? [chipStyles.tint, chipStyles.pending]
          : null,
        looksDeclined(occurrence) ? chipStyles.declined : null,
        banner.continuesBefore ? styles.chipCut : null,
        banner.continuesAfter ? styles.chipCutEnd : null,
        colorVars.color(occurrence.color),
        place.at(
          `${String(banner.from + 1)} / ${String(banner.to + 2)}`,
          banner.row + 2
        )
      )}
    >
      {bar ? null : (
        <span
          {...stylex.props(
            chipStyles.dot,
            looksPending(occurrence) ? chipStyles.dashed : null
          )}
        />
      )}
      {bar ? null : (
        <span {...stylex.props(chipStyles.time)}>
          {formatTime(occurrence.start, zone, locale)}
        </span>
      )}
      <span {...stylex.props(chipStyles.title)}>
        {titleText(occurrence.title)}
      </span>
    </button>
  );
};

export const MonthView = ({
  dayExtras,
  locale,
  month,
  occurrences,
  onDay,
  onOpen,
  today,
  weeks,
  zone,
}: MonthViewProps) => {
  // Each row only looks at the events that touch its week.
  const perWeek = occurrencesByWeek(occurrences, weeks, zone);
  return (
    <div {...stylex.props(styles.grid)}>
      <div aria-hidden {...stylex.props(styles.head)}>
        {(weeks[0] ?? []).map((day) => (
          <span key={day.toString()}>
            {formatDate(day, locale, { weekday: "short" })}
          </span>
        ))}
      </div>
      {weeks.map((week, weekIndex) => {
        const banners = placeBanners(
          perWeek[weekIndex] ?? [],
          week,
          zone,
          () => true
        );
        const shown = banners.filter((banner) => banner.row < LANES);
        // Extras (tasks) take the lanes events leave free that day; the
        // rest count in "+N more".
        const extras = week.map((day, index) => {
          const used = new Set(
            shown
              .filter((banner) => banner.from <= index && banner.to >= index)
              .map((banner) => banner.row)
          );
          const free = Array.from({ length: LANES }, (_, lane) => lane).filter(
            (lane) => !used.has(lane)
          );
          const nodes = dayExtras?.(day) ?? [];
          return {
            hidden: Math.max(0, nodes.length - free.length),
            placed: nodes
              .slice(0, free.length)
              .map((node, at) => ({ lane: free[at] ?? 0, node })),
          };
        });
        const hidden = week.map(
          (day, index) =>
            banners.filter(
              (banner) =>
                banner.row >= LANES && occursOn(banner.occurrence, day, zone)
            ).length + (extras[index]?.hidden ?? 0)
        );
        return (
          <div key={week[0]?.toString()} {...stylex.props(styles.week)}>
            {week.map((day, index) => (
              <span
                aria-hidden
                // Where a dragged task lands (components/tasks).
                data-drop-date={day.toString()}
                data-drop-kind="day"
                key={`cell-${day.toString()}`}
                {...stylex.props(
                  styles.cell,
                  day.month === month ? null : styles.cellOutside,
                  place.cell(index + 1)
                )}
              />
            ))}
            {week.map((day, index) => {
              const isToday = day.compare(today) === 0;
              return (
                <button
                  aria-current={isToday ? "date" : undefined}
                  aria-label={m.cal_open_day({
                    day: formatDate(day, locale, { dateStyle: "full" }),
                  })}
                  key={day.toString()}
                  onClick={() => {
                    onDay(day);
                  }}
                  type="button"
                  {...stylex.props(
                    shared.resetButton,
                    shared.focusRing,
                    styles.day,
                    day.month === month ? null : styles.dayOutside,
                    isToday ? styles.dayToday : null,
                    place.cell(index + 1)
                  )}
                >
                  {day.day}
                </button>
              );
            })}
            {shown.map((banner) => (
              <Chip
                banner={banner}
                key={`${banner.occurrence.id}-${String(banner.from)}`}
                locale={locale}
                onOpen={onOpen}
                zone={zone}
              />
            ))}
            {week.map((day, index) =>
              (hidden[index] ?? 0) > 0 ? (
                <button
                  key={`more-${day.toString()}`}
                  onClick={() => {
                    onDay(day);
                  }}
                  type="button"
                  {...stylex.props(
                    shared.resetButton,
                    shared.focusRing,
                    styles.more,
                    place.at(String(index + 1), LANES + 2)
                  )}
                >
                  {m.cal_more({ count: hidden[index] ?? 0 })}
                </button>
              ) : null
            )}
            {extras.flatMap((extra, index) =>
              extra.placed.map(({ lane, node }) => (
                <div
                  // oxlint-disable-next-line react/no-array-index-key -- a lane holds one extra; lanes never reorder.
                  key={`extra-${String(index)}-${String(lane)}`}
                  {...stylex.props(
                    styles.extras,
                    place.at(String(index + 1), lane + 2)
                  )}
                >
                  {node}
                </div>
              ))
            )}
          </div>
        );
      })}
    </div>
  );
};
