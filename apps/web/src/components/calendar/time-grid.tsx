// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Week and day views: a column per day with the hours, all-day events on
// top, a line at the current time. Click an empty hour to create an event
// there, or drag over empty time to create one that long (a finger rests
// a moment first, so a swipe still scrolls; from the keyboard, C and the
// editor's times do the same). With a mouse or pen, drag an event to move
// it and its bottom edge to change its length; from the keyboard, the same
// with Alt + arrows (Alt + Shift + ↑/↓ for the length), so dragging is
// never the only way.

import type { CalendarDate } from "@internationalized/date";
import { colors, font, radius, touch } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { useEffect, useEffectEvent, useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";

import {
  drawnRange,
  localAt,
  placeBanners,
  placeDay,
} from "../../calendar/layout";
import type { Banner, PlacedEvent } from "../../calendar/layout";
import type { Occurrence } from "../../calendar/model";
import {
  DAY_MINUTES,
  dateTimeFormat,
  formatDate,
  formatTime,
  minutesInDay,
  timeZoneLabel,
  timeZoneOffset,
} from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { bp } from "../mail/breakpoints.stylex";
import { shared } from "../mail/styles";
import {
  chipStyles,
  colorVars,
  eventLabel,
  looksDeclined,
  looksPending,
  timeRangeText,
} from "./event-chip";
import { titleText } from "./event-text";

/** Height of one hour in px. */
const HOUR = 48;
const PX_PER_MINUTE = HOUR / 60;
/** Drag and keyboard steps, in minutes. */
export const STEP = 15;
/** How far the pointer moves before a press becomes a drag. */
const DRAG_THRESHOLD = 4;
const GUTTER = "52px";

const CHIP_NARROW = "@container event-chip (max-width: 110px)";

const styles = stylex.create({
  allDay: {
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    display: "grid",
    flexShrink: 0,
    gridAutoRows: "24px",
    paddingBlock: "4px",
    rowGap: "2px",
  },
  allDayLabel: {
    alignSelf: "center",
    color: colors.fg3,
    fontSize: font.sizeXs,
    gridColumn: "1",
    gridRow: "1",
    paddingInlineEnd: "6px",
    textAlign: "end",
  },
  banner: {
    alignItems: "center",
    borderRadius: radius.sm,
    boxSizing: "border-box",
    columnGap: "6px",
    display: "flex",
    fontSize: font.sizeSm,
    marginInline: "2px",
    minWidth: 0,
    paddingInline: "6px",
  },
  bannerCut: { borderEndStartRadius: 0, borderStartStartRadius: 0 },
  bannerCutEnd: { borderEndEndRadius: 0, borderStartEndRadius: 0 },
  body: {
    display: "flex",
    minHeight: `${String(24 * HOUR)}px`,
    position: "relative",
  },
  column: {
    borderInlineStartColor: colors.line,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: "1px",
    cursor: "copy",
    flexBasis: 0,
    flexGrow: 1,
    minWidth: 0,
    position: "relative",
  },
  columnDrawing: { cursor: "ns-resize", userSelect: "none" },
  drawn: {
    backgroundColor: `color-mix(in srgb, ${colors.acc} 18%, ${colors.bg})`,
    borderColor: colors.acc,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    boxSizing: "border-box",
    color: colors.fg,
    fontSize: font.sizeSm,
    overflow: "hidden",
    paddingBlock: "2px",
    paddingInline: "6px",
    pointerEvents: "none",
    position: "absolute",
    zIndex: 3,
  },
  columnToday: {
    backgroundColor: `color-mix(in srgb, ${colors.acc} 3%, transparent)`,
  },
  columns: { display: "flex", flexGrow: 1, minWidth: 0, position: "relative" },
  dayButton: {
    alignItems: "center",
    borderRadius: radius.md,
    columnGap: "6px",
    display: "inline-flex",
    flexDirection: { default: "row", [bp.mobile]: "column" },
    minHeight: { default: "32px", [touch.coarse]: touch.target },
    paddingInline: "6px",
  },
  dayHeader: {
    alignItems: "center",
    borderInlineStartColor: colors.line,
    borderInlineStartStyle: "solid",
    borderInlineStartWidth: "1px",
    display: "flex",
    flexBasis: 0,
    flexGrow: 1,
    justifyContent: "center",
    minWidth: 0,
    paddingBlock: "6px",
  },
  dayName: {
    color: colors.fg3,
    fontSize: font.sizeSm,
    textTransform: "capitalize",
  },
  dayNumber: {
    fontSize: font.sizeXl,
    fontVariantNumeric: "tabular-nums",
    fontWeight: font.weightSemibold,
    lineHeight: 1.1,
  },
  dayNumberToday: {
    alignItems: "center",
    backgroundColor: colors.acc,
    borderRadius: radius.pill,
    color: colors.accFg,
    display: "inline-flex",
    height: "30px",
    justifyContent: "center",
    minWidth: "30px",
  },
  event: {
    boxSizing: "border-box",
    cursor: "pointer",
    display: "flex",
    flexDirection: "column",
    fontSize: font.sizeSm,
    lineHeight: 1.25,
    overflow: "hidden",
    paddingBlock: "2px",
    paddingInline: "6px",
    position: "absolute",
    textAlign: "start",
    touchAction: "manipulation",
    // Narrow chips (an event panel open, overlapping events) show only
    // the start time instead of a range cut mid-string.
    containerName: "event-chip",
    containerType: "inline-size",
  },
  rangeWide: {
    display: { default: "inline", [CHIP_NARROW]: "none" },
  },
  rangeNarrow: {
    display: { default: "none", [CHIP_NARROW]: "inline" },
  },
  eventDragging: {
    boxShadow: "0 8px 20px -6px rgba(21, 21, 43, 0.35)",
    cursor: "grabbing",
    opacity: 0.9,
    zIndex: 3,
  },
  eventRow: { alignItems: "baseline", columnGap: "6px", display: "flex" },
  gutter: {
    flexShrink: 0,
    position: "relative",
    width: GUTTER,
  },
  gutterSpacer: { flexShrink: 0, width: GUTTER },
  // The empty corner over the hour labels names their time zone, like
  // Google Calendar and Fantastical.
  zone: {
    // Centred on the day headers' row, like the weekday names.
    alignSelf: "center",
    boxSizing: "border-box",
    color: colors.fg3,
    fontSize: font.sizeXs,
    fontVariantNumeric: "tabular-nums",
    overflow: "hidden",
    paddingInlineEnd: "8px",
    textAlign: "end",
    whiteSpace: "nowrap",
  },
  head: {
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    display: "flex",
    flexShrink: 0,
  },
  hourLabel: {
    color: colors.fg3,
    fontSize: font.sizeXs,
    fontVariantNumeric: "tabular-nums",
    insetInlineEnd: "8px",
    position: "absolute",
    transform: "translateY(-50%)",
    whiteSpace: "nowrap",
  },
  hourLine: {
    borderTopColor: colors.line,
    borderTopStyle: "solid",
    borderTopWidth: "1px",
    insetInline: 0,
    pointerEvents: "none",
    position: "absolute",
  },
  nowDot: {
    backgroundColor: colors.acc,
    borderRadius: "50%",
    height: "10px",
    insetInlineStart: "-5px",
    position: "absolute",
    top: "-5px",
    width: "10px",
  },
  nowLine: {
    borderTopColor: colors.acc,
    borderTopStyle: "solid",
    borderTopWidth: "2px",
    insetInline: 0,
    pointerEvents: "none",
    position: "absolute",
    zIndex: 2,
  },
  resize: {
    bottom: 0,
    cursor: "ns-resize",
    height: "8px",
    insetInline: 0,
    position: "absolute",
  },
  root: {
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
  },
  scroll: {
    // Days and all-day row scroll with the hours (sticky at the top), so
    // the scrollbar takes width from both and the columns stay aligned.
    display: "flex",
    flexDirection: "column",
    flexGrow: 1,
    minHeight: 0,
    // Days never get narrower than 120px on wider screens (an event
    // panel open next to the week): the grid scrolls sideways instead.
    overflowX: { default: "auto", [bp.mobile]: "hidden" },
    overflowY: "auto",
    overscrollBehavior: "contain",
    // Hour labels are centred on their lines: scrolled to the end, the top
    // one would sit half under the day header without this room.
    paddingBottom: "12px",
  },
});

const minDays = stylex.create({
  wide: {
    minWidth: {
      default: `calc(${GUTTER} + var(--grid-days) * 120px)`,
      [bp.mobile]: 0,
    },
  },
});

const sticky = stylex.create({
  top: {
    backgroundColor: colors.bg,
    flexShrink: 0,
    position: "sticky",
    top: 0,
    // Above events, the "now" line and a dragged event.
    zIndex: 4,
  },
});

const place = stylex.create({
  banner: (column: string, row: number) => ({
    gridColumn: column,
    gridRow: String(row),
  }),
  block: (top: number, height: number, left: string, width: string) => ({
    height: `${String(height)}px`,
    insetInlineStart: left,
    top: `${String(top)}px`,
    width,
  }),
  columns: (count: number) => ({
    gridTemplateColumns: `${GUTTER} repeat(${String(count)}, minmax(0, 1fr))`,
  }),
  days: (count: number) => ({ "--grid-days": String(count) }),
  top: (top: number) => ({ top: `${String(top)}px` }),
});

const extraStyles = stylex.create({
  allDay: {
    display: "flex",
    marginInline: "2px",
    minWidth: 0,
  },
  timed: {
    display: "flex",
    insetInlineEnd: "2px",
    insetInlineStart: "2px",
    position: "absolute",
    // Over the events of that hour, under a dragged one.
    zIndex: 2,
  },
});

export interface MoveRequest {
  readonly occurrence: Occurrence;
  /** Whole days to move by. */
  readonly days: number;
  /** Minutes to move the start by. */
  readonly minutes: number;
  /** Minutes to change the length by. */
  readonly resize: number;
}

export interface TimeGridProps {
  readonly days: readonly CalendarDate[];
  readonly occurrences: readonly Occurrence[];
  readonly zone: string;
  readonly locale: string;
  readonly today: CalendarDate;
  readonly now: number;
  readonly onOpen: (occurrence: Occurrence) => void;
  /** A click on an empty hour, or a dragged range (`localEnd`). */
  readonly onNew: (localStart: string, localEnd?: string) => void;
  readonly onMove: (request: MoveRequest) => void;
  readonly onDay: (day: CalendarDate) => void;
  /** More things in the all-day row (tasks due that day), by day index. */
  readonly allDayExtras?: readonly DayExtra[];
  /** Small marks at a time of day (tasks due then), by day index. */
  readonly timedExtras?: readonly TimedExtra[];
}

/** Something the grid shows for a day without knowing what it is. */
export interface DayExtra {
  readonly key: string;
  /** Index into `days`. */
  readonly day: number;
  readonly node: ReactNode;
}

export interface TimedExtra extends DayExtra {
  /** Minutes into the day. */
  readonly minutes: number;
}

/** Where a range drawn on an empty part of a day started. */
interface DrawOrigin {
  readonly day: number;
  /** Minute of the day under the press. */
  readonly anchor: number;
  /** Top of the day column on screen, when the press started. */
  readonly top: number;
  readonly pointerId: number;
  /** Started by a long press (shown at once). */
  readonly touch: boolean;
}

/** A range being drawn (minutes of the day). */
interface Draw {
  /** The same object for the whole gesture. */
  readonly origin: DrawOrigin;
  readonly current: number;
  /** Moved far enough to be a drag, not a click. */
  readonly moved: boolean;
}

/** How long a finger rests before drawing a range (else it scrolls). */
const LONG_PRESS_MS = 450;
/** A finger that moves more than this during the long press scrolls. */
const TOUCH_SLOP = 10;

// drawnRange and localAt: calendar/layout.ts (pure, with tests).

/** A press on an empty part of a day, before and while drawing. */
interface Press {
  /** The pointer moved: the click that follows isn't a click. */
  drawn: boolean;
  /** Where a finger came down (a long press is pending). */
  start: { readonly x: number; readonly y: number } | null;
  timer: ReturnType<typeof setTimeout> | null;
  /** A finger is drawing: the page must not scroll. */
  drawing: boolean;
}

/**
 * A press on empty time in a day column, not on an event. Touch
 * screens may report a decoration (an hour line) as the target.
 */
const onEmptyTime = (event: {
  readonly target: EventTarget;
  readonly currentTarget: EventTarget;
}): boolean =>
  event.target === event.currentTarget ||
  (event.target instanceof Element &&
    event.target.closest("button, [data-no-draw]") === null &&
    event.target.getAttribute("aria-hidden") === "true");

const minuteAt = (clientY: number, top: number): number =>
  Math.min(DAY_MINUTES, Math.max(0, (clientY - top) / PX_PER_MINUTE));

const cancelPress = (press: Press): void => {
  if (press.timer !== null) {
    clearTimeout(press.timer);
    press.timer = null;
  }
  press.start = null;
};

/**
 * Fingers on the day columns: a pending long press is cancelled when the
 * finger moves first (it scrolls) or lifts; while drawing, the page
 * doesn't scroll. touchmove is only cancelable when a non-passive
 * listener is already there when the finger comes down, so this one stays
 * for the life of the grid. Returns the cleanup.
 */
const watchPress = (
  element: HTMLElement,
  ref: { readonly current: Press }
): (() => void) => {
  const press = ref.current;
  const watch = (event: TouchEvent): void => {
    if (press.drawing) {
      event.preventDefault();
      return;
    }
    const { start } = press;
    const [finger] = event.touches;
    if (
      start !== null &&
      finger !== undefined &&
      (Math.abs(finger.clientX - start.x) > TOUCH_SLOP ||
        Math.abs(finger.clientY - start.y) > TOUCH_SLOP)
    ) {
      cancelPress(press);
    }
  };
  const end = (): void => {
    cancelPress(press);
  };
  element.addEventListener("touchmove", watch, { passive: false });
  element.addEventListener("touchend", end);
  element.addEventListener("touchcancel", end);
  return () => {
    element.removeEventListener("touchmove", watch);
    element.removeEventListener("touchend", end);
    element.removeEventListener("touchcancel", end);
    cancelPress(press);
  };
};

interface DrawOptions {
  /** A new event over minutes [from, to) of a day (`to`: default length). */
  readonly onNew: (day: number, from: number, to?: number) => void;
  readonly press: { readonly current: Press };
  readonly setDraw: (draw: Draw | null) => void;
}

/**
 * Follows the pointer while drawing a range; returns the cleanup. Once per
 * gesture: the progress lives here, and the grid only re-renders when the
 * drawn range (in steps) changes.
 */
const followDraw = (
  origin: DrawOrigin,
  { onNew, press: ref, setDraw }: DrawOptions
): (() => void) => {
  const press = ref.current;
  press.drawing = origin.touch;
  let latest: Draw = { current: origin.anchor, moved: false, origin };
  const stop = (): void => {
    press.drawing = false;
    setDraw(null);
  };
  const move = (event: globalThis.PointerEvent): void => {
    if (event.pointerId !== origin.pointerId) {
      return;
    }
    const current = minuteAt(event.clientY, origin.top);
    const moved =
      latest.moved ||
      Math.abs(current - origin.anchor) * PX_PER_MINUTE > DRAG_THRESHOLD;
    press.drawn = moved;
    const before = drawnRange(origin.anchor, latest.current);
    const after = drawnRange(origin.anchor, current);
    const changed =
      moved !== latest.moved ||
      before.from !== after.from ||
      before.to !== after.to;
    latest = { current, moved, origin };
    if (changed) {
      setDraw(latest);
    }
  };
  const up = (event: globalThis.PointerEvent): void => {
    if (event.pointerId !== origin.pointerId) {
      return;
    }
    const { from, to } = drawnRange(origin.anchor, latest.current);
    if (press.drawn) {
      onNew(origin.day, from, to);
    } else if (event.pointerType === "touch") {
      // A long press without moving: a new event there, default length.
      press.drawn = true;
      onNew(origin.day, from);
    }
    stop();
    // Let the click that follows see `drawn`, then forget it.
    setTimeout(() => {
      press.drawn = false;
    }, 0);
  };
  const cancel = (
    event: globalThis.KeyboardEvent | globalThis.PointerEvent
  ): void => {
    if (!(event instanceof KeyboardEvent) || event.key === "Escape") {
      press.drawn = false;
      stop();
    }
  };
  globalThis.addEventListener("pointermove", move);
  globalThis.addEventListener("pointerup", up);
  globalThis.addEventListener("pointercancel", cancel);
  globalThis.addEventListener("keydown", cancel);
  return () => {
    globalThis.removeEventListener("pointermove", move);
    globalThis.removeEventListener("pointerup", up);
    globalThis.removeEventListener("pointercancel", cancel);
    globalThis.removeEventListener("keydown", cancel);
  };
};

/** Where a drag started. */
interface DragOrigin {
  readonly id: string;
  readonly mode: "move" | "resize";
  readonly x: number;
  readonly y: number;
  /** Width of a day column on screen, when the drag started. */
  readonly columnWidth: number;
}

interface Drag {
  /** The same object for the whole gesture. */
  readonly origin: DragOrigin;
  readonly days: number;
  readonly minutes: number;
  readonly moved: boolean;
}

const snap = (minutes: number): number => Math.round(minutes / STEP) * STEP;

interface FollowOptions {
  /** The drag ended: move or resize the event if it changed. */
  readonly onDrop: (drag: Drag) => void;
  readonly setDrag: (drag: Drag | null) => void;
}

/**
 * Listens to the pointer during a drag; returns the cleanup. Once per
 * gesture: the progress lives here, and only a change of step (15 min, a
 * day) re-renders the grid.
 */
const followPointer = (
  origin: DragOrigin,
  { onDrop, setDrag }: FollowOptions
): (() => void) => {
  let latest: Drag = { days: 0, minutes: 0, moved: false, origin };
  let done = false;
  const move = (event: globalThis.PointerEvent): void => {
    if (done) {
      return;
    }
    const dx = event.clientX - origin.x;
    const dy = event.clientY - origin.y;
    const moved =
      latest.moved ||
      Math.abs(dx) > DRAG_THRESHOLD ||
      Math.abs(dy) > DRAG_THRESHOLD;
    const width = origin.columnWidth > 0 ? origin.columnWidth : 1;
    const next: Drag = {
      days:
        origin.mode === "move"
          ? Math.max(-6, Math.min(6, Math.round(dx / width)))
          : 0,
      minutes: snap(dy / PX_PER_MINUTE),
      moved,
      origin,
    };
    if (
      next.days !== latest.days ||
      next.minutes !== latest.minutes ||
      next.moved !== latest.moved
    ) {
      latest = next;
      setDrag(next);
    }
  };
  const up = (): void => {
    done = true;
    onDrop(latest);
    // Let the click that follows see `moved`, then forget the drag.
    setTimeout(() => {
      setDrag(null);
    }, 0);
  };
  const cancel = (event: globalThis.KeyboardEvent): void => {
    if (event.key === "Escape") {
      setDrag(null);
    }
  };
  globalThis.addEventListener("pointermove", move);
  globalThis.addEventListener("pointerup", up, { once: true });
  globalThis.addEventListener("keydown", cancel);
  return () => {
    globalThis.removeEventListener("pointermove", move);
    globalThis.removeEventListener("pointerup", up);
    globalThis.removeEventListener("keydown", cancel);
  };
};

const pad = (value: number): string => String(value).padStart(2, "0");

const hourText = (hour: number, locale: string): string =>
  dateTimeFormat(locale, { hour: "numeric", timeZone: "UTC" }).format(
    Date.UTC(2024, 0, 1, hour)
  );

/** Keys that move or resize a focused event; null for other keys. */
export const keyMove = (
  event: Pick<KeyboardEvent, "altKey" | "shiftKey" | "key">
): { days: number; minutes: number; resize: number } | null => {
  if (!event.altKey) {
    return null;
  }
  switch (event.key) {
    case "ArrowUp": {
      return event.shiftKey
        ? { days: 0, minutes: 0, resize: -STEP }
        : { days: 0, minutes: -STEP, resize: 0 };
    }
    case "ArrowDown": {
      return event.shiftKey
        ? { days: 0, minutes: 0, resize: STEP }
        : { days: 0, minutes: STEP, resize: 0 };
    }
    case "ArrowLeft": {
      return { days: -1, minutes: 0, resize: 0 };
    }
    case "ArrowRight": {
      return { days: 1, minutes: 0, resize: 0 };
    }
    default: {
      return null;
    }
  }
};

const BannerChip = ({
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
        styles.banner,
        looksPending(occurrence) ? chipStyles.tint : chipStyles.filled,
        looksPending(occurrence) ? chipStyles.pending : null,
        looksDeclined(occurrence) ? chipStyles.declined : null,
        banner.continuesBefore ? styles.bannerCut : null,
        banner.continuesAfter ? styles.bannerCutEnd : null,
        colorVars.color(occurrence.color),
        place.banner(
          `${String(banner.from + 2)} / ${String(banner.to + 3)}`,
          banner.row + 1
        )
      )}
    >
      <span {...stylex.props(chipStyles.title)}>
        {titleText(occurrence.title)}
      </span>
    </button>
  );
};

/** The range being drawn, with its times (announced as it changes). */
const DrawnRange = ({
  draw,
  locale,
}: {
  readonly draw: Draw;
  readonly locale: string;
}) => {
  const { from, to } = drawnRange(draw.origin.anchor, draw.current);
  // Wall times of the grid (like its hour labels), whatever the zone.
  const wall = (minutes: number): string =>
    formatTime(Date.UTC(2024, 0, 1, 0, minutes), "UTC", locale);
  const text = `${wall(from)} – ${wall(to)}`;
  return (
    <span
      aria-live="polite"
      {...stylex.props(
        styles.drawn,
        place.block(
          from * PX_PER_MINUTE,
          Math.max(12, (to - from) * PX_PER_MINUTE - 2),
          "0%",
          "calc(100% - 3px)"
        )
      )}
    >
      {m.cal_new_range({ time: text })}
    </span>
  );
};

interface BlockProps {
  readonly item: PlacedEvent;
  readonly zone: string;
  readonly locale: string;
  /** Only for the event being dragged (the others don't re-render). */
  readonly drag: Drag | null;
  readonly onPointerDown: (
    event: PointerEvent<HTMLElement>,
    item: PlacedEvent,
    mode: DragOrigin["mode"]
  ) => void;
  readonly onKeyMove: (
    occurrence: Occurrence,
    move: ReturnType<typeof keyMove>
  ) => void;
  readonly onOpen: (occurrence: Occurrence) => void;
}

const EventBlock = ({
  drag,
  item,
  locale,
  onKeyMove,
  onOpen,
  onPointerDown,
  zone,
}: BlockProps) => {
  const { occurrence } = item;
  const dragging = drag?.origin.id === occurrence.id && drag.moved;
  const mode = drag?.origin.mode;
  const shiftMinutes = dragging && mode === "move" ? drag.minutes : 0;
  const grow = dragging && mode === "resize" ? drag.minutes : 0;
  const shiftDays = dragging && mode === "move" ? drag.days : 0;
  const columnWidth = drag?.origin.columnWidth ?? 0;
  const top = (item.top + shiftMinutes) * PX_PER_MINUTE;
  const height = Math.max(
    16,
    (item.bottom - item.top + grow) * PX_PER_MINUTE - 2
  );
  const width = 100 / item.columns;
  const left = `calc(${String(item.column * width)}% + ${String(shiftDays * columnWidth)}px)`;
  const short = item.bottom - item.top < 45;
  return (
    <button
      aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown Alt+ArrowLeft Alt+ArrowRight Alt+Shift+ArrowUp Alt+Shift+ArrowDown"
      aria-label={eventLabel(occurrence, zone, locale)}
      data-event-id={occurrence.id}
      onClick={(event) => {
        // A drag ends with a click: that one doesn't open the event.
        if (event.detail !== 0 && drag?.moved === true) {
          return;
        }
        onOpen(occurrence);
      }}
      onKeyDown={(event) => {
        const move = keyMove(event);
        if (move !== null) {
          event.preventDefault();
          onKeyMove(occurrence, move);
        }
      }}
      onPointerDown={(event) => {
        onPointerDown(event, item, "move");
      }}
      type="button"
      {...stylex.props(
        shared.resetButton,
        shared.focusRing,
        styles.event,
        chipStyles.tint,
        looksPending(occurrence) ? chipStyles.pending : null,
        looksDeclined(occurrence) ? chipStyles.declined : null,
        dragging ? styles.eventDragging : null,
        colorVars.color(occurrence.color),
        place.block(top, height, left, `calc(${String(width)}% - 3px)`)
      )}
    >
      <span {...stylex.props(styles.eventRow)}>
        <span {...stylex.props(chipStyles.title)}>
          {titleText(occurrence.title)}
        </span>
        {short ? (
          <span {...stylex.props(chipStyles.time)}>
            {formatTime(occurrence.start, zone, locale)}
          </span>
        ) : null}
      </span>
      {short ? null : (
        <span {...stylex.props(chipStyles.time)}>
          <span {...stylex.props(styles.rangeWide)}>
            {timeRangeText(occurrence, zone, locale)}
          </span>
          <span {...stylex.props(styles.rangeNarrow)}>
            {occurrence.allDay
              ? timeRangeText(occurrence, zone, locale)
              : formatTime(occurrence.start, zone, locale)}
          </span>
        </span>
      )}
      {item.continuesAfter ? null : (
        <span
          aria-hidden
          onPointerDown={(event) => {
            event.stopPropagation();
            onPointerDown(event, item, "resize");
          }}
          {...stylex.props(styles.resize)}
        />
      )}
    </button>
  );
};

const NO_EXTRAS: readonly DayExtra[] = [];
const NO_TIMED: readonly TimedExtra[] = [];

export const TimeGrid = ({
  allDayExtras = NO_EXTRAS,
  days,
  locale,
  now,
  occurrences,
  onDay,
  onMove,
  onNew,
  onOpen,
  timedExtras = NO_TIMED,
  today,
  zone,
}: TimeGridProps) => {
  const scroller = useRef<HTMLDivElement>(null);
  const columnsRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);

  // Start the day at 7:00 (or an hour before now, today), like most
  // calendars. Once per range: the parent remounts the grid (key) when the
  // days change.
  const scrollToStart = useEffectEvent(() => {
    const element = scroller.current;
    if (element === null) {
      return;
    }
    const showsToday = days.some((day) => day.compare(today) === 0);
    const hour = showsToday
      ? Math.max(0, Math.floor(minutesInDay(Date.now(), zone) / 60) - 1)
      : 7;
    // A little above the hour's line: its label is centred on the line,
    // so landing on it hides the top half under the day header. Late in
    // the day the browser clamps that to the end of the scroll, which
    // would land anywhere: snap the end to an hour line as well.
    const end = element.scrollHeight - element.clientHeight;
    element.scrollTop = Math.max(
      0,
      Math.min(hour * HOUR - 12, Math.floor((end + 12) / HOUR) * HOUR - 12)
    );
  });
  useEffect(() => {
    scrollToStart();
  }, []);

  const banners = placeBanners(occurrences, days, zone);
  const bannerRows = Math.max(0, ...banners.map((banner) => banner.row + 1));
  // Extras go under the banners, one row each, per day.
  const extraRow = new Map<string, number>();
  const perDay = new Map<number, number>();
  for (const extra of allDayExtras) {
    const row = perDay.get(extra.day) ?? 0;
    perDay.set(extra.day, row + 1);
    extraRow.set(extra.key, bannerRows + row + 1);
  }
  const allDayRows = bannerRows + Math.max(0, ...perDay.values());
  const placedByDay = days.map((day) => placeDay(occurrences, day, zone));
  const nowMinutes = minutesInDay(now, zone);

  const handlePointerDown = (
    event: PointerEvent<HTMLElement>,
    item: PlacedEvent,
    mode: DragOrigin["mode"]
  ): void => {
    if (event.pointerType === "touch" || event.button !== 0) {
      return;
    }
    const columnWidth =
      (columnsRef.current?.getBoundingClientRect().width ?? 0) / days.length;
    setDrag({
      days: 0,
      minutes: 0,
      moved: false,
      origin: {
        columnWidth,
        id: item.occurrence.id,
        mode,
        x: event.clientX,
        y: event.clientY,
      },
    });
  };

  // The latest events and callback when the drag ends (not a reason to
  // listen again).
  const dropDrag = useEffectEvent((ended: Drag): void => {
    const { origin } = ended;
    const occurrence = occurrences.find((item) => item.id === origin.id);
    if (
      ended.moved &&
      occurrence !== undefined &&
      (ended.minutes !== 0 || ended.days !== 0)
    ) {
      onMove(
        origin.mode === "move"
          ? {
              days: ended.days,
              minutes: ended.minutes,
              occurrence,
              resize: 0,
            }
          : { days: 0, minutes: 0, occurrence, resize: ended.minutes }
      );
    }
  });

  // Pointer moves anywhere on the page while dragging: listened once per
  // drag (its origin), not once per move.
  const dragOrigin = drag?.origin ?? null;
  useEffect(
    () =>
      dragOrigin === null
        ? undefined
        : followPointer(dragOrigin, {
            onDrop: (ended) => {
              dropDrag(ended);
            },
            setDrag,
          }),
    [dragOrigin]
  );

  const handleKeyMove = (
    occurrence: Occurrence,
    move: ReturnType<typeof keyMove>
  ): void => {
    if (move !== null) {
      onMove({ ...move, occurrence });
    }
  };

  // Drawing a range on an empty part of a day: press and drag with a
  // mouse or pen; with a finger, rest it for a moment first (a quick swipe
  // still scrolls). A plain click or tap still creates a 30-minute slot.
  const [draw, setDraw] = useState<Draw | null>(null);
  const press = useRef<Press>({
    drawing: false,
    drawn: false,
    start: null,
    timer: null,
  });

  const startDraw = (event: PointerEvent<HTMLElement>): void => {
    const { target } = event;
    if (
      event.button !== 0 ||
      !(target instanceof Element) ||
      target.closest("button, [data-no-draw]") !== null
    ) {
      return;
    }
    // The day under the pointer (the target may be an hour line).
    const column = [
      ...event.currentTarget.querySelectorAll<HTMLElement>("[data-day]"),
    ].find((element) => {
      const box = element.getBoundingClientRect();
      return event.clientX >= box.left && event.clientX < box.right;
    });
    const dayIndex = Number(column?.dataset.day ?? Number.NaN);
    if (column === undefined || !Number.isInteger(dayIndex)) {
      return;
    }
    const { top } = column.getBoundingClientRect();
    const minute = minuteAt(event.clientY, top);
    const byFinger = event.pointerType === "touch";
    const next: Draw = {
      current: minute,
      moved: false,
      origin: {
        anchor: minute,
        day: dayIndex,
        pointerId: event.pointerId,
        top,
        touch: byFinger,
      },
    };
    const state = press.current;
    state.drawn = false;
    if (byFinger) {
      cancelPress(state);
      state.start = { x: event.clientX, y: event.clientY };
      state.timer = setTimeout(() => {
        state.timer = null;
        setDraw(next);
      }, LONG_PRESS_MS);
      return;
    }
    setDraw(next);
  };

  // The latest days and callback when a range is drawn.
  const newRange = useEffectEvent(
    (dayIndex: number, from: number, to?: number): void => {
      const day = days[dayIndex];
      if (day === undefined) {
        return;
      }
      if (to === undefined) {
        onNew(localAt(day, from));
      } else {
        onNew(localAt(day, from), localAt(day, to));
      }
    }
  );

  // While drawing: follow the pointer anywhere, finish on release, Esc
  // cancels. With a finger, the page must not scroll meanwhile. Listened
  // once per range (its origin), not once per move.
  const drawOrigin = draw?.origin ?? null;
  useEffect(
    () =>
      drawOrigin === null
        ? undefined
        : followDraw(drawOrigin, {
            onNew: (dayIndex, from, to) => {
              newRange(dayIndex, from, to);
            },
            press,
            setDraw,
          }),
    [drawOrigin, press]
  );

  // During a long press, moving the finger means scrolling, not drawing.
  useEffect(() => {
    const element = columnsRef.current;
    return element === null ? undefined : watchPress(element, press);
  }, [press]);

  const newAt = (
    day: CalendarDate,
    clientY: number,
    element: HTMLElement
  ): void => {
    if (press.current.drawn) {
      return;
    }
    const box = element.getBoundingClientRect();
    const minutes = Math.min(
      DAY_MINUTES - 30,
      Math.max(0, Math.floor((clientY - box.top) / PX_PER_MINUTE / 30) * 30)
    );
    onNew(
      `${day.toString()}T${pad(Math.floor(minutes / 60))}:${pad(minutes % 60)}`
    );
  };

  return (
    <div {...stylex.props(styles.root)}>
      <div
        ref={scroller}
        {...stylex.props(styles.scroll, place.days(days.length))}
      >
        <div {...stylex.props(sticky.top, minDays.wide)}>
          <div {...stylex.props(styles.head)}>
            <span
              title={timeZoneLabel(zone, locale, now)}
              {...stylex.props(styles.gutterSpacer, styles.zone)}
            >
              {timeZoneOffset(zone, locale, now)}
            </span>
            {days.map((day) => {
              const isToday = day.compare(today) === 0;
              return (
                <div
                  data-drop-date={day.toString()}
                  data-drop-kind="all-day"
                  key={day.toString()}
                  {...stylex.props(styles.dayHeader)}
                >
                  <button
                    aria-current={isToday ? "date" : undefined}
                    aria-label={m.cal_open_day({
                      day: formatDate(day, locale, { dateStyle: "full" }),
                    })}
                    onClick={() => {
                      onDay(day);
                    }}
                    type="button"
                    {...stylex.props(
                      shared.resetButton,
                      shared.focusRing,
                      styles.dayButton
                    )}
                  >
                    <span {...stylex.props(styles.dayName)}>
                      {formatDate(day, locale, { weekday: "short" })}
                    </span>
                    <span
                      {...stylex.props(
                        styles.dayNumber,
                        isToday ? styles.dayNumberToday : null
                      )}
                    >
                      {day.day}
                    </span>
                  </button>
                </div>
              );
            })}
          </div>
          {allDayRows > 0 ? (
            <div
              data-drop-kind="all-day"
              {...stylex.props(styles.allDay, place.columns(days.length))}
            >
              <span {...stylex.props(styles.allDayLabel)}>
                {m.cal_all_day()}
              </span>
              {banners.map((banner) => (
                <BannerChip
                  banner={banner}
                  key={`${banner.occurrence.id}-${String(banner.from)}`}
                  locale={locale}
                  onOpen={onOpen}
                  zone={zone}
                />
              ))}
              {allDayExtras.map((extra) => (
                <div
                  data-no-draw
                  key={extra.key}
                  {...stylex.props(
                    extraStyles.allDay,
                    place.banner(
                      `${String(extra.day + 2)} / ${String(extra.day + 3)}`,
                      extraRow.get(extra.key) ?? 1
                    )
                  )}
                >
                  {extra.node}
                </div>
              ))}
            </div>
          ) : null}
        </div>
        <div {...stylex.props(styles.body, minDays.wide)}>
          <div aria-hidden {...stylex.props(styles.gutter)}>
            {Array.from({ length: 23 }, (_, index) => index + 1).map((hour) => (
              <span
                key={hour}
                {...stylex.props(styles.hourLabel, place.top(hour * HOUR))}
              >
                {hourText(hour, locale)}
              </span>
            ))}
          </div>
          {/* Pressing on empty time draws a new event (startDraw); from
              the keyboard, C and the editor's times do the same. */}
          {/* oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- the pointer way of drawing a range; the keyboard way is C. */}
          <div
            onPointerDown={startDraw}
            ref={columnsRef}
            {...stylex.props(styles.columns)}
          >
            {Array.from({ length: 23 }, (_, index) => index + 1).map((hour) => (
              <span
                aria-hidden
                key={hour}
                {...stylex.props(styles.hourLine, place.top(hour * HOUR))}
              />
            ))}
            {days.map((day, index) => {
              const isToday = day.compare(today) === 0;
              return (
                // A click on an empty hour creates an event there. From the
                // keyboard, "C" and the "New event" button do the same.
                // oxlint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-noninteractive-element-interactions
                <section
                  aria-label={formatDate(day, locale, { dateStyle: "full" })}
                  key={day.toString()}
                  onClick={(event) => {
                    if (onEmptyTime(event)) {
                      newAt(day, event.clientY, event.currentTarget);
                    }
                  }}
                  onContextMenu={(event) => {
                    // A finger resting to draw a range: no context menu.
                    if (press.current.start !== null || draw !== null) {
                      event.preventDefault();
                    }
                  }}
                  data-day={index}
                  // Where a dragged task lands (components/tasks).
                  data-drop-date={day.toString()}
                  data-drop-kind="time"
                  {...stylex.props(
                    styles.column,
                    isToday ? styles.columnToday : null,
                    draw === null ? null : styles.columnDrawing
                  )}
                >
                  {draw?.origin.day === index &&
                  (draw.moved || draw.origin.touch) ? (
                    <DrawnRange draw={draw} locale={locale} />
                  ) : null}
                  {(placedByDay[index] ?? []).map((item) => (
                    <EventBlock
                      drag={
                        drag?.origin.id === item.occurrence.id ? drag : null
                      }
                      item={item}
                      key={item.occurrence.id}
                      locale={locale}
                      onKeyMove={handleKeyMove}
                      onOpen={onOpen}
                      onPointerDown={handlePointerDown}
                      zone={zone}
                    />
                  ))}
                  {timedExtras
                    .filter((extra) => extra.day === index)
                    .map((extra) => (
                      <div
                        data-no-draw
                        key={extra.key}
                        {...stylex.props(
                          extraStyles.timed,
                          place.top(extra.minutes * PX_PER_MINUTE)
                        )}
                      >
                        {extra.node}
                      </div>
                    ))}
                  {isToday && now > 0 ? (
                    <span
                      aria-hidden
                      {...stylex.props(
                        styles.nowLine,
                        place.top(nowMinutes * PX_PER_MINUTE)
                      )}
                    >
                      <span {...stylex.props(styles.nowDot)} />
                    </span>
                  ) : null}
                </section>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
};
