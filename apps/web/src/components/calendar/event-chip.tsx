// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// How an event looks in the views: the calendar's colour as a bar or a
// tint, dashed when the user hasn't answered or said "maybe", faded and
// struck through when declined or cancelled. The accessible name says it
// all in words (title, time, place, answer).

import { cleanDisplayText } from "@nebulamail/mail-render";
import { colors, font, radius } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";

import type { Occurrence } from "../../calendar/model";
import { formatTime } from "../../calendar/time";
import { m } from "../../paraglide/messages.js";
import { titleText } from "./event-text";

export const chipStyles = stylex.create({
  bar: {
    backgroundColor: "var(--event-color)",
    borderRadius: "3px",
    flexShrink: 0,
    width: "3px",
  },
  dashed: {
    borderStyle: "dashed",
    borderWidth: "1px",
  },
  declined: { opacity: 0.55, textDecorationLine: "line-through" },
  dot: {
    backgroundColor: "var(--event-color)",
    borderRadius: "50%",
    flexShrink: 0,
    height: "8px",
    width: "8px",
  },
  filled: {
    backgroundColor: "var(--event-color)",
    borderColor: "var(--event-color)",
    color: "#FFFFFF",
  },
  /** Tinted block (week and day view). */
  tint: {
    backgroundColor: `color-mix(in srgb, var(--event-color) 16%, ${colors.bg})`,
    borderColor: `color-mix(in srgb, var(--event-color) 45%, ${colors.bg})`,
    borderRadius: radius.md,
    borderStyle: "solid",
    borderWidth: "1px",
    color: colors.fg,
  },
  pending: {
    backgroundColor: colors.bg,
    borderColor: "var(--event-color)",
    borderStyle: "dashed",
  },
  time: {
    color: colors.fg3,
    fontSize: font.sizeXs,
    fontVariantNumeric: "tabular-nums",
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
  timeOnFill: { color: "rgba(255, 255, 255, 0.85)" },
  title: {
    fontWeight: font.weightSemibold,
    minWidth: 0,
    overflow: "hidden",
    textOverflow: "ellipsis",
    whiteSpace: "nowrap",
  },
});

/** The `--event-color` custom property of an occurrence (validated colour). */
export const colorVars = stylex.create({
  color: (color: string) => ({ "--event-color": color }),
});

/** Whether to draw it as not (yet) going. */
export const looksPending = (occurrence: Occurrence): boolean =>
  occurrence.myStatus === "needs-action" || occurrence.myStatus === "tentative";

export const looksDeclined = (occurrence: Occurrence): boolean =>
  occurrence.myStatus === "declined" || occurrence.status === "cancelled";

const statusWord = (occurrence: Occurrence): string | null => {
  if (occurrence.status === "cancelled") {
    return m.cal_status_cancelled();
  }
  switch (occurrence.myStatus) {
    case "needs-action": {
      return m.cal_rsvp_needed();
    }
    case "tentative": {
      return m.cal_rsvp_tentative();
    }
    case "declined": {
      return m.cal_rsvp_declined();
    }
    case "accepted":
    case "delegated":
    case null: {
      return null;
    }
    default: {
      return occurrence.myStatus satisfies never;
    }
  }
};

export const timeRangeText = (
  occurrence: Occurrence,
  zone: string,
  locale: string
): string =>
  occurrence.allDay
    ? m.cal_all_day()
    : `${formatTime(occurrence.start, zone, locale)} – ${formatTime(occurrence.end, zone, locale)}`;

/** Spoken name of an event button. */
export const eventLabel = (
  occurrence: Occurrence,
  zone: string,
  locale: string,
  day?: string
): string =>
  [
    titleText(occurrence.title),
    ...(day === undefined ? [] : [day]),
    timeRangeText(occurrence, zone, locale),
    ...(occurrence.location === null
      ? []
      : [cleanDisplayText(occurrence.location)]),
    ...(occurrence.recurring ? [m.cal_repeats()] : []),
    ...[statusWord(occurrence)].filter((word) => word !== null),
  ].join(", ");
