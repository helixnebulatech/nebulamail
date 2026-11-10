// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Dates and times (React Aria, only for this, because Base UI
// has no calendar or date picker yet). React Aria brings the keyboard and
// screen-reader behaviour, the locale's week start and date order; the look
// is Helix. Values are @internationalized/date objects (CalendarDate, Time).

import type { CalendarDate, Time } from "@internationalized/date";
import {
  CalendarDate as CalendarDay,
  getLocalTimeZone,
  today as todayIn,
} from "@internationalized/date";
import * as stylex from "@stylexjs/stylex";
import {
  CalendarDays,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
} from "lucide-react";
import type { ReactNode } from "react";
import { useContext, useEffect, useState } from "react";
import {
  Button,
  Calendar,
  CalendarCell,
  CalendarStateContext,
  CalendarGrid,
  CalendarGridBody,
  CalendarGridHeader,
  CalendarHeaderCell,
  DateInput,
  DatePicker as AriaDatePicker,
  DateSegment,
  Dialog,
  Group,
  Heading,
  I18nProvider,
  Label,
  Popover,
  TimeField as AriaTimeField,
  useLocale,
} from "react-aria-components";

import { cls, shared } from "./internal";
import {
  colors,
  density,
  font,
  motion,
  radius,
  shadow,
  touch,
  zIndex,
} from "./tokens.stylex";

/** First day of the week, if not the locale's (React Aria names). */
export type WeekStart = "sun" | "mon" | "tue" | "wed" | "thu" | "fri" | "sat";

const styles = stylex.create({
  box: {
    alignItems: "center",
    backgroundColor: colors.bg,
    borderColor: {
      ":focus-within": colors.acc,
      ":hover": colors.fg3,
      default: colors.line2,
    },
    borderRadius: radius.control,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: { ":focus-within": shadow.focusHalo, default: "none" },
    boxSizing: "border-box",
    color: colors.fg,
    columnGap: "4px",
    display: "flex",
    fontFamily: font.sans,
    fontSize: font.sizeMd,
    height: { default: density.input, [touch.coarse]: touch.target },
    paddingInlineEnd: "4px",
    paddingInlineStart: "10px",
    transitionDuration: motion.fast,
    transitionProperty: "border-color, box-shadow",
    transitionTimingFunction: motion.easing,
  },
  boxTime: { paddingInlineEnd: "10px" },
  // Round, like today's marker in the calendar's week and day headers.
  cell: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.pill,
    boxSizing: "border-box",
    color: colors.fg,
    cursor: "pointer",
    display: "flex",
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    height: { default: "32px", [touch.coarse]: "40px" },
    justifyContent: "center",
    outline: "none",
    width: { default: "32px", [touch.coarse]: "40px" },
  },
  cellCompact: {
    height: { default: "28px", [touch.coarse]: "40px" },
    width: { default: "28px", [touch.coarse]: "40px" },
  },
  cellFocus: {
    boxShadow: `0 0 0 2px ${colors.focus}`,
  },
  cellOutside: { color: colors.fg3, opacity: 0.55 },
  // The chosen day: an accent circle drawn on top of the cell, so the
  // week's band (the cell's own background) runs on behind it unbroken.
  cellSelected: {
    "::before": {
      backgroundColor: colors.acc,
      borderRadius: radius.pill,
      content: '""',
      inset: 0,
      position: "absolute",
      zIndex: -1,
    },
    color: colors.accFg,
    fontWeight: font.weightSemibold,
    isolation: "isolate",
    position: "relative",
  },
  // The days the calendar shows (a week): one soft band behind them, its
  // ends rounded, the chosen day's dot on top.
  cellInRange: {
    backgroundColor: { ":hover": colors.sel, default: colors.accSoft },
    borderRadius: 0,
  },
  cellRangeStart: {
    borderEndStartRadius: radius.pill,
    borderStartStartRadius: radius.pill,
  },
  cellRangeEnd: {
    borderEndEndRadius: radius.pill,
    borderStartEndRadius: radius.pill,
  },
  cellToday: { color: colors.accText, fontWeight: font.weightBold },
  cellTodaySelected: { color: colors.accFg },
  cellUnavailable: { cursor: "default", opacity: 0.35 },
  calendar: {
    color: colors.fg,
    fontFamily: font.sans,
    width: "fit-content",
  },
  grid: { borderCollapse: "collapse", borderSpacing: 0 },
  // Always room for six weeks (header + 6 rows), so what's under the
  // calendar doesn't move when a month needs five or six rows.
  gridBox: {
    minHeight: { default: "216px", [touch.coarse]: "264px" },
  },
  gridBoxCompact: {
    minHeight: { default: "192px", [touch.coarse]: "264px" },
  },
  // The month and year as a button that opens the month picker.
  headingButton: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    color: colors.fg,
    columnGap: "4px",
    cursor: "pointer",
    display: "inline-flex",
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    height: { default: "28px", [touch.coarse]: touch.target },
    // The text stays on the sidebar's icon column (x = 22).
    marginInlineStart: "4px",
    paddingInline: "6px",
    textTransform: "capitalize",
  },
  headingSpacer: { flexGrow: 1 },
  fieldset: {
    borderWidth: 0,
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    margin: 0,
    minWidth: 0,
    padding: 0,
  },
  // The month picker takes exactly the calendar's place: its header plus
  // six weeks, and the grid's width (7 day cells).
  pickerBox: {
    display: "flex",
    flexDirection: "column",
    minHeight: { default: "250px", [touch.coarse]: "314px" },
    width: { default: "224px", [touch.coarse]: "280px" },
  },
  pickerBoxCompact: {
    display: "flex",
    flexDirection: "column",
    minHeight: { default: "226px", [touch.coarse]: "314px" },
    width: { default: "196px", [touch.coarse]: "280px" },
  },
  headingChevron: { color: colors.fg3 },
  // Three by four, the rows spread over the calendar's height; each month
  // a pill like the day cells (round hover, accent when shown).
  months: {
    alignItems: "center",
    display: "grid",
    flexGrow: 1,
    gridTemplateColumns: "repeat(3, 1fr)",
    gridTemplateRows: "repeat(4, 1fr)",
    // The six weeks' height (see gridBox), so nothing moves.
    height: { default: "216px", [touch.coarse]: "264px" },
    justifyItems: "center",
  },
  monthsCompact: { height: { default: "192px", [touch.coarse]: "264px" } },
  month: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.pill,
    color: colors.fg,
    cursor: "pointer",
    display: "flex",
    fontSize: font.sizeSm,
    fontVariantNumeric: "tabular-nums",
    height: { default: "28px", [touch.coarse]: touch.target },
    justifyContent: "center",
    minWidth: "52px",
    paddingInline: "10px",
    textTransform: "capitalize",
  },
  // This month: like today in the days (accent text, bold).
  monthCurrent: { color: colors.accText, fontWeight: font.weightBold },
  // The month on screen: like the chosen day (accent fill).
  monthShown: {
    backgroundColor: { ":hover": colors.accHi, default: colors.acc },
    color: colors.accFg,
    fontWeight: font.weightSemibold,
  },
  header: {
    alignItems: "center",
    columnGap: "4px",
    display: "flex",
    justifyContent: "space-between",
    marginBottom: "6px",
  },
  headerCell: {
    color: colors.fg3,
    fontSize: font.sizeXs,
    fontWeight: font.weightSemibold,
    height: "24px",
    textTransform: "uppercase",
  },
  heading: {
    flexGrow: 1,
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    margin: 0,
    // On a sidebar's icon column (x = 22 in the calendar's sidebar), like
    // the section headings below it.
    paddingInlineStart: "10px",
    textTransform: "capitalize",
  },
  input: { display: "flex", flexGrow: 1, minWidth: 0 },
  invalid: { borderColor: colors.danger },
  label: {
    color: colors.fg2,
    display: "block",
    fontSize: font.sizeSm,
    fontWeight: font.weightMedium,
    marginBottom: "4px",
  },
  nav: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.md,
    color: colors.fg2,
    cursor: "pointer",
    display: "inline-flex",
    height: { default: "28px", [touch.coarse]: touch.target },
    justifyContent: "center",
    width: { default: "28px", [touch.coarse]: touch.target },
  },
  navDisabled: { cursor: "default", opacity: 0.4 },
  popover: {
    backgroundColor: colors.raised,
    borderColor: colors.line,
    borderRadius: radius.lg,
    borderStyle: "solid",
    borderWidth: "1px",
    boxShadow: shadow.sh3,
    boxSizing: "border-box",
    outline: "none",
    padding: "12px",
    zIndex: zIndex.popover,
  },
  root: { minWidth: 0 },
  segment: {
    borderRadius: radius.xs,
    color: colors.fg,
    fontVariantNumeric: "tabular-nums",
    outline: "none",
    paddingInline: "1px",
  },
  segmentFocus: { backgroundColor: colors.acc, color: colors.accFg },
  segmentLiteral: { color: colors.fg3 },
  segmentPlaceholder: { color: colors.fg3 },
  trigger: {
    alignItems: "center",
    backgroundColor: { ":hover": colors.hover, default: "transparent" },
    borderRadius: radius.sm,
    color: colors.fg3,
    cursor: "pointer",
    display: "inline-flex",
    flexShrink: 0,
    height: { default: "28px", [touch.coarse]: "36px" },
    justifyContent: "center",
    width: { default: "28px", [touch.coarse]: "36px" },
  },
});

const NavButton = ({
  slot,
  label,
}: {
  readonly slot: "previous" | "next";
  readonly label: string;
}) => (
  <Button
    aria-label={label}
    className={({ isDisabled, isFocusVisible }) =>
      cls(
        shared.resetButton,
        styles.nav,
        isFocusVisible ? styles.cellFocus : null,
        isDisabled ? styles.navDisabled : null
      )
    }
    slot={slot}
  >
    {slot === "previous" ? (
      <ChevronLeft aria-hidden size={16} />
    ) : (
      <ChevronRight aria-hidden size={16} />
    )}
  </Button>
);

/** A run of days to shade, `end` excluded (the shown week). */
export interface DayRange {
  readonly start: CalendarDate;
  readonly end: CalendarDate;
}

const rangeStyles = (
  date: CalendarDate,
  range: DayRange | undefined
): readonly stylex.StyleXStyles[] => {
  if (
    range === undefined ||
    date.compare(range.start) < 0 ||
    date.compare(range.end) >= 0
  ) {
    return [];
  }
  return [
    styles.cellInRange,
    date.compare(range.start) === 0 ? styles.cellRangeStart : null,
    date.add({ days: 1 }).compare(range.end) === 0 ? styles.cellRangeEnd : null,
  ];
};

interface MonthGridProps {
  readonly compact: boolean;
  readonly range?: DayRange;
  readonly previousLabel: string;
  readonly nextLabel: string;
  /** Marks days (e.g. with events) with an accessible description. */
  readonly renderDay?: (date: CalendarDate) => ReactNode;
}

/** Labels of the month picker (open from the month's title). */
export interface MonthPickerLabels {
  /** The title button, e.g. "Choose month and year". */
  readonly choose: string;
  readonly previousYear: string;
  readonly nextYear: string;
}

/**
 * Twelve months of a year, ‹ year ›: jump to any month at once (older
 * dates without paging month by month). Takes the calendar's place.
 */
const MonthPicker = ({
  compact,
  labels,
  onDone,
}: {
  readonly compact: boolean;
  readonly labels: MonthPickerLabels;
  readonly onDone: () => void;
}) => {
  const state = useContext(CalendarStateContext);
  const { locale } = useLocale();
  const shown = state?.focusedDate;
  // Esc goes back to the days (and doesn't close a dialog around it).
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onDone();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
    };
  }, [onDone]);
  const zone = getLocalTimeZone();
  const now = todayIn(zone);
  const [year, setYear] = useState(shown?.year ?? now.year);
  if (state === null || shown === undefined) {
    return null;
  }
  const short = new Intl.DateTimeFormat(locale, { month: "short" });
  return (
    <fieldset {...stylex.props(styles.fieldset)}>
      <legend {...stylex.props(shared.visuallyHidden)}>{labels.choose}</legend>
      <header {...stylex.props(styles.header)}>
        <span {...stylex.props(styles.heading)}>{year}</span>
        <Button
          slot={null}
          aria-label={labels.previousYear}
          className={({ isFocusVisible }) =>
            cls(
              shared.resetButton,
              styles.nav,
              isFocusVisible ? styles.cellFocus : null
            )
          }
          onPress={() => {
            setYear((current) => current - 1);
          }}
        >
          <ChevronLeft aria-hidden size={16} />
        </Button>
        <Button
          slot={null}
          aria-label={labels.nextYear}
          className={({ isFocusVisible }) =>
            cls(
              shared.resetButton,
              styles.nav,
              isFocusVisible ? styles.cellFocus : null
            )
          }
          onPress={() => {
            setYear((current) => current + 1);
          }}
        >
          <ChevronRight aria-hidden size={16} />
        </Button>
      </header>
      <div
        {...stylex.props(styles.months, compact ? styles.monthsCompact : null)}
      >
        {Array.from({ length: 12 }, (_, index) => {
          const month = index + 1;
          const isShown = year === shown.year && month === shown.month;
          const isCurrent = year === now.year && month === now.month;
          return (
            <Button
              slot={null}
              {...(isShown ? { "aria-current": "date" as const } : {})}
              className={({ isFocusVisible }) =>
                cls(
                  shared.resetButton,
                  styles.month,
                  isCurrent ? styles.monthCurrent : null,
                  isShown ? styles.monthShown : null,
                  isFocusVisible ? styles.cellFocus : null
                )
              }
              key={month}
              onPress={() => {
                state.setFocusedDate(shown.set({ day: 1, month, year }));
                onDone();
              }}
            >
              {short.format(new CalendarDay(year, month, 1).toDate(zone))}
            </Button>
          );
        })}
      </div>
    </fieldset>
  );
};

/** The month's title as a button that opens the month picker. */
const MonthTitle = ({
  label,
  onPress,
}: {
  readonly label: string;
  readonly onPress: () => void;
}) => {
  const state = useContext(CalendarStateContext);
  const { locale } = useLocale();
  if (state === null) {
    return null;
  }
  const { start } = state.visibleRange;
  const title = new Intl.DateTimeFormat(locale, {
    month: "long",
    year: "numeric",
  }).format(start.toDate(getLocalTimeZone()));
  return (
    <Button
      slot={null}
      aria-label={`${title} · ${label}`}
      className={({ isFocusVisible }) =>
        cls(
          shared.resetButton,
          styles.headingButton,
          isFocusVisible ? styles.cellFocus : null
        )
      }
      onPress={onPress}
    >
      {title}
      <ChevronDown
        aria-hidden
        size={14}
        {...stylex.props(styles.headingChevron)}
      />
    </Button>
  );
};

const MonthGrid = ({
  compact,
  monthPicker,
  nextLabel,
  previousLabel,
  range,
  renderDay,
}: MonthGridProps & { readonly monthPicker?: MonthPickerLabels }) => {
  const [picking, setPicking] = useState(false);
  if (monthPicker !== undefined && picking) {
    return (
      <div
        {...stylex.props(compact ? styles.pickerBoxCompact : styles.pickerBox)}
      >
        <MonthPicker
          compact={compact}
          labels={monthPicker}
          onDone={() => {
            setPicking(false);
          }}
        />
      </div>
    );
  }
  return (
    <>
      <header {...stylex.props(styles.header)}>
        {monthPicker === undefined ? (
          <Heading className={cls(styles.heading)} />
        ) : (
          <>
            {/* The live title screen readers hear when the month changes. */}
            <Heading className={cls(shared.visuallyHidden)} />
            <MonthTitle
              label={monthPicker.choose}
              onPress={() => {
                setPicking(true);
              }}
            />
            <span {...stylex.props(styles.headingSpacer)} />
          </>
        )}
        <NavButton label={previousLabel} slot="previous" />
        <NavButton label={nextLabel} slot="next" />
      </header>
      <div {...stylex.props(compact ? styles.gridBoxCompact : styles.gridBox)}>
        <CalendarGrid className={cls(styles.grid)} weekdayStyle="narrow">
          <CalendarGridHeader>
            {(day) => (
              <CalendarHeaderCell className={cls(styles.headerCell)}>
                {day}
              </CalendarHeaderCell>
            )}
          </CalendarGridHeader>
          <CalendarGridBody>
            {(date) => (
              <CalendarCell
                className={({
                  isFocusVisible,
                  isOutsideMonth,
                  isSelected,
                  isToday,
                  isUnavailable,
                  isDisabled,
                }) =>
                  cls(
                    styles.cell,
                    compact ? styles.cellCompact : null,
                    isOutsideMonth ? styles.cellOutside : null,
                    ...rangeStyles(date, range),
                    isToday ? styles.cellToday : null,
                    isSelected ? styles.cellSelected : null,
                    isSelected && isToday ? styles.cellTodaySelected : null,
                    isUnavailable || isDisabled ? styles.cellUnavailable : null,
                    isFocusVisible ? styles.cellFocus : null
                  )
                }
                date={date}
              >
                {({ formattedDate }) => (
                  <>
                    {formattedDate}
                    {renderDay?.(date)}
                  </>
                )}
              </CalendarCell>
            )}
          </CalendarGridBody>
        </CalendarGrid>
      </div>
    </>
  );
};

export interface MiniCalendarProps {
  readonly value: CalendarDate;
  readonly onChange: (date: CalendarDate) => void;
  /** Accessible name, e.g. "Choose a date". */
  readonly label: string;
  readonly previousLabel: string;
  readonly nextLabel: string;
  readonly firstDayOfWeek?: WeekStart;
  readonly renderDay?: (date: CalendarDate) => ReactNode;
  /** Days to shade behind the chosen one (the week on screen). */
  readonly range?: DayRange;
  /** Labels to open a month picker from the title (jump to old dates). */
  readonly monthPicker?: MonthPickerLabels;
}

/** A month you can move through with the keyboard (arrows, PageUp/Down). */
export const MiniCalendar = ({
  firstDayOfWeek,
  label,
  nextLabel,
  onChange,
  monthPicker,
  previousLabel,
  range,
  renderDay,
  value,
}: MiniCalendarProps) => (
  <Calendar
    aria-label={label}
    className={cls(styles.calendar)}
    onChange={onChange}
    value={value}
    {...(firstDayOfWeek === undefined ? {} : { firstDayOfWeek })}
  >
    <MonthGrid
      compact
      {...(monthPicker === undefined ? {} : { monthPicker })}
      nextLabel={nextLabel}
      previousLabel={previousLabel}
      {...(range === undefined ? {} : { range })}
      {...(renderDay === undefined ? {} : { renderDay })}
    />
  </Calendar>
);

export interface DatePickerProps {
  readonly label: string;
  /** Hide the label visually (it stays for screen readers). */
  readonly hideLabel?: boolean;
  readonly value: CalendarDate | null;
  readonly onChange: (date: CalendarDate | null) => void;
  readonly minValue?: CalendarDate;
  readonly isDisabled?: boolean;
  readonly isInvalid?: boolean;
  /** Label of the button that opens the calendar. */
  readonly calendarLabel: string;
  readonly previousLabel: string;
  readonly nextLabel: string;
  readonly firstDayOfWeek?: WeekStart;
}

/** A date field (typed segment by segment) with a calendar popover. */
export const DatePicker = ({
  calendarLabel,
  firstDayOfWeek,
  hideLabel = false,
  isDisabled = false,
  isInvalid = false,
  label,
  minValue,
  nextLabel,
  onChange,
  previousLabel,
  value,
}: DatePickerProps) => (
  <AriaDatePicker
    className={cls(styles.root)}
    granularity="day"
    isDisabled={isDisabled}
    isInvalid={isInvalid}
    onChange={onChange}
    value={value}
    {...(minValue === undefined ? {} : { minValue })}
    {...(firstDayOfWeek === undefined ? {} : { firstDayOfWeek })}
  >
    <Label className={cls(hideLabel ? shared.visuallyHidden : styles.label)}>
      {label}
    </Label>
    <Group
      className={({ isInvalid: invalid }) =>
        cls(styles.box, invalid ? styles.invalid : null)
      }
    >
      <DateInput className={cls(styles.input)}>
        {(segment) => (
          <DateSegment
            className={({ isFocused, isPlaceholder, type }) =>
              cls(
                styles.segment,
                type === "literal" ? styles.segmentLiteral : null,
                isPlaceholder ? styles.segmentPlaceholder : null,
                isFocused ? styles.segmentFocus : null
              )
            }
            segment={segment}
          />
        )}
      </DateInput>
      <Button
        aria-label={calendarLabel}
        className={({ isFocusVisible }) =>
          cls(
            shared.resetButton,
            styles.trigger,
            isFocusVisible ? styles.cellFocus : null
          )
        }
      >
        <CalendarDays aria-hidden size={16} />
      </Button>
    </Group>
    <Popover className={cls(styles.popover)} placement="bottom start">
      <Dialog aria-label={calendarLabel} className={cls(shared.focusRing)}>
        <Calendar className={cls(styles.calendar)}>
          <MonthGrid
            compact={false}
            nextLabel={nextLabel}
            previousLabel={previousLabel}
          />
        </Calendar>
      </Dialog>
    </Popover>
  </AriaDatePicker>
);

export interface TimeFieldProps {
  readonly label: string;
  readonly hideLabel?: boolean;
  readonly value: Time | null;
  readonly onChange: (time: Time | null) => void;
  readonly isDisabled?: boolean;
  readonly isInvalid?: boolean;
}

/** A time typed segment by segment (hours, minutes, AM/PM if the locale has it). */
export const TimeField = ({
  hideLabel = false,
  isDisabled = false,
  isInvalid = false,
  label,
  onChange,
  value,
}: TimeFieldProps) => (
  <AriaTimeField
    className={cls(styles.root)}
    granularity="minute"
    isDisabled={isDisabled}
    isInvalid={isInvalid}
    onChange={onChange}
    value={value}
  >
    <Label className={cls(hideLabel ? shared.visuallyHidden : styles.label)}>
      {label}
    </Label>
    <DateInput
      className={({ isInvalid: invalid }) =>
        cls(styles.box, styles.boxTime, invalid ? styles.invalid : null)
      }
    >
      {(segment) => (
        <DateSegment
          className={({ isFocused, isPlaceholder, type }) =>
            cls(
              styles.segment,
              type === "literal" ? styles.segmentLiteral : null,
              isPlaceholder ? styles.segmentPlaceholder : null,
              isFocused ? styles.segmentFocus : null
            )
          }
          segment={segment}
        />
      )}
    </DateInput>
  </AriaTimeField>
);

export interface DateLocaleProps {
  /** BCP 47 tag, e.g. "es" or "en-GB". */
  readonly locale: string;
  readonly children: ReactNode;
}

/** The locale date components format and order dates with. */
export const DateLocale = ({ children, locale }: DateLocaleProps) => (
  <I18nProvider locale={locale}>{children}</I18nProvider>
);
