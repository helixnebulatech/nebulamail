// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// What the calendar shows lives in the URL (/calendar?view=week&date=…),
// so reload, back and links work: the view, the day, the open event (and
// its account, for calendars shared by others) and whether it is being
// edited, or the range of a new event.

import type { CalendarDate } from "@internationalized/date";
import { parseDate, today } from "@internationalized/date";
import type { AccountId, CalendarEventId } from "@nebulamail/jmap-client";
import {
  AccountId as AccountIdSchema,
  CalendarEventId as CalendarEventIdSchema,
} from "@nebulamail/jmap-client";
import { getRouteApi, useNavigate } from "@tanstack/react-router";

import { nextSlot } from "../../calendar/api";
import { useTimeZone } from "../../calendar/hooks";
import type { CalendarView } from "../../calendar/layout";
import { shiftAnchor } from "../../calendar/layout";
import { usePhone } from "../mail/use-phone";

const route = getRouteApi("/calendar");

export interface EventTarget {
  readonly id: CalendarEventId;
  readonly baseId: CalendarEventId;
  /** null: the user's own account. */
  readonly accountId: AccountId | null;
}

/** A new event: its start and, when dragged, its end. */
export interface NewRange {
  readonly start: string;
  readonly end: string | null;
}

export interface CalendarNav {
  readonly view: CalendarView;
  readonly anchor: CalendarDate;
  readonly todayDate: CalendarDate;
  readonly event: EventTarget | null;
  readonly editing: boolean;
  /** Start of a new event being created, if any. */
  readonly newAt: string | null;
  /** Its end, when a range was dragged. */
  readonly newEnd: string | null;
  readonly setView: (view: CalendarView) => void;
  readonly goTo: (date: CalendarDate, view?: CalendarView) => void;
  readonly shift: (direction: -1 | 1) => void;
  readonly goToday: () => void;
  readonly openEvent: (target: EventTarget) => void;
  readonly editEvent: (target: EventTarget) => void;
  readonly newEvent: (start: string, end?: string | null) => void;
  /** A new event at a sensible time of the day shown. */
  readonly newEventHere: () => void;
  readonly closePanel: () => void;
}

const DEFAULT_VIEW: CalendarView = "week";
/** Seven columns don't fit a phone: one day, unless another was chosen. */
const PHONE_VIEW: CalendarView = "day";

const parseId = (value: string | undefined): CalendarEventId | null => {
  const parsed = CalendarEventIdSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
};

const parseAccount = (value: string | undefined): AccountId | null => {
  const parsed = AccountIdSchema.safeParse(value);
  return parsed.success ? parsed.data : null;
};

export const useCalendarNav = (): CalendarNav => {
  const search = route.useSearch();
  const navigate = useNavigate();
  const zone = useTimeZone();
  const todayDate = today(zone);
  let anchor = todayDate;
  if (search.date !== undefined) {
    try {
      anchor = parseDate(search.date);
    } catch {
      anchor = todayDate;
    }
  }
  const defaultView = usePhone() ? PHONE_VIEW : DEFAULT_VIEW;
  const view = search.view ?? defaultView;
  const id = parseId(search.event);
  const baseId = parseId(search.series) ?? id;
  const event =
    id === null || baseId === null
      ? null
      : { accountId: parseAccount(search.account), baseId, id };
  const newAt = search.new ?? null;
  const newEnd = newAt === null ? null : (search.end ?? null);

  const go = (
    next: {
      readonly view?: CalendarView;
      readonly date?: CalendarDate;
      readonly event?: EventTarget | null;
      readonly edit?: boolean;
      readonly newAt?: string | null;
      readonly newEnd?: string | null;
    },
    replace = false
  ): void => {
    const target = next.event === undefined ? event : next.event;
    const date = next.date ?? anchor;
    const start = next.newAt ?? undefined;
    void navigate({
      replace,
      search: {
        account: target?.accountId ?? undefined,
        date: date.compare(todayDate) === 0 ? undefined : date.toString(),
        edit: next.edit === true ? "1" : undefined,
        end: start === undefined ? undefined : (next.newEnd ?? undefined),
        event: target?.id,
        new: start,
        series:
          target === null || target.baseId === target.id
            ? undefined
            : target.baseId,
        view:
          (next.view ?? view) === defaultView ? undefined : (next.view ?? view),
      },
      to: "/calendar",
    });
  };

  const keep = {
    edit: search.edit === "1",
    newAt,
    newEnd,
  };

  return {
    anchor,
    closePanel: () => {
      go({ edit: false, event: null, newAt: null });
    },
    editEvent: (target) => {
      go({ edit: true, event: target, newAt: null });
    },
    editing: search.edit === "1" && event !== null,
    event,
    goTo: (date, nextView) => {
      go(
        {
          date,
          ...(nextView === undefined ? {} : { view: nextView }),
          ...keep,
        },
        true
      );
    },
    goToday: () => {
      go({ date: todayDate, ...keep }, true);
    },
    newAt,
    newEnd,
    newEvent: (start, end = null) => {
      go({ edit: false, event: null, newAt: start, newEnd: end });
    },
    newEventHere: () => {
      const start =
        anchor.compare(todayDate) === 0
          ? nextSlot(Date.now(), zone).slice(0, 16)
          : `${anchor.toString()}T09:00`;
      go({ edit: false, event: null, newAt: start });
    },
    openEvent: (target) => {
      go({ edit: false, event: target, newAt: null });
    },
    setView: (next) => {
      go({ ...keep, view: next }, true);
    },
    shift: (direction) => {
      go({ date: shiftAnchor(view, anchor, direction), ...keep }, true);
    },
    todayDate,
    view,
  };
};
